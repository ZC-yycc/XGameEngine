//! 实例化绘制：按网格分组，一次 draw 覆盖同模型的多个实体。

use std::collections::HashMap;

use super::assets::MeshCache;
use super::camera::Mat4;
use super::draw::DrawList;
use super::frame_uniform::{create_frame_bind, create_frame_layout, FrameLighting, FrameTextures};
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::mesh::Mesh;
use super::pipeline::RenderCache;
use super::texture_bind::TextureBinding;

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
    include_str!("sky_common.wgsl"),
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
                        // 法线流：模型没导法线时缓冲里是全零（片元退回导数叉积）。
                        wgpu::VertexBufferLayout {
                            array_stride: 12,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![2 => Float32x3],
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
    pub(crate) point_shadow_view: &'a wgpu::TextureView,
    /// 预过滤环境立方体 / BRDF 表 / 线性采样器（天空关时也绑着，着色端按开关跳过）。
    pub(crate) env_cube: &'a wgpu::TextureView,
    pub(crate) brdf_lut: &'a wgpu::TextureView,
    pub(crate) env_sampler: &'a wgpu::Sampler,
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
            FrameTextures {
                shadow: request.shadow_view,
                shadow_sampler: request.shadow_sampler,
                point_shadow: request.point_shadow_view,
                env_cube: request.env_cube,
                brdf_lut: request.brdf_lut,
                env_sampler: request.env_sampler,
            },
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
                pass.set_vertex_buffer(2, mesh.normal_buffer.slice(..));
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
