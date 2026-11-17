//! GPU 蒙皮：关节矩阵只负责顶点变形，材质 / 贴图 / PBR / ACES 与静态主场景共用。

use std::collections::HashMap;

use super::camera::{mul, Mat4, IDENTITY};
use super::gpu::{GpuContext, GpuError};
use super::instanced::{create_frame_bind, create_frame_layout, FrameLighting};
use super::mesh::Mesh;
use super::texture_bind::TextureBinding;

const MAX_JOINTS: usize = 64;
/// 64 × mat4x4<f32>：palette uniform 的字节数（主场景与阴影 pass 共用）。
pub(crate) const MAX_JOINTS_BYTES: usize = MAX_JOINTS * 64;
const SKINNED_WGSL: &str = concat!(
    include_str!("scene_shading.wgsl"),
    include_str!("skin_vertex.wgsl"),
    include_str!("skinned.wgsl")
);

pub(crate) struct GpuSkinMesh {
    position: wgpu::Buffer,
    uv: wgpu::Buffer,
    joints: wgpu::Buffer,
    weights: wgpu::Buffer,
    texture_group: wgpu::BindGroup,
    index_buffer: Option<wgpu::Buffer>,
    vertices: u32,
    indices: u32,
}

impl GpuSkinMesh {
    /// 阴影 pass 需要的三个顶点流：位置 / 关节下标 / 权重（UV 与贴图不参与深度）。
    pub(crate) fn shadow_buffers(&self) -> (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer) {
        (&self.position, &self.joints, &self.weights)
    }

    pub(crate) fn shadow_index_buffer(&self) -> Option<&wgpu::Buffer> {
        self.index_buffer.as_ref()
    }

    pub(crate) fn shadow_vertices(&self) -> u32 {
        self.vertices
    }

    pub(crate) fn shadow_indices(&self) -> u32 {
        self.indices
    }
}

#[derive(Default)]
pub(crate) struct GpuSkinMeshCache {
    meshes: HashMap<String, GpuSkinMesh>,
}

impl GpuSkinMeshCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.meshes.len()
    }

    /// 阴影 pass 只借用已上传的缓冲（顶点属性由主场景的 `upload` 负责）。
    pub(crate) fn get(&self, key: &str) -> Option<&GpuSkinMesh> {
        self.meshes.get(key)
    }

    /// 蒙皮顶点属性、UV 与贴图只上传一次；同一模型键跨实体 / 帧复用。
    pub(crate) fn upload(
        &mut self,
        context: &GpuContext,
        key: &str,
        mesh: &Mesh,
        textures: &TextureBinding,
    ) -> &GpuSkinMesh {
        if !self.meshes.contains_key(key) {
            let skin = mesh.skin.as_ref();
            let mut positions = Vec::with_capacity(mesh.positions.len() * 12);
            let mut uvs = Vec::with_capacity(mesh.positions.len() * 8);
            let mut joints = Vec::with_capacity(mesh.positions.len() * 8);
            let mut weights = Vec::with_capacity(mesh.positions.len() * 16);
            for (index, position) in mesh.positions.iter().enumerate() {
                for value in position {
                    positions.extend_from_slice(&value.to_le_bytes());
                }
                for value in mesh.uvs.get(index).copied().unwrap_or([0.0; 2]) {
                    uvs.extend_from_slice(&value.to_le_bytes());
                }
                for value in skin
                    .and_then(|skin| skin.joints.get(index))
                    .copied()
                    .unwrap_or([0; 4])
                {
                    joints.extend_from_slice(&value.to_le_bytes());
                }
                for value in skin
                    .and_then(|skin| skin.weights.get(index))
                    .copied()
                    .unwrap_or([1.0, 0.0, 0.0, 0.0])
                {
                    weights.extend_from_slice(&value.to_le_bytes());
                }
            }
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
                    position: create_buffer(context, "xgc-skin-positions", &positions, false),
                    uv: create_buffer(context, "xgc-skin-uvs", &uvs, false),
                    joints: create_buffer(context, "xgc-skin-joints", &joints, false),
                    weights: create_buffer(context, "xgc-skin-weights", &weights, false),
                    texture_group: textures.group(context, mesh.texture.as_deref()),
                    index_buffer,
                    vertices: mesh.positions.len() as u32,
                    indices: mesh.indices.len() as u32,
                },
            );
        }
        self.meshes.get(key).unwrap()
    }
}

