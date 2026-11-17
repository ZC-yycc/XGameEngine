//! GPU 蒙皮：关节矩阵只负责顶点变形，材质 / 贴图 / PBR / ACES 与静态主场景共用。

use std::collections::HashMap;

use super::camera::{mul, Mat4, IDENTITY};
use super::gpu::{GpuContext, GpuError};
use super::instanced::create_frame_layout;
use super::mesh::Mesh;
use super::texture_bind::TextureBinding;

/// palette 槽数：与旧引擎 `BONES_PER_SLOT = 256` 对齐，超出才回退 CPU。
///
/// 256 × 64B = 16 KiB 的 uniform 仍在 wgpu 默认 `max_uniform_buffer_binding_size`
/// （64 KiB）之内；代价是每个蒙皮 draw 每帧要传 16 KiB palette，换来的是
/// "65~256 关节不再静默掉到 CPU 慢路径"（旧端同容量）。
const MAX_JOINTS: usize = 256;
/// MAX_JOINTS × mat4x4<f32>：palette uniform 的字节数（主场景与阴影 pass 共用）。
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

/// 设备能否绑定 256 槽 palette（16 KiB）。
///
/// downlevel GL 路径（egui 窗口在 GL 后端下发的是 `downlevel_webgl2_defaults`）的
/// `max_uniform_buffer_binding_size` **正好是 16 KiB**：等于上限仍然合法，但零余量。
/// 上限更小的设备必须在路由阶段就退回 CPU，而不是等 pipeline 创建校验失败。
pub(crate) fn palette_fits_device(max_uniform_binding: u64) -> bool {
    max_uniform_binding >= MAX_JOINTS_BYTES as u64
}

/// 当前姿态的保守局部包围盒（尚未乘实体 model）。
///
/// 两桶求并：
/// 1. **会动的顶点**：按顶点扫出"真的有 palette 项"的关节下标，再对每根关节把整网格
///    绑定 AABB 变换一次。逐顶点蒙皮结果是这些变换点的凸组合，而并集的 AABB 覆盖整条
///    凸包，所以盒子一定盖住它们；同一关节的盒对整张网格是常量，因此只需
///    O(顶点 + 关节) 次变换。旧端在加载期按关节预计算**顶点范围**盒，这里用整网格
///    AABB 近似，更松（骨架场景的自动取景会比旧端略远），但实现代价更低。
/// 2. **不动的顶点**：缺 JOINTS_0 / WEIGHTS_0、权重全零、或关节下标超出 pose /
///    逆绑定的顶点，`skin_positions` 与 shader 都让它们留在绑定姿态，这里按顶点
///    并进盒子——漏掉这一桶会让自动取景把这类顶点裁出画面。
///
/// 没有蒙皮数据 / 姿态为空时退回绑定姿态盒子；`None` 只表示网格没有顶点。
pub(crate) fn posed_bounds(mesh: &Mesh, pose: &[[f32; 16]]) -> Option<super::shot::Bounds> {
    let local = super::shot::Bounds::from_mesh(mesh)?;
    let Some(skin) = &mesh.skin else {
        return Some(local);
    };
    if pose.is_empty() || skin.inverse_bind.is_empty() {
        return Some(local);
    }
    let slots = pose.len().min(skin.inverse_bind.len());
    let mut used = vec![false; slots];
    let mut kept = super::shot::Bounds::EMPTY;
    for (index, position) in mesh.positions.iter().enumerate() {
        let (Some(joints), Some(weights)) = (skin.joints.get(index), skin.weights.get(index))
        else {
            kept.union_point(*position);
            continue;
        };
        let mut skinned = false;
        for slot in 0..4 {
            if weights[slot] <= 0.0 {
                continue;
            }
            let joint = usize::from(joints[slot]);
            if joint < slots {
                used[joint] = true;
                skinned = true;
            }
        }
        if !skinned {
            kept.union_point(*position);
        }
    }
    let mut out = kept;
    for (joint, flag) in used.iter().enumerate() {
        if !flag {
            continue;
        }
        out.union(&local.transformed(mul(pose[joint], skin.inverse_bind[joint])));
    }
    Some(if out.is_empty() { local } else { out })
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

/// 一批蒙皮 draw 里的一条：网格、palette 字节、模型矩阵与材质参数。
pub(crate) struct SkinnedDraw<'a> {
    pub(crate) mesh: &'a GpuSkinMesh,
    /// `palette_bytes` 的结果（256 槽 × mat4）；每帧每角色只算一次。
    pub(crate) palette: &'a [u8],
    pub(crate) model: Mat4,
    pub(crate) color: [f32; 4],
    pub(crate) metallic: f32,
    pub(crate) roughness: f32,
}

