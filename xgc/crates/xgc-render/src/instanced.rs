//! 实例化绘制：按网格分组，一次 draw 覆盖同模型的多个实体。

use std::collections::HashMap;

use super::assets::MeshCache;
use super::camera::Mat4;
use super::draw::DrawList;
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::lights::{self, GpuLight, MAX_LIGHTS};
use super::mesh::Mesh;
use super::pipeline::RenderCache;
use super::shadow::{ShadowCascades, SHADOW_CASCADES};
use super::texture_bind::TextureBinding;
use xgc_world::SceneEnvironment;

/// 帧 uniform：view-proj 64B + 相机 / ambient 2 个 vec4 + 9 个 vec4 的天空球谐
/// + 天空开关 vec4 + 4 级光视图投影 256B + 切分 / 视线 / 参数 / 阴影画质 4 个 vec4
/// + 光源盏数 vec4 + 8 盏灯各 48B = 976B。
///
/// 光源本身只在 `lights[]` 里出现一次：以前另有 sun_dir / sun_color 两个 vec4，
/// 多光源之后它们会与 `lights[0]` 重复且可能不一致，已删。
pub(crate) const FRAME_UNIFORM_BYTES: u64 =
    576 + 16 + (MAX_LIGHTS as u64) * lights::LIGHT_UNIFORM_BYTES;

/// 帧级光照参数：环境光 + 全部可用光源（`lights` / `light_count` 进 uniform）。
///
/// `sun_*` 三个字段不进 uniform，只给 CPU 侧消费：天空太阳盘与自动取景的顺光需要
/// "主方向光"这一个标量摘要。着色永远走 `lights[]`，所以两者不会各自漂移。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameLighting {
    pub(crate) eye: [f32; 3],
    pub(crate) ambient: [f32; 3],
    pub(crate) sun_direction: [f32; 3],
    pub(crate) sun_intensity: f32,
    pub(crate) sun_color: [f32; 3],
    /// 天空漫反射环境（E/π 口径的 L2 球谐）；`None` = 天空关，只剩 `ambient`。
    pub(crate) sky_irradiance: Option<[[f32; 4]; 9]>,
    /// 4 级方向光级联；`None` = 这一帧没有可用阴影。
    pub(crate) shadow_cascades: Option<ShadowCascades>,
    /// 软阴影（PCSS）的灯光尺寸；0 = 硬阴影（与引入 PCSS 之前逐位一致）。
    pub(crate) shadow_softness: f32,
    /// 级间混合带宽占该级跨度的比例；0 = 两级硬接。
    pub(crate) shadow_cascade_blend: f32,
    /// 本帧参与着色的光源（与旧端 `LightsUniform` 同布局），空位留 0。
    pub(crate) lights: [GpuLight; MAX_LIGHTS],
    /// 有效光源数；着色器按它提前退出循环。
    pub(crate) light_count: u32,
}

impl FrameLighting {
    pub(crate) fn from_environment(environment: &SceneEnvironment, eye: [f32; 3]) -> Self {
        let sun = environment.sun();
        let packed = lights::pack(environment);
        Self {
            eye,
            ambient: environment.ambient,
            sun_direction: sun
                .map(|light| light.normalized_direction())
                .unwrap_or([0.0, -1.0, 0.0]),
            sun_intensity: sun.map(|light| light.intensity).unwrap_or(0.0),
            sun_color: sun.map(|light| light.color).unwrap_or([1.0; 3]),
            sky_irradiance: None,
            shadow_cascades: None,
            shadow_softness: 0.0,
            shadow_cascade_blend: 0.0,
            lights: packed.lights,
            light_count: packed.count,
        }
    }

    /// 挂上 4 级方向光级联（`None` = 不采样阴影）。
    pub(crate) fn with_shadow(mut self, cascades: Option<ShadowCascades>) -> Self {
        self.shadow_cascades = cascades;
        self
    }

    /// 挂上阴影画质参数（软阴影尺寸 + 级间混合带宽）：0 / 0 就是老路径。
    pub(crate) fn with_shadow_quality(mut self, softness: f32, cascade_blend: f32) -> Self {
        self.shadow_softness = softness;
        self.shadow_cascade_blend = cascade_blend;
        self
    }

    /// 挂上天空环境照明：`None` = 天空关（片元整段跳过 SH 求值）。
    pub(crate) fn with_sky(mut self, sky_irradiance: Option<[[f32; 4]; 9]>) -> Self {
        self.sky_irradiance = sky_irradiance;
        self
    }

