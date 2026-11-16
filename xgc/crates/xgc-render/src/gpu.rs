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
    Mesh(String),
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
            Self::Mesh(message) => write!(f, "网格加载失败：{message}"),
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

    pub fn into_texture(self) -> Option<wgpu::Texture> {
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
