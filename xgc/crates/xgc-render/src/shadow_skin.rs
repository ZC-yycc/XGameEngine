//! 阴影 pass 的蒙皮投射体：GPU palette 变形 + CPU 回退的已变形顶点。
//!
//! 旧引擎里蒙皮网格和静态网格一样进阴影批次（同一支 depth-only 顶点入口按
//! `skin_slot` 分支），所以 XGC 也要求"看得见的蒙皮表面会投影"。变形公式来自
//! `skin_vertex.wgsl`、palette 来自 `skinned::palette_bytes`，都与主场景同一份；
//! 这里只管阴影用的资源与绘制。

use super::assets::MeshCache;
use super::camera::Mat4;
use super::draw::DrawItem;
use super::gpu::GpuContext;
use super::gpu_mesh::GpuMeshCache;
use super::skinned::{self, arena::UniformArena, GpuSkinMeshCache, MAX_JOINTS_BYTES};
use super::texture_bind::TextureBinding;

const SHADOW_SKIN_WGSL: &str = concat!(
    include_str!("skin_vertex.wgsl"),
    include_str!("shadow_skin.wgsl")
);

/// 阴影投射体只需要模型矩阵，比主场景的实例 uniform（96 B）小。
const SHADOW_MODEL_UNIFORM_BYTES: u64 = 64;

/// 一个阴影投射体：`gpu_meshes` 里已就绪的网格键 + 模型矩阵。
///
/// `palette` 有值 = 这一条走 GPU 蒙皮管线；否则按实例化顶点流画（静态网格与
/// CPU 回退的已变形顶点都在这一类）。
pub(super) struct Caster {
    pub(super) mesh_key: String,
    pub(super) model: Mat4,
    palette: Option<Vec<u8>>,
}

impl Caster {
    /// GPU 蒙皮投射体由 palette 管线画，实例化那支要跳过它们。
    pub(super) fn skins_here(&self) -> bool {
        self.palette.is_some()
    }
}

/// 每帧每个蒙皮投射体一个槽位（4 级级联共用同一批绑定，不再逐级重建）。
///
/// 只记槽号不记绑定组：palette / model 都并进常驻竞技场，绑定组每帧只有一个，
/// 逐投射体变的只是 dynamic offset（与主场景 `SkinnedPipeline` 同一套做法）。
pub(super) struct SkinnedBinding {
    mesh_key: String,
    slot: usize,
}

pub(super) struct SkinnedShadow {
    pipeline: wgpu::RenderPipeline,
    palette_layout: wgpu::BindGroupLayout,
    model_layout: wgpu::BindGroupLayout,
    palette: UniformArena,
    model: UniformArena,
    /// 逐 caster 的 model 字节打包暂存：跨帧复用，避免每帧 N 个小 `Vec` 分配。
    model_scratch: Vec<Vec<u8>>,
}

/// 阴影 pass 要用的网格来源：三份缓存 + 贴图绑定。打成一个结构避免 `render`
/// 参数爆表，也让"哪些缓存是投影必需的"在一处写清。
pub(super) struct CasterSources<'a> {
    pub(super) meshes: &'a mut MeshCache,
    pub(super) gpu_meshes: &'a mut GpuMeshCache,
    pub(super) skin_meshes: &'a mut GpuSkinMeshCache,
    pub(super) textures: &'a TextureBinding,
}

/// 把 draw list 解析成阴影投射体：静态网格原样上传，CPU 回退蒙皮在这里先算一遍
/// 变形顶点（阴影 pass 排在主 pass 之前，不能等主 pass 上传）。
pub(super) fn prepare(
    context: &GpuContext,
    items: &[DrawItem],
    sources: &mut CasterSources<'_>,
) -> Result<Vec<Caster>, String> {
    let mut casters = Vec::with_capacity(items.len());
    for item in items {
        let Some(mesh) = sources
            .meshes
            .get(&item.model)
            .map_err(|err| err.to_string())?
        else {
            continue;
        };
        if mesh.positions.is_empty() {
            continue;
        }
        let (mesh_key, palette) = match item.pose.as_deref() {
            Some(pose)
                if context.supports_full_skin_palette()
                    && skinned::supports_gpu_skin(mesh, pose) =>
            {
                // 顶点属性走主场景同一份缓存；阴影只借用它的缓冲，不改写。
                sources
                    .skin_meshes
                    .upload(context, &item.model, mesh, sources.textures);
                let inverse_bind = mesh
                    .skin
                    .as_ref()
                    .map(|skin| skin.inverse_bind.as_slice())
                    .unwrap_or(&[]);
                (
                    item.model.clone(),
                    Some(skinned::palette_bytes(pose, inverse_bind)),
                )
            }
            Some(pose) => {
                // >256 关节 / palette 不完整：退回 CPU 变形，与主 pass 同一套函数。
                // key 与主通道（`instanced.rs` 的 `skin:实体:模型`）相同：同一份变形
                // 顶点只占一份缓存；代价是这一帧多算一次 `skin_positions`（阴影排在
                // 主 pass 之前，不能等主 pass 上传）。
                let key = format!("skin:{}:{}", item.entity.raw(), item.model);
                let positions = super::skinning::skin_positions(mesh, pose);
                sources.gpu_meshes.upload_deformed(
                    context,
                    &key,
                    mesh,
                    &positions,
                    sources.textures,
                );
                (key, None)
            }
            None => {
                sources
                    .gpu_meshes
                    .upload(context, &item.model, mesh, sources.textures);
                (item.model.clone(), None)
            }
        };
        casters.push(Caster {
            mesh_key,
            model: item.transform,
            palette,
        });
    }
    Ok(casters)
}

