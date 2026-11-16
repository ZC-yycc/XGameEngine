//! 实例化绘制：按网格分组，一次 draw 覆盖同模型的多个实体。

use std::collections::HashMap;

use super::assets::MeshCache;
use super::camera::Mat4;
use super::draw::DrawList;
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::mesh::Mesh;
use super::pipeline::RenderCache;

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

const INSTANCED_WGSL: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
};

@group(1) @binding(0)
var<storage, read> instances: array<Instance>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@location(0) position: vec3<f32>, @builtin(instance_index) index: u32) -> VsOut {
    let instance = instances[index];
    var out: VsOut;
    out.position = camera.view_proj * instance.model * vec4<f32>(position, 1.0);
    out.color = instance.color;
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

impl GpuContext {
    pub fn create_instanced_pipeline(
        &self,
        format: wgpu::TextureFormat,
        depth: bool,
    ) -> InstancedPipeline {
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-instanced"),
                source: wgpu::ShaderSource::Wgsl(INSTANCED_WGSL.into()),
            });
        let camera_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-instanced-camera"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    }],
                });
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
                            min_binding_size: wgpu::BufferSize::new(80),
                        },
                        count: None,
                    }],
                });
        let pipeline_layout =
            self.device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xgc-instanced-layout"),
                    bind_group_layouts: &[&camera_layout, &instance_layout],
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
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }],
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
    instances: Vec<(Mat4, [f32; 4])>,
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
    pub(crate) depth: &'a wgpu::TextureView,
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
        if list.items.is_empty() {
            context.clear_texture(texture, wgpu::Color::BLACK)?;
            return Ok(0);
        }
        let format = texture.format();
        if self
            .instanced
            .as_ref()
            .is_none_or(|(current, _)| *current != format)
        {
            self.instanced = Some((format, context.create_instanced_pipeline(format, true)));
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
            buckets[index].instances.push((item.transform, item.color));
        }

        let mut camera_bytes = Vec::with_capacity(64);
        for value in view_proj {
            camera_bytes.extend_from_slice(&value.to_le_bytes());
        }
        let camera = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-instanced-camera-buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&camera, 0, &camera_bytes);
        let camera_bind = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-instanced-camera-bind"),
                layout: pipeline.camera_layout(),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                }],
            });

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
                gpu_meshes.upload_deformed(context, &group.key, &positions, &mesh.indices);
            } else {
                gpu_meshes.upload(context, &group.key, mesh);
            }
            let mut bytes = Vec::with_capacity(group.instances.len() * 80);
            for (model, color) in &group.instances {
                for value in model {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                for value in color {
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
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
