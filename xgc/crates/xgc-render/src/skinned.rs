//! GPU 蒙皮：逐顶点关节索引 / 权重进入顶点着色器，关节矩阵走 uniform。
//!
//! 单 draw 独立管线；深度附件可选，先把数据通路与着色器跑通，再接入实例化帧。

use std::collections::HashMap;

use super::camera::{mul, Mat4, IDENTITY};
use super::gpu::{GpuContext, GpuError};
use super::mesh::Mesh;

const MAX_JOINTS: usize = 64;

const SKINNED_WGSL: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
};

struct Joints {
    matrices: array<mat4x4<f32>, 64>,
};

struct Model {
    model: mat4x4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;
@group(1) @binding(0)
var<uniform> joints: Joints;
@group(2) @binding(0)
var<uniform> model: Model;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) joints_idx: vec4<u32>,
    @location(2) weights: vec4<f32>,
) -> VsOut {
    var skinned = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var i = 0u; i < 4u; i = i + 1u) {
        let matrix = joints.matrices[joints_idx[i]];
        skinned = skinned + (matrix * vec4<f32>(position, 1.0)) * weights[i];
    }
    var out: VsOut;
    out.position = camera.view_proj * model.model * skinned;
    out.color = model.color;
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

pub struct GpuSkinMesh {
    pub position: wgpu::Buffer,
    pub joints: wgpu::Buffer,
    pub weights: wgpu::Buffer,
    pub index_buffer: Option<wgpu::Buffer>,
    pub vertices: u32,
    pub indices: u32,
}

#[derive(Default)]
pub struct GpuSkinMeshCache {
    meshes: HashMap<String, GpuSkinMesh>,
}

impl GpuSkinMeshCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    /// 蒙皮顶点属性只上传一次；同一模型键复用。
    pub fn upload(&mut self, context: &GpuContext, key: &str, mesh: &Mesh) -> &GpuSkinMesh {
        if !self.meshes.contains_key(key) {
            let skin = mesh.skin.as_ref();
            let mut positions = Vec::with_capacity(mesh.positions.len() * 12);
            let mut joints = Vec::with_capacity(mesh.positions.len() * 8);
            let mut weights = Vec::with_capacity(mesh.positions.len() * 16);
            for (index, position) in mesh.positions.iter().enumerate() {
                for value in position {
                    positions.extend_from_slice(&value.to_le_bytes());
                }
                let joint = skin
                    .and_then(|skin| skin.joints.get(index))
                    .copied()
                    .unwrap_or([0; 4]);
                for value in joint {
                    joints.extend_from_slice(&value.to_le_bytes());
                }
                let weight = skin
                    .and_then(|skin| skin.weights.get(index))
                    .copied()
                    .unwrap_or([1.0, 0.0, 0.0, 0.0]);
                for value in weight {
                    weights.extend_from_slice(&value.to_le_bytes());
                }
            }
            let position = create_buffer(context, "xgc-skin-positions", &positions, false);
            let joints = create_buffer(context, "xgc-skin-joints", &joints, false);
            let weights = create_buffer(context, "xgc-skin-weights", &weights, false);
            let index_buffer = if mesh.indices.is_empty() {
                None
            } else {
                let bytes: Vec<u8> = mesh
                    .indices
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect();
                Some(create_buffer(context, "xgc-skin-indices", &bytes, true))
            };
            self.meshes.insert(
                key.to_string(),
                GpuSkinMesh {
                    position,
                    joints,
                    weights,
                    index_buffer,
                    vertices: mesh.positions.len() as u32,
                    indices: mesh.indices.len() as u32,
                },
            );
        }
        self.meshes.get(key).unwrap()
    }

    pub fn get(&self, key: &str) -> Option<&GpuSkinMesh> {
        self.meshes.get(key)
    }
}

fn create_buffer(context: &GpuContext, label: &str, bytes: &[u8], index: bool) -> wgpu::Buffer {
    let usage = if index {
        wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST
    } else {
        wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST
    };
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len().max(4) as u64,
        usage,
        mapped_at_creation: false,
    });
    if !bytes.is_empty() {
        context.queue().write_buffer(&buffer, 0, bytes);
    }
    buffer
}

pub struct SkinnedPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_layout: wgpu::BindGroupLayout,
    joints_layout: wgpu::BindGroupLayout,
    model_layout: wgpu::BindGroupLayout,
}