    /// 按 WGSL `Frame` 的字段顺序打包；不足 16 字节的部分用 0 补齐。
    fn write(&self, view_proj: Mat4, bytes: &mut Vec<u8>) {
        for value in view_proj {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.eye[0], self.eye[1], self.eye[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.ambient[0], self.ambient[1], self.ambient[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let sky = self.sky_irradiance.unwrap_or([[0.0; 4]; 9]);
        for coefficient in sky {
            for value in coefficient {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let enabled: f32 = if self.sky_irradiance.is_some() {
            1.0
        } else {
            0.0
        };
        for value in [enabled, 0.0, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let cascades = self.shadow_cascades;
        for cascade in 0..SHADOW_CASCADES {
            let matrix = cascades
                .map(|c| c.view_proj[cascade])
                .unwrap_or(super::camera::IDENTITY);
            for value in matrix {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let mut splits = [0.0f32; SHADOW_CASCADES];
        let mut forward = [0.0f32; 3];
        let mut count = 0.0f32;
        if let Some(cascades) = cascades {
            splits = cascades.splits;
            forward = cascades.forward;
            count = cascades.count as f32;
        }
        for value in splits {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [forward[0], forward[1], forward[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let (shadow_enabled, shadow_bias) = match cascades {
            Some(_) => (1.0, super::shadow::SHADOW_DEPTH_BIAS),
            None => (0.0, 0.0),
        };
        for value in [shadow_enabled, shadow_bias, 0.0, count] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        // x = 软阴影尺寸，y = 级间混合带宽；z / w 留给以后（显式写 0 而不是留空洞）。
        for value in [self.shadow_softness, self.shadow_cascade_blend, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        // x = 有效光源数；y / z / w 预留（显式写 0，保持 16 字节对齐）。
        for value in [self.light_count as f32, 0.0, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for light in &self.lights {
            light.write(bytes);
        }
    }
}

pub(crate) fn create_frame_layout(context: &GpuContext, label: &str) -> wgpu::BindGroupLayout {
    context
        .device()
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(FRAME_UNIFORM_BYTES),
                    },
                    count: None,
                },
                // 阴影深度图：硬件比较采样（比较采样器配 textureSampleCompareLevel）。
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        })
}

pub(crate) fn create_frame_bind(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    view_proj: Mat4,
    lighting: &FrameLighting,
    shadow_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let mut bytes = Vec::with_capacity(FRAME_UNIFORM_BYTES as usize);
    lighting.write(view_proj, &mut bytes);
    debug_assert_eq!(bytes.len(), FRAME_UNIFORM_BYTES as usize);
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("xgc-scene-frame-buffer"),
        size: FRAME_UNIFORM_BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    context.queue().write_buffer(&buffer, 0, &bytes);
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-scene-frame-bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(shadow_sampler),
                },
            ],
        })
}

pub struct InstancedPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_layout: wgpu::BindGroupLayout,
    instance_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

impl InstancedPipeline {
    pub(crate) fn raw(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub(crate) fn camera_layout(&self) -> &wgpu::BindGroupLayout {
        &self.camera_layout
    }

    pub(crate) fn instance_layout(&self) -> &wgpu::BindGroupLayout {
        &self.instance_layout
    }
}

const INSTANCED_WGSL: &str = concat!(
    include_str!("scene_shading.wgsl"),
    include_str!("instanced.wgsl")
);

impl GpuContext {
    pub fn create_instanced_pipeline(
        &self,
        format: wgpu::TextureFormat,
        depth: bool,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> InstancedPipeline {
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-instanced"),
                source: wgpu::ShaderSource::Wgsl(INSTANCED_WGSL.into()),
            });
        let camera_layout = create_frame_layout(self, "xgc-instanced-camera");
        let instance_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-instanced-models"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            // 模型矩阵 64B + 颜色 16B + 材质（metallic / roughness）16B。
                            min_binding_size: wgpu::BufferSize::new(96),
                        },
                        count: None,
                    }],
                });
        let pipeline_layout =
            self.device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xgc-instanced-layout"),
                    bind_group_layouts: &[&camera_layout, &instance_layout, texture_layout],
                    push_constant_ranges: &[],
                });
        let pipeline = self
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-instanced-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[
                        wgpu::VertexBufferLayout {
                            array_stride: 12,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: 8,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![1 => Float32x2],
                        },
                    ],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth.then_some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
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
        InstancedPipeline {
            pipeline,
            camera_layout,
            instance_layout,
            format,
        }
    }
}

struct Group {
    key: String,
    source: String,
    pose: Option<Vec<[f32; 16]>>,
    /// (模型矩阵, 颜色, 材质参数 [metallic, roughness])。
    instances: Vec<(Mat4, [f32; 4], [f32; 2])>,
}

struct Prepared {
    mesh_key: String,
    instance_bind: wgpu::BindGroup,
    count: u32,
}

