//! wgpu 资源创建层：把 CPU 侧的 `ResourceDesc` 变成真实纹理 / 缓冲。
//!
//! 这一层只负责设备与资源；pass 录制 / 管线 / 着色器在下一刀。无 GPU 的环境
//! 里 `GpuContext::headless()` 返回错误，调用方自行降级，测试里按"无适配器则跳过"处理。

use std::collections::HashMap;
use std::fmt;

use super::draw::DrawList;
use super::mesh::{builtin_mesh, Mesh};
use super::{ResourceDesc, ResourceKind, ResourceTable, ResourceUsage};

#[derive(Debug)]
pub enum GpuError {
    NoAdapter(String),
    RequestDevice(String),
    UnsupportedFormat(String),
    UnsupportedUsage(String),
    Readback(String),
    Pipeline(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(message) => write!(f, "没有可用的 GPU 适配器：{message}"),
            Self::RequestDevice(message) => write!(f, "创建 GPU 设备失败：{message}"),
            Self::UnsupportedFormat(format) => write!(f, "不支持的渲染格式：{format}"),
            Self::UnsupportedUsage(resource) => {
                write!(f, "资源 {resource} 的用途无法映射到 wgpu")
            }
            Self::Readback(message) => write!(f, "GPU 回读失败：{message}"),
            Self::Pipeline(message) => write!(f, "GPU 管线错误：{message}"),
        }
    }
}

impl std::error::Error for GpuError {}

pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
}