/// 只有 GPU palette 能完整表达 CPU 蒙皮语义时才走快路径；否则保留确定性的 CPU 回退。
pub(crate) fn supports_gpu_skin(mesh: &Mesh, pose: &[[f32; 16]]) -> bool {
    let Some(skin) = &mesh.skin else {
        return false;
    };
    if mesh.positions.is_empty()
        || pose.is_empty()
        || pose.len() > MAX_JOINTS
        || skin.joints.len() != mesh.positions.len()
        || skin.weights.len() != mesh.positions.len()
        || skin.inverse_bind.is_empty()
    {
        return false;
    }
    skin.joints
        .iter()
        .zip(&skin.weights)
        .all(|(joints, weights)| {
            weights.iter().enumerate().all(|(slot, weight)| {
                weight.is_finite()
                    && (*weight <= 0.0
                        || usize::from(joints[slot]) < pose.len()
                            && usize::from(joints[slot]) < skin.inverse_bind.len()
                            && usize::from(joints[slot]) < MAX_JOINTS)
            })
        })
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

pub(crate) struct SkinnedPipeline {
    pipeline: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    joints_layout: wgpu::BindGroupLayout,
    model_layout: wgpu::BindGroupLayout,
}

impl GpuContext {
    pub(crate) fn create_skinned_pipeline(
        &self,
        format: wgpu::TextureFormat,
        depth: bool,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> SkinnedPipeline {
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
        let frame_layout = create_frame_layout(self, "xgc-skin-frame");
        let joints_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-skin-joints"),
                    entries: &[uniform(MAX_JOINTS_BYTES as u64, wgpu::ShaderStages::VERTEX)],
                });
        let model_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-skin-model"),
                    entries: &[uniform(96, wgpu::ShaderStages::VERTEX_FRAGMENT)],
                });
        let layout = self
            .device()
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("xgc-skin-layout"),
                bind_group_layouts: &[&frame_layout, &joints_layout, texture_layout, &model_layout],
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
                            attributes: &wgpu::vertex_attr_array![1 => Float32x2],
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: 8,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![2 => Uint16x4],
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: 16,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![3 => Float32x4],
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
        SkinnedPipeline {
            pipeline,
            frame_layout,
            joints_layout,
            model_layout,
        }
    }
}

pub(crate) struct SkinnedRequest<'a> {
    pub(crate) texture: &'a wgpu::Texture,
    pub(crate) depth: &'a wgpu::TextureView,
    pub(crate) clear: bool,
    pub(crate) clear_color: wgpu::Color,
    pub(crate) mesh: &'a GpuSkinMesh,
    pub(crate) pose: &'a [[f32; 16]],
    pub(crate) inverse_bind: &'a [[f32; 16]],
    pub(crate) model: Mat4,
    pub(crate) color: [f32; 4],
    pub(crate) metallic: f32,
    pub(crate) roughness: f32,
    pub(crate) view_proj: Mat4,
    pub(crate) lighting: &'a FrameLighting,
    pub(crate) shadow_view: &'a wgpu::TextureView,
    pub(crate) shadow_sampler: &'a wgpu::Sampler,
}