impl GpuContext {
    pub fn create_skinned_pipeline(&self, format: wgpu::TextureFormat) -> SkinnedPipeline {
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-skinned"),
                source: wgpu::ShaderSource::Wgsl(SKINNED_WGSL.into()),
            });
        let uniform = |size: u64, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let camera_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-skin-camera"),
                    entries: &[uniform(64, wgpu::ShaderStages::VERTEX)],
                });
        let joints_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-skin-joints"),
                    entries: &[uniform(
                        (MAX_JOINTS * 64) as u64,
                        wgpu::ShaderStages::VERTEX,
                    )],
                });
        let model_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-skin-model"),
                    entries: &[uniform(80, wgpu::ShaderStages::VERTEX_FRAGMENT)],
                });
        let layout = self
            .device()
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xgc-skin-layout"),
                bind_group_layouts: &[&camera_layout, &joints_layout, &model_layout],
                push_constant_ranges: &[],
            });
        let pipeline = self
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-skin-pipeline"),
                layout: Some(&layout),
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
                            attributes: &wgpu::vertex_attr_array![1 => Uint16x4],
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: 16,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![2 => Float32x4],
                        },
                    ],
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
        SkinnedPipeline {
            pipeline,
            camera_layout,
            joints_layout,
            model_layout,
        }
    }
}

pub struct SkinnedRequest<'a> {
    pub texture: &'a wgpu::Texture,
    pub mesh: &'a GpuSkinMesh,
    pub pose: &'a [[f32; 16]],
    pub inverse_bind: &'a [[f32; 16]],
    pub model: Mat4,
    pub color: [f32; 4],
    pub view_proj: Mat4,
}

impl SkinnedPipeline {
    /// 画一个蒙皮网格；关节数超过 64 时只取前 64。
    pub fn draw(&self, context: &GpuContext, request: &SkinnedRequest<'_>) -> Result<(), GpuError> {
        let mut camera_bytes = Vec::with_capacity(64);
        for value in request.view_proj {
            camera_bytes.extend_from_slice(&value.to_le_bytes());
        }
        let camera = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-skin-camera-buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&camera, 0, &camera_bytes);

        let mut joints_bytes = vec![0u8; MAX_JOINTS * 64];
        for index in 0..MAX_JOINTS {
            let matrix = match (request.pose.get(index), request.inverse_bind.get(index)) {
                (Some(pose), Some(inverse)) => mul(*pose, *inverse),
                (Some(pose), None) => *pose,
                _ => IDENTITY,
            };
            for (slot, value) in matrix.iter().enumerate() {
                let offset = index * 64 + slot * 4;
                joints_bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        let joints = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-skin-joints-buffer"),
            size: (MAX_JOINTS * 64) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&joints, 0, &joints_bytes);

        let mut model_bytes = Vec::with_capacity(80);
        for value in request.model {
            model_bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in request.color {
            model_bytes.extend_from_slice(&value.to_le_bytes());
        }
        let model = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-skin-model-buffer"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&model, 0, &model_bytes);

        let camera_bind = self.camera_bind(context, &camera);
        let joints_bind = self.joints_bind(context, &joints);
        let model_bind = self.model_bind(context, &model);
        let view = request
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-skin-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-skin-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &camera_bind, &[]);
            pass.set_bind_group(1, &joints_bind, &[]);
            pass.set_bind_group(2, &model_bind, &[]);
            pass.set_vertex_buffer(0, request.mesh.position.slice(..));
            pass.set_vertex_buffer(1, request.mesh.joints.slice(..));
            pass.set_vertex_buffer(2, request.mesh.weights.slice(..));
            if let Some(indices) = &request.mesh.index_buffer {
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..request.mesh.indices, 0, 0..1);
            } else {
                pass.draw(0..request.mesh.vertices, 0..1);
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(())
    }

    fn camera_bind(&self, context: &GpuContext, buffer: &wgpu::Buffer) -> wgpu::BindGroup {
        context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-skin-camera-bind"),
                layout: &self.camera_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            })
    }

    fn joints_bind(&self, context: &GpuContext, buffer: &wgpu::Buffer) -> wgpu::BindGroup {
        context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-skin-joints-bind"),
                layout: &self.joints_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            })
    }

    fn model_bind(&self, context: &GpuContext, buffer: &wgpu::Buffer) -> wgpu::BindGroup {
        context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-skin-model-bind"),
                layout: &self.model_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            })
    }
}