impl GpuContext {
    pub fn headless() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| GpuError::NoAdapter("没有匹配的适配器".to_string()))?;
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("xgc-device"),
                ..Default::default()
            },
            None,
        ))
        .map_err(|err| GpuError::RequestDevice(err.to_string()))?;
        Ok(Self {
            device,
            queue,
            adapter_info,
        })
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn create_resource(&self, desc: &ResourceDesc) -> Result<GpuResource, GpuError> {
        if !desc.readable() && !desc.writable() {
            return Err(GpuError::UnsupportedUsage(desc.name.clone()));
        }
        match desc.kind {
            ResourceKind::Texture2D => {
                let format = parse_format(&desc.format)?;
                let usage = texture_usage(desc)?;
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&desc.name),
                    size: wgpu::Extent3d {
                        width: desc.width.max(1),
                        height: desc.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                });
                Ok(GpuResource::Texture(texture))
            }
            ResourceKind::Buffer => {
                let usage = buffer_usage(desc)?;
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&desc.name),
                    size: desc.size.max(4),
                    usage,
                    mapped_at_creation: false,
                });
                Ok(GpuResource::Buffer(buffer))
            }
        }
    }

    pub fn create_resources(&self, table: &ResourceTable) -> Result<GpuResourceTable, GpuError> {
        let mut resources = HashMap::with_capacity(table.len());
        for desc in table.iter() {
            resources.insert(desc.name.clone(), self.create_resource(desc)?);
        }
        Ok(GpuResourceTable { resources })
    }

    /// 按 draw list 的实体顺序出 draw call；空列表清黑，首项 clear，后续 load。
    pub fn draw_list(&self, texture: &wgpu::Texture, list: &DrawList) -> Result<(), GpuError> {
        let pipeline = self.create_triangle_pipeline(texture.format());
        if list.items.is_empty() {
            return self.clear_texture(texture, wgpu::Color::BLACK);
        }
        for (index, item) in list.items.iter().enumerate() {
            let mesh = builtin_mesh(&item.model).unwrap_or_else(Mesh::fullscreen_triangle);
            pipeline.draw_mesh(self, texture, &mesh, item.color, index == 0)?;
        }
        Ok(())
    }

    /// 录制并提交一个只做 clear 的 render pass；用于资源 / 通道验证。
    pub fn clear_texture_rgba(
        &self,
        texture: &wgpu::Texture,
        rgba: [f64; 4],
    ) -> Result<(), GpuError> {
        self.clear_texture(
            texture,
            wgpu::Color {
                r: rgba[0],
                g: rgba[1],
                b: rgba[2],
                a: rgba[3],
            },
        )
    }

    /// 画一个覆盖屏幕的三角形：最小可验证的管线 / 着色器 / draw call 路径。
    pub fn draw_triangle(&self, texture: &wgpu::Texture) -> Result<(), GpuError> {
        let pipeline = self.create_triangle_pipeline(texture.format());
        pipeline.draw(self, texture, [0.1, 0.8, 0.2, 1.0])
    }

    /// 创建可复用的三角形管线；颜色由 uniform 缓冲提供。
    pub fn create_triangle_pipeline(&self, format: wgpu::TextureFormat) -> TrianglePipeline {
        const TRIANGLE_WGSL: &str = r#"
struct Material {
    color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> material: Material;

@vertex
fn vs(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return material.color;
}
"#;
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-triangle"),
                source: wgpu::ShaderSource::Wgsl(TRIANGLE_WGSL.into()),
            });
        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-triangle-material"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xgc-triangle-layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-triangle-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            });
        TrianglePipeline {
            pipeline,
            bind_group_layout,
            format,
        }
    }

    /// 录制并提交一个只做 clear 的 render pass；用于资源 / 通道验证。
    pub fn clear_texture(
        &self,
        texture: &wgpu::Texture,
        color: wgpu::Color,
    ) -> Result<(), GpuError> {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xgc-clear"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// 回读 `rgba8unorm` 纹理为紧凑 RGBA 字节；行对齐在内部处理。
    pub fn read_texture_rgba8(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, GpuError> {
        if texture.format() != wgpu::TextureFormat::Rgba8Unorm {
            return Err(GpuError::Readback(format!(
                "只支持 rgba8unorm 回读，实际 {:?}",
                texture.format()
            )));
        }
        let bytes_per_row = width * 4;
        let padded_row = bytes_per_row.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xgc-readback-encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        receiver
            .recv()
            .map_err(|err| GpuError::Readback(err.to_string()))?
            .map_err(|err| GpuError::Readback(err.to_string()))?;

        let data = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((bytes_per_row * height) as usize);
        for row in 0..height {
            let start = (row * padded_row) as usize;
            pixels.extend_from_slice(&data[start..start + bytes_per_row as usize]);
        }
        drop(data);
        buffer.unmap();
        Ok(pixels)
    }
}

pub struct TrianglePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

impl TrianglePipeline {
    /// 用材质颜色画覆盖屏幕的三角形；uniform 缓冲每次重建，后续按材质缓存。
    pub fn draw(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        color: [f32; 4],
    ) -> Result<(), GpuError> {
        self.draw_mesh(context, texture, &Mesh::fullscreen_triangle(), color, true)
    }

    /// 在已有颜色上继续画，不 clear；用于同一次 pass 的后续 draw。
    pub fn draw_over(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        color: [f32; 4],
    ) -> Result<(), GpuError> {
        self.draw_mesh(context, texture, &Mesh::fullscreen_triangle(), color, false)
    }

    /// 用真实顶点缓冲画网格；`clear` 控制是否先清黑。
    pub fn draw_mesh(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        mesh: &Mesh,
        color: [f32; 4],
        clear: bool,
    ) -> Result<(), GpuError> {
        let load = if clear {
            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
        } else {
            wgpu::LoadOp::Load
        };
        self.draw_with(context, texture, mesh, color, load)
    }

    fn draw_with(
        &self,
        context: &GpuContext,
        texture: &wgpu::Texture,
        mesh: &Mesh,
        color: [f32; 4],
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Result<(), GpuError> {
        if texture.format() != self.format {
            return Err(GpuError::Pipeline(format!(
                "纹理格式 {:?} 与管线 {:?} 不一致",
                texture.format(),
                self.format
            )));
        }
        let bytes: Vec<u8> = color
            .iter()
            .flat_map(|component| component.to_le_bytes())
            .collect();
        let buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-material"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&buffer, 0, &bytes);

        if mesh.positions.is_empty() {
            return Ok(());
        }
        let vertex_bytes: Vec<u8> = mesh
            .positions
            .iter()
            .flat_map(|position| position.iter().flat_map(|value| value.to_le_bytes()))
            .collect();
        let vertex_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&vertex_buffer, 0, &vertex_bytes);
        let bind_group = context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-material-bind"),
                layout: &self.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xgc-triangle-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-triangle-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.draw(0..mesh.positions.len() as u32, 0..1);
        }
        context.queue.submit(Some(encoder.finish()));
        Ok(())
    }
}

