//! GPU 蒙皮：关节矩阵只负责顶点变形，材质 / 贴图 / PBR / ACES 与静态主场景共用。

use std::collections::HashMap;

use super::camera::{mul, Mat4, IDENTITY};
use super::frame_uniform::create_frame_layout;
use super::gpu::{GpuContext, GpuError};
use super::mesh::Mesh;
use super::texture_bind::TextureBinding;

pub(crate) mod arena;

/// palette 槽数：与旧引擎 `BONES_PER_SLOT = 256` 对齐，超出才回退 CPU。
///
/// 256 × 64B = 16 KiB 的 uniform 仍在 wgpu 默认 `max_uniform_buffer_binding_size`
/// （64 KiB）之内；代价是每个蒙皮 draw 每帧要传 16 KiB palette，换来的是
/// "65~256 关节不再静默掉到 CPU 慢路径"（旧端同容量）。
const MAX_JOINTS: usize = 256;
/// MAX_JOINTS × mat4x4<f32>：palette uniform 的字节数（主场景与阴影 pass 共用）。
pub(crate) const MAX_JOINTS_BYTES: usize = MAX_JOINTS * 64;
/// 逐 draw 的 model uniform 字节数（模型矩阵 64 B + 颜色 16 B + 材质 16 B）。
///
/// layout 的 `min_binding_size`、竞技场槽距、打包容量三处共用这一份，避免加字段时
/// 只改 layout 而竞技场仍按旧字节数收、在 `upload` 的断言处才炸。
const MODEL_UNIFORM_BYTES: u64 = 96;
const SKINNED_WGSL: &str = concat!(
    include_str!("scene_shading.wgsl"),
    include_str!("skin_vertex.wgsl"),
    include_str!("skinned.wgsl")
);

pub(crate) struct GpuSkinMesh {
    position: wgpu::Buffer,
    uv: wgpu::Buffer,
    /// 每顶点法线（模型空间）；模型没导法线时全零，片元据此退回屏幕导数。
    normal: wgpu::Buffer,
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
            let mut normals = Vec::with_capacity(mesh.positions.len() * 12);
            for (index, position) in mesh.positions.iter().enumerate() {
                for value in position {
                    positions.extend_from_slice(&value.to_le_bytes());
                }
                for value in mesh.uvs.get(index).copied().unwrap_or([0.0; 2]) {
                    uvs.extend_from_slice(&value.to_le_bytes());
                }
                for value in mesh.normals.get(index).copied().unwrap_or([0.0; 3]) {
                    normals.extend_from_slice(&value.to_le_bytes());
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
                    normal: create_buffer(context, "xgc-skin-normals", &normals, false),
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
    /// 逐角色 palette 并进同一张缓冲，靠 dynamic offset 分槽。
    palette: arena::UniformArena,
    model: arena::UniformArena,
    /// 逐 draw model uniform 的打包暂存：跨帧复用，稳态下不再每个角色一次分配。
    model_scratch: Vec<Vec<u8>>,
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
                // 竞技场按槽偏移，layout 必须声明 dynamic offset。
                has_dynamic_offset: true,
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
                    entries: &[uniform(
                        MODEL_UNIFORM_BYTES,
                        wgpu::ShaderStages::VERTEX_FRAGMENT,
                    )],
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
                        // 法线流：模型没导法线时缓冲里是全零（片元退回导数叉积）。
                        wgpu::VertexBufferLayout {
                            array_stride: 12,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![4 => Float32x3],
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
            palette: arena::UniformArena::new(
                self,
                "xgc-skin-joints-arena",
                MAX_JOINTS_BYTES as u64,
            ),
            model: arena::UniformArena::new(self, "xgc-skin-model-arena", MODEL_UNIFORM_BYTES),
            model_scratch: Vec::new(),
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

/// 一批蒙皮 draw 的产物：画了几条、这一帧为 uniform 竞技场新建了几张 GPU 缓冲。
///
/// `uniform_buffers` 只在"palette / model 竞技场扩过容量"时非零；它是"逐角色建
/// 缓冲"这个旧代价是否真的消失的可观测口径（渲染统计里的 `skin_uniform_buffers`）。
#[derive(Default)]
pub(crate) struct SkinnedBatchOutcome {
    pub(crate) draws: usize,
    pub(crate) uniform_buffers: usize,
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
        &mut self,
        context: &GpuContext,
        request: &SkinnedBatchRequest<'_>,
    ) -> Result<SkinnedBatchOutcome, GpuError> {
        let draws = request.draws;
        if draws.is_empty() {
            return Ok(SkinnedBatchOutcome::default());
        }
        // 逐角色 palette / model 并进两张常驻缓冲：整批只写两次、绑定组只在扩容时建，
        // 进 pass 之后每条 draw 只换 dynamic offset（旧实现是逐条建缓冲 + 建绑定组）。
        let palettes: Vec<&[u8]> = draws.iter().map(|draw| draw.palette).collect();
        let mut uniform_buffers = self.palette.upload(context, &self.joints_layout, &palettes);
        // 打包暂存跨帧复用（只增不减，和竞技场同一条取舍）：稳态下这里不分配。
        let mut models = std::mem::take(&mut self.model_scratch);
        while models.len() < draws.len() {
            models.push(Vec::new());
        }
        for (bytes, draw) in models.iter_mut().take(draws.len()).zip(draws.iter()) {
            model_bytes_into(bytes, draw);
        }
        let model_slices: Vec<&[u8]> = models
            .iter()
            .take(draws.len())
            .map(|bytes| bytes.as_slice())
            .collect();
        uniform_buffers += self
            .model
            .upload(context, &self.model_layout, &model_slices);
        drop(model_slices);
        self.model_scratch = models;
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
            for (slot, draw) in draws.iter().enumerate() {
                pass.set_bind_group(1, self.palette.bind(), &[self.palette.offset(slot)]);
                pass.set_bind_group(2, &draw.mesh.texture_group, &[]);
                pass.set_bind_group(3, self.model.bind(), &[self.model.offset(slot)]);
                pass.set_vertex_buffer(0, draw.mesh.position.slice(..));
                pass.set_vertex_buffer(1, draw.mesh.uv.slice(..));
                pass.set_vertex_buffer(4, draw.mesh.normal.slice(..));
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
        Ok(SkinnedBatchOutcome {
            draws: draws.len(),
            uniform_buffers,
        })
    }

    pub(crate) fn frame_layout(&self) -> &wgpu::BindGroupLayout {
        &self.frame_layout
    }
}

/// 一条 draw 的模型 / 材质 uniform 字节（模型矩阵 64 B + 颜色 16 B + 材质 16 B）。
fn model_bytes_into(bytes: &mut Vec<u8>, draw: &SkinnedDraw<'_>) {
    bytes.clear();
    bytes.reserve(MODEL_UNIFORM_BYTES as usize);
    for value in draw.model {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in draw.color {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [draw.metallic, draw.roughness, 0.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
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

#[cfg(test)]
mod tests;