impl SkinnedPipeline {
    pub(crate) fn draw(
        &self,
        context: &GpuContext,
        request: &SkinnedRequest<'_>,
    ) -> Result<(), GpuError> {
        let frame_bind = create_frame_bind(
            context,
            &self.frame_layout,
            request.view_proj,
            request.lighting,
            request.shadow_view,
            request.shadow_sampler,
        );
        let joints = self.joints_buffer(context, request.pose, request.inverse_bind);
        let joints_bind = uniform_bind(
            context,
            &self.joints_layout,
            &joints,
            "xgc-skin-joints-bind",
        );
        let model = model_buffer(context, request);
        let model_bind = uniform_bind(context, &self.model_layout, &model, "xgc-skin-model-bind");
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
                        load: if request.clear {
                            wgpu::LoadOp::Clear(request.clear_color)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: request.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if request.clear {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &frame_bind, &[]);
            pass.set_bind_group(1, &joints_bind, &[]);
            pass.set_bind_group(2, &request.mesh.texture_group, &[]);
            pass.set_bind_group(3, &model_bind, &[]);
            pass.set_vertex_buffer(0, request.mesh.position.slice(..));
            pass.set_vertex_buffer(1, request.mesh.uv.slice(..));
            pass.set_vertex_buffer(2, request.mesh.joints.slice(..));
            pass.set_vertex_buffer(3, request.mesh.weights.slice(..));
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

    fn joints_buffer(
        &self,
        context: &GpuContext,
        pose: &[[f32; 16]],
        inverse_bind: &[[f32; 16]],
    ) -> wgpu::Buffer {
        create_uniform_buffer(
            context,
            "xgc-skin-joints-buffer",
            &palette_bytes(pose, inverse_bind),
        )
    }
}

fn model_buffer(context: &GpuContext, request: &SkinnedRequest<'_>) -> wgpu::Buffer {
    let mut bytes = Vec::with_capacity(96);
    for value in request.model {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in request.color {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [request.metallic, request.roughness, 0.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    create_uniform_buffer(context, "xgc-skin-model-buffer", &bytes)
}

/// 64 槽 palette 的唯一计算处：主场景与阴影 pass 都读这一份，`pose × inverse_bind`
/// 逐槽；缺失的槽用单位矩阵（与 shader 里"没有权重就保持原位"同一口径）。
pub(crate) fn palette_bytes(pose: &[[f32; 16]], inverse_bind: &[[f32; 16]]) -> Vec<u8> {
    let mut bytes = vec![0u8; MAX_JOINTS_BYTES];
    for index in 0..MAX_JOINTS {
        let matrix = match (pose.get(index), inverse_bind.get(index)) {
            (Some(pose), Some(inverse)) => mul(*pose, *inverse),
            _ => IDENTITY,
        };
        for (slot, value) in matrix.iter().enumerate() {
            let offset = index * 64 + slot * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn create_uniform_buffer(context: &GpuContext, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    context.queue().write_buffer(&buffer, 0, bytes);
    buffer
}

fn uniform_bind(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    label: &str,
) -> wgpu::BindGroup {
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
}

#[cfg(test)]
mod tests {
    use super::supports_gpu_skin;
    use crate::{identity, Mesh, SkinData};

    fn mesh(joint: u16) -> Mesh {
        let mut mesh = Mesh::new(vec![[0.0; 3]], Vec::new());
        mesh.skin = Some(SkinData {
            joints: vec![[joint, 0, 0, 0]],
            weights: vec![[1.0, 0.0, 0.0, 0.0]],
            joint_nodes: vec![0],
            inverse_bind: vec![identity()],
        });
        mesh
    }

    #[test]
    fn gpu_skin_requires_a_fully_representable_palette() {
        assert!(supports_gpu_skin(&mesh(0), &[identity()]));
        let mut empty = mesh(0);
        empty.positions.clear();
        empty.skin.as_mut().unwrap().joints.clear();
        empty.skin.as_mut().unwrap().weights.clear();
        assert!(!supports_gpu_skin(&empty, &[identity()]));
        assert!(!supports_gpu_skin(&mesh(1), &[identity()]));
        assert!(!supports_gpu_skin(&mesh(0), &vec![identity(); 65]));
        let mut missing_inverse = mesh(0);
        missing_inverse.skin.as_mut().unwrap().inverse_bind.clear();
        assert!(!supports_gpu_skin(&missing_inverse, &[identity()]));
    }
}