pub enum GpuResource {
    Texture(wgpu::Texture),
    Buffer(wgpu::Buffer),
}

impl GpuResource {
    pub fn as_texture(&self) -> Option<&wgpu::Texture> {
        match self {
            Self::Texture(texture) => Some(texture),
            Self::Buffer(_) => None,
        }
    }
}

pub struct GpuResourceTable {
    resources: HashMap<String, GpuResource>,
}

impl GpuResourceTable {
    pub fn get(&self, name: &str) -> Option<&GpuResource> {
        self.resources.get(name)
    }

    pub fn len(&self) -> usize {
        self.resources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

fn parse_format(text: &str) -> Result<wgpu::TextureFormat, GpuError> {
    match text {
        "rgba8unorm" => Ok(wgpu::TextureFormat::Rgba8Unorm),
        "rgba8unorm-srgb" => Ok(wgpu::TextureFormat::Rgba8UnormSrgb),
        "rgba16float" => Ok(wgpu::TextureFormat::Rgba16Float),
        "rgba32float" => Ok(wgpu::TextureFormat::Rgba32Float),
        "bgra8unorm" => Ok(wgpu::TextureFormat::Bgra8Unorm),
        "depth32float" => Ok(wgpu::TextureFormat::Depth32Float),
        "depth24plus" => Ok(wgpu::TextureFormat::Depth24Plus),
        "r32float" => Ok(wgpu::TextureFormat::R32Float),
        other => Err(GpuError::UnsupportedFormat(other.to_string())),
    }
}

fn texture_usage(desc: &ResourceDesc) -> Result<wgpu::TextureUsages, GpuError> {
    let mut usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
    let mut mapped = false;
    for usage_flag in &desc.usage {
        match usage_flag {
            ResourceUsage::Sampled => {
                usage |= wgpu::TextureUsages::TEXTURE_BINDING;
                mapped = true;
            }
            ResourceUsage::ColorAttachment | ResourceUsage::DepthAttachment => {
                usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                mapped = true;
            }
            ResourceUsage::Storage => {
                usage |= wgpu::TextureUsages::STORAGE_BINDING;
                mapped = true;
            }
            ResourceUsage::Uniform => {}
        }
    }
    if mapped {
        Ok(usage)
    } else {
        Err(GpuError::UnsupportedUsage(desc.name.clone()))
    }
}

fn buffer_usage(desc: &ResourceDesc) -> Result<wgpu::BufferUsages, GpuError> {
    let mut usage = wgpu::BufferUsages::COPY_DST;
    let mut mapped = false;
    for usage_flag in &desc.usage {
        match usage_flag {
            ResourceUsage::Uniform => {
                usage |= wgpu::BufferUsages::UNIFORM;
                mapped = true;
            }
            ResourceUsage::Storage => {
                usage |= wgpu::BufferUsages::STORAGE;
                mapped = true;
            }
            ResourceUsage::Sampled
            | ResourceUsage::ColorAttachment
            | ResourceUsage::DepthAttachment => {}
        }
    }
    if mapped {
        Ok(usage)
    } else {
        Err(GpuError::UnsupportedUsage(desc.name.clone()))
    }
}