/// 一批蒙皮 draw 的公共参数：目标、深度、清屏与否、本帧共用的一次 frame binding。
pub(crate) struct SkinnedBatchRequest<'a> {
    pub(crate) texture: &'a wgpu::Texture,
    pub(crate) depth: &'a wgpu::TextureView,
    pub(crate) clear: bool,
    pub(crate) clear_color: wgpu::Color,
    pub(crate) frame_bind: &'a wgpu::BindGroup,
    pub(crate) draws: &'a [SkinnedDraw<'a>],
}

impl SkinnedPipeline {
    /// 一批蒙皮 draw 共用**一个** frame binding、**一个** pass、**一次** submit。
    ///
    /// 旧端也是这样：palette 逐角色，提交不逐角色。此前 XGC 每条蒙皮 draw 各建
    /// 一次 frame bind / encoder / submit，多角色场景下提交次数随角色数线性增长。
    ///
    /// `clear` 只有"本帧还没有任何 draw 写进这张目标"时才为真（与实例化 pass 同款），
    /// 否则用 `LoadOp::Load` 接在前面写好的颜色 / 深度上。
    pub(crate) fn draw_batch(
        &self,
        context: &GpuContext,
        request: &SkinnedBatchRequest<'_>,
    ) -> Result<usize, GpuError> {
        let draws = request.draws;
        if draws.is_empty() {
            return Ok(0);
        }
        // palette / model 逐条建绑定，但都在进 pass 之前建好（pass 只借用）。
        let mut bindings = Vec::with_capacity(draws.len());
        for draw in draws {
            let palette = create_uniform_buffer(context, "xgc-skin-joints-buffer", draw.palette);
            let palette_bind = uniform_bind(
                context,
                &self.joints_layout,
                &palette,
                "xgc-skin-joints-bind",
            );
            let model = model_buffer(context, draw);
            let model_bind =
                uniform_bind(context, &self.model_layout, &model, "xgc-skin-model-bind");
            bindings.push((palette_bind, model_bind));
        }
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
            pass.set_bind_group(0, request.frame_bind, &[]);
            for (draw, (palette_bind, model_bind)) in draws.iter().zip(&bindings) {
                pass.set_bind_group(1, palette_bind, &[]);
                pass.set_bind_group(2, &draw.mesh.texture_group, &[]);
                pass.set_bind_group(3, model_bind, &[]);
                pass.set_vertex_buffer(0, draw.mesh.position.slice(..));
                pass.set_vertex_buffer(1, draw.mesh.uv.slice(..));
                pass.set_vertex_buffer(2, draw.mesh.joints.slice(..));
                pass.set_vertex_buffer(3, draw.mesh.weights.slice(..));
                if let Some(indices) = &draw.mesh.index_buffer {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.mesh.indices, 0, 0..1);
                } else {
                    pass.draw(0..draw.mesh.vertices, 0..1);
                }
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(draws.len())
    }

    pub(crate) fn frame_layout(&self) -> &wgpu::BindGroupLayout {
        &self.frame_layout
    }
}

fn model_buffer(context: &GpuContext, draw: &SkinnedDraw<'_>) -> wgpu::Buffer {
    let mut bytes = Vec::with_capacity(96);
    for value in draw.model {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in draw.color {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [draw.metallic, draw.roughness, 0.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    create_uniform_buffer(context, "xgc-skin-model-buffer", &bytes)
}

/// 256 槽 palette 的唯一计算处：主场景与阴影 pass 都读这一份，`pose × inverse_bind`
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
mod tests;
