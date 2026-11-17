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
use super::skinned::{self, GpuSkinMeshCache, MAX_JOINTS_BYTES};
use super::texture_bind::TextureBinding;

const SHADOW_SKIN_WGSL: &str = concat!(
    include_str!("skin_vertex.wgsl"),
    include_str!("shadow_skin.wgsl")
);

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

/// 每帧每个蒙皮投射体一份绑定（4 级级联共用，不再逐级重建）。
pub(super) struct SkinnedBinding {
    mesh_key: String,
    palette: wgpu::BindGroup,
    model: wgpu::BindGroup,
}

pub(super) struct SkinnedShadow {
    pipeline: wgpu::RenderPipeline,
    palette_layout: wgpu::BindGroupLayout,
    model_layout: wgpu::BindGroupLayout,
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
            Some(pose) if skinned::supports_gpu_skin(mesh, pose) => {
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
                // >64 关节 / palette 不完整：退回 CPU 变形，与主 pass 同一套函数。
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
                has_dynamic_offset: false,
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
            entries: &[uniform(64)],
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
        }
    }

    /// 进 pass 之前把每帧的 palette / model 绑定建好：4 级级联复用同一批。
    pub(super) fn bindings(&self, context: &GpuContext, casters: &[Caster]) -> Vec<SkinnedBinding> {
        let mut bindings = Vec::new();
        for caster in casters {
            let Some(palette) = &caster.palette else {
                continue;
            };
            let mut model_bytes = Vec::with_capacity(64);
            for value in caster.model {
                model_bytes.extend_from_slice(&value.to_le_bytes());
            }
            bindings.push(SkinnedBinding {
                mesh_key: caster.mesh_key.clone(),
                palette: uniform_bind(
                    context,
                    &self.palette_layout,
                    "xgc-shadow-skin-palette",
                    palette,
                ),
                model: uniform_bind(
                    context,
                    &self.model_layout,
                    "xgc-shadow-skin-model",
                    &model_bytes,
                ),
            });
        }
        bindings
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
            pass.set_bind_group(1, &binding.palette, &[]);
            pass.set_bind_group(2, &binding.model, &[]);
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

fn uniform_bind(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    label: &str,
    bytes: &[u8],
) -> wgpu::BindGroup {
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    context.queue().write_buffer(&buffer, 0, bytes);
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