pub(crate) struct InstancedRequest<'a> {
    pub(crate) texture: &'a wgpu::Texture,
    pub(crate) list: &'a DrawList,
    pub(crate) meshes: &'a mut MeshCache,
    pub(crate) gpu_meshes: &'a mut GpuMeshCache,
    pub(crate) view_proj: Mat4,
    pub(crate) lighting: FrameLighting,
    pub(crate) shadow_view: &'a wgpu::TextureView,
    pub(crate) shadow_sampler: &'a wgpu::Sampler,
    pub(crate) depth: &'a wgpu::TextureView,
    pub(crate) clear_color: wgpu::Color,
    pub(crate) textures: &'a TextureBinding,
}

impl RenderCache {
    /// 按模型分组实例化绘制；返回实际 draw call 数。
    pub(crate) fn draw_list_instanced(
        &mut self,
        context: &GpuContext,
        request: &mut InstancedRequest<'_>,
    ) -> Result<usize, GpuError> {
        let texture = request.texture;
        let list = request.list;
        let meshes = &mut *request.meshes;
        let gpu_meshes = &mut *request.gpu_meshes;
        let view_proj = request.view_proj;
        let depth = request.depth;
        let textures = request.textures;
        if list.items.is_empty() {
            context.clear_texture(texture, request.clear_color)?;
            return Ok(0);
        }
        let format = texture.format();
        if self
            .instanced
            .as_ref()
            .is_none_or(|(current, _)| *current != format)
        {
            self.instanced = Some((
                format,
                context.create_instanced_pipeline(format, true, textures.layout()),
            ));
        }
        let pipeline = &self.instanced.as_ref().unwrap().1;
        if pipeline.format != format {
            return Err(GpuError::Pipeline("实例化管线格式不一致".to_string()));
        }

        let mut groups: HashMap<String, usize> = HashMap::new();
        let mut buckets = Vec::new();
        for item in &list.items {
            let key = if item.pose.is_some() {
                format!("skin:{}:{}", item.entity.raw(), item.model)
            } else {
                item.model.clone()
            };
            let index = *groups.entry(key.clone()).or_insert_with(|| {
                buckets.push(Group {
                    key,
                    source: item.model.clone(),
                    pose: item.pose.clone(),
                    instances: Vec::new(),
                });
                buckets.len() - 1
            });
            buckets[index].instances.push((
                item.transform,
                item.color,
                [item.metallic, item.roughness],
            ));
        }

        let camera_bind = create_frame_bind(
            context,
            pipeline.camera_layout(),
            view_proj,
            &request.lighting,
            request.shadow_view,
            request.shadow_sampler,
        );

        let mut prepared = Vec::with_capacity(buckets.len());
        for group in &buckets {
            let loaded = meshes
                .get(&group.source)
                .map_err(|err| GpuError::Mesh(err.to_string()))?;
            let fallback = loaded.is_none().then(Mesh::fullscreen_triangle);
            let mesh = loaded.unwrap_or_else(|| fallback.as_ref().unwrap());
            if mesh.positions.is_empty() {
                continue;
            }
            if let Some(pose) = &group.pose {
                let positions = super::skinning::skin_positions(mesh, pose);
                gpu_meshes.upload_deformed(context, &group.key, mesh, &positions, textures);
            } else {
                gpu_meshes.upload(context, &group.key, mesh, request.textures);
            }
            let mut bytes = Vec::with_capacity(group.instances.len() * 96);
            for (model, color, material) in &group.instances {
                for value in model {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                for value in color {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                for value in [material[0], material[1], 0.0, 0.0] {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            let instances = context.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("xgc-instanced-models-buffer"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context.queue().write_buffer(&instances, 0, &bytes);
            let instance_bind = context
                .device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xgc-instanced-models-bind"),
                    layout: pipeline.instance_layout(),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instances.as_entire_binding(),
                    }],
                });
            prepared.push(Prepared {
                mesh_key: group.key.clone(),
                instance_bind,
                count: group.instances.len() as u32,
            });
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-instanced-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-instanced-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(request.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline.raw());
            pass.set_bind_group(0, &camera_bind, &[]);
            for draw in &prepared {
                pass.set_bind_group(1, &draw.instance_bind, &[]);
                let Some(mesh) = gpu_meshes.get(&draw.mesh_key) else {
                    continue;
                };
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, mesh.uv_buffer.slice(..));
                pass.set_bind_group(2, &mesh.texture_group, &[]);
                if let Some(indices) = &mesh.index_buffer {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.indices, 0, 0..draw.count);
                } else {
                    pass.draw(0..mesh.vertices, 0..draw.count);
                }
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(prepared.len())
    }
}