impl SkinnedShadow {
    pub(super) fn new(context: &GpuContext, frame_layout: &wgpu::BindGroupLayout) -> Self {
        let device = context.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xgc-shadow-skin"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SKIN_WGSL.into()),
        });
        let uniform = |size: u64| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                // 竞技场按槽偏移，layout 必须声明 dynamic offset。
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let palette_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xgc-shadow-skin-palette-layout"),
            entries: &[uniform(MAX_JOINTS_BYTES as u64)],
        });
        let model_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xgc-shadow-skin-model-layout"),
            entries: &[uniform(SHADOW_MODEL_UNIFORM_BYTES)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xgc-shadow-skin-layout"),
            bind_group_layouts: &[frame_layout, &palette_layout, &model_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xgc-shadow-skin-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // 只要位置 / 关节下标 / 权重三个流：UV 在深度 pass 没有消费者。
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
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
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                // 与静态投射体共用同一份偏置，否则同一表面两种投射体的条纹不同。
                bias: super::shadow::CASTER_DEPTH_BIAS,
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            palette_layout,
            model_layout,
            palette: UniformArena::new(
                context,
                "xgc-shadow-skin-palette-arena",
                MAX_JOINTS_BYTES as u64,
            ),
            model: UniformArena::new(
                context,
                "xgc-shadow-skin-model-arena",
                SHADOW_MODEL_UNIFORM_BYTES,
            ),
            model_scratch: Vec::new(),
        }
    }

    /// 进 pass 之前把本帧的 palette / model 传进竞技场，返回每个投射体的槽位与本帧
    /// 新建的缓冲数（同尺寸下恒 0，便于回归钉住"没有退化成逐投射体建缓冲"）。
    pub(super) fn bindings(
        &mut self,
        context: &GpuContext,
        casters: &[Caster],
    ) -> (Vec<SkinnedBinding>, usize) {
        let skinned: Vec<&Caster> = casters
            .iter()
            .filter(|caster| caster.skins_here())
            .collect();
        if skinned.is_empty() {
            return (Vec::new(), 0);
        }
        let palettes: Vec<&[u8]> = skinned
            .iter()
            .filter_map(|caster| caster.palette.as_deref())
            .collect();
        // `skins_here` 与 `palette.is_some()` 必须同义：两者一旦漂移，槽号就会整体错位，
        // 而竞技场只校验"非空"，不会替我们发现条数不符。
        debug_assert_eq!(
            palettes.len(),
            skinned.len(),
            "蒙皮投射体与 palette 条数必须一致"
        );
        let mut models = std::mem::take(&mut self.model_scratch);
        while models.len() < skinned.len() {
            models.push(Vec::with_capacity(SHADOW_MODEL_UNIFORM_BYTES as usize));
        }
        for (bytes, caster) in models.iter_mut().take(skinned.len()).zip(&skinned) {
            bytes.clear();
            for value in caster.model {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let model_slices: Vec<&[u8]> = models
            .iter()
            .take(skinned.len())
            .map(|bytes| bytes.as_slice())
            .collect();
        let mut uniform_buffers = self
            .palette
            .upload(context, &self.palette_layout, &palettes);
        uniform_buffers += self
            .model
            .upload(context, &self.model_layout, &model_slices);
        drop(model_slices);
        self.model_scratch = models;
        let bindings = skinned
            .iter()
            .enumerate()
            .map(|(slot, caster)| SkinnedBinding {
                mesh_key: caster.mesh_key.clone(),
                slot,
            })
            .collect();
        (bindings, uniform_buffers)
    }

    /// 在当前级联的 pass 里画出所有 GPU 蒙皮投射体。
    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frame_bind: &wgpu::BindGroup,
        bindings: &[SkinnedBinding],
        skin_meshes: &GpuSkinMeshCache,
    ) {
        if bindings.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, frame_bind, &[]);
        for binding in bindings {
            let Some(mesh) = skin_meshes.get(&binding.mesh_key) else {
                continue;
            };
            pass.set_bind_group(1, self.palette.bind(), &[self.palette.offset(binding.slot)]);
            pass.set_bind_group(2, self.model.bind(), &[self.model.offset(binding.slot)]);
            let (position, joints, weights) = mesh.shadow_buffers();
            pass.set_vertex_buffer(0, position.slice(..));
            pass.set_vertex_buffer(1, joints.slice(..));
            pass.set_vertex_buffer(2, weights.slice(..));
            if let Some(indices) = mesh.shadow_index_buffer() {
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.shadow_indices(), 0, 0..1);
            } else {
                pass.draw(0..mesh.shadow_vertices(), 0..1);
            }
        }
    }
}
