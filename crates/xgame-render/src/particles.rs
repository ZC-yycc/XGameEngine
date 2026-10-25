//! 粒子渲染：**一次实例化 draw 画完同一种混合方式的所有粒子**。
//!
//! 输入是 [`xgame_particle::ParticleQuad`]（世界空间中心 + 世界单位尺寸 +
//! 线性颜色），本模块把它们变成**朝向相机的正方形**并画进**场景 pass**。
//!
//! # 为什么在场景 pass 里画（而不是像 UI 那样在后处理之后）
//!
//! 粒子是**场景里的东西**：它要被几何遮住（深度测试）、要参与曝光与色调映射、
//! 加色火焰写 2.0 的亮度要在 Bloom 里晕开。UI 的规矩正好相反（颜色是作者按
//! 屏幕观感调的，不能随曝光变），两者因此落在 pass 的不同侧。
//!
//! 代价是颜色必须在**线性空间**写：组件里的 `start_color` / `end_color` 就是
//! 线性值（与材质同款约定），不能在这里再做 sRGB 解码 / 编码。
//!
//! # 朝向怎么算（为什么不传相机的 right / up 进来）
//!
//! [`billboard_basis`] 从 `view_proj` 的逆矩阵现算：取 NDC 近平面上的
//! 「左-右」「下-上」两对点，映射回世界空间做差 —— 相机右方向与上方向就出来了。
//! 这样渲染入口的签名只多一个 `quads` 参数，不必让三个宿主各自把相机基传一遍
//! （传参就多三处会写错的地方）。
//!
//! # 混合与批次
//!
//! 两种混合方式 = 两条管线、两次 draw：
//!
//! | 方式 | 混合 | 排序 |
//! |---|---|---|
//! | Alpha | `One / OneMinusSrcAlpha`（预乘） | 远 → 近（混合不可交换） |
//! | Additive | `One / One` | 不用排（加法可交换） |
//!
//! 片元输出是**预乘**的（`rgb * coverage * alpha`），所以 alpha 管线用
//! `src = One` 而不是 `SrcAlpha` —— 用后者会把 alpha 乘两遍，烟雾会薄一半。
//!
//! 深度**测试开、写关**：粒子之间不该互相遮挡（也不该把自己的深度写进去），
//! 但必须被前面的不透明几何挡住。

use std::ops::Range;

use wgpu::util::DeviceExt as _;
use xgame_math::glam::{Mat4, Vec3};
use xgame_particle::ParticleQuad;

use crate::ui_renderer::upload_instances;

// ---------------------------------------------------------------------------
// GPU 数据布局
// ---------------------------------------------------------------------------

/// 全局参数。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ParticleUniform {
    view_proj: [[f32; 4]; 4],
    /// `xyz` = 相机右方向（单位向量）。
    right: [f32; 4],
    /// `xyz` = 相机上方向（单位向量）。
    up: [f32; 4],
}

/// 一个 billboard 实例。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ParticleInstance {
    /// `xyz` = 世界中心，`w` = 半边长（世界单位）。
    center_size: [f32; 4],
    /// 线性空间 RGBA（RGB 可以 > 1）。
    color: [f32; 4],
}

const PARTICLE_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];

// ---------------------------------------------------------------------------
// 着色器
// ---------------------------------------------------------------------------

const PARTICLE_WGSL: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct Instance {
    @location(0) center_size: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // [-1, 1]：以四边形中心为原点，用来算软圆片的 SDF。
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    // 4 个顶点组成 triangle strip：(-1,-1) (1,-1) (-1,1) (1,1)
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;
    let half = inst.center_size.w;
    // 世界空间里展开：朝向完全由相机基决定，与粒子自身"没有朝向"这件事一致。
    let world = inst.center_size.xyz
        + u.right.xyz * (corner.x * half)
        + u.up.xyz * (corner.y * half);
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(world, 1.0);
    out.local = corner;
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    if (d > 1.0) {
        discard;
    }
    // 软边：中心实、边缘渐隐。内圈 0.35 之外开始过渡 —— 全程线性淡出
    // 会让小火苗看起来像一团雾，硬边又会在放大时露出锯齿。
    let coverage = 1.0 - smoothstep(0.35, 1.0, d);
    // **预乘**输出：rgb 先乘覆盖率与 alpha（见模块头的混合说明）。
    let alpha = in.color.a * coverage;
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
"#;

// ---------------------------------------------------------------------------
// 统计
// ---------------------------------------------------------------------------

/// 一帧的粒子绘制统计（性能面板 / 测试断言）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParticleDrawStats {
    /// 参与绘制的粒子数。
    pub quads: usize,
    /// Alpha 混合的粒子数（需要排序）。
    pub alpha: usize,
    /// 加色混合的粒子数。
    pub additive: usize,
    /// 实际 draw call 数（0 / 1 / 2）。
    pub draws: usize,
}

// ---------------------------------------------------------------------------
// 渲染器
// ---------------------------------------------------------------------------

/// 粒子渲染器：两条管线 + 一块逐帧重写的实例缓冲。
pub struct ParticleRenderer {
    uniform: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    alpha_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,

    instances: Vec<ParticleInstance>,
    /// Alpha 粒子的临时排序缓冲（跨帧复用容量，稳态零分配）。
    scratch: Vec<(f32, ParticleQuad)>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,

    /// 实例缓冲里两段的区间：`[0, alpha_end)` 是 Alpha，`[alpha_end, ..)` 是加色。
    alpha_range: Range<u32>,
    additive_range: Range<u32>,
    stats: ParticleDrawStats,
}

impl ParticleRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle_shader"),
            source: wgpu::ShaderSource::Wgsl(PARTICLE_WGSL.into()),
        });

        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle_uniform"),
            contents: bytemuck::bytes_of(&ParticleUniform {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                right: [1.0, 0.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle_uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle_uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle_pipeline_layout"),
            bind_group_layouts: &[&uniform_bgl],
            push_constant_ranges: &[],
        });

        // 预乘 alpha 混合（片元已经乘过 alpha，所以 src 因子是 One）。
        let alpha_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let additive_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let build = |label: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ParticleInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &PARTICLE_ATTRS,
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    // 粒子是双面片：不要剔面（相机在粒子后方时不该消失）。
                    cull_mode: None,
                    ..Default::default()
                },
                // 深度测试开、写关（见模块头）。
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count.max(1),
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        };

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle_instances"),
            size: std::mem::size_of::<ParticleInstance>() as u64 * 256,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            uniform,
            uniform_bg,
            alpha_pipeline: build("particle_alpha_pipeline", alpha_blend),
            additive_pipeline: build("particle_additive_pipeline", additive_blend),
            instances: Vec::new(),
            scratch: Vec::new(),
            instance_buffer,
            instance_capacity: 256,
            alpha_range: 0..0,
            additive_range: 0..0,
            stats: ParticleDrawStats::default(),
        }
    }

    /// 本帧统计。
    pub fn stats(&self) -> ParticleDrawStats {
        self.stats
    }

    /// 准备本帧的实例数据。没有粒子时返回 `false`（`draw` 会整段跳过）。
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: &Mat4,
        camera_pos: &Vec3,
        quads: &[ParticleQuad],
    ) -> bool {
        self.instances.clear();
        self.alpha_range = 0..0;
        self.additive_range = 0..0;
        self.stats = ParticleDrawStats::default();
        if quads.is_empty() {
            return false;
        }

        let (right, up) = billboard_basis(view_proj);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&ParticleUniform {
                view_proj: view_proj.to_cols_array_2d(),
                right: [right.x, right.y, right.z, 0.0],
                up: [up.x, up.y, up.z, 0.0],
            }),
        );

        // -- Alpha：远 → 近 --
        self.scratch.clear();
        for quad in quads {
            if quad.blend != xgame_core::ParticleBlend::Alpha {
                continue;
            }
            let d = Vec3::from(quad.position).distance_squared(*camera_pos);
            self.scratch.push((d, *quad));
        }
        // `sort_by` 是稳定排序：距离相同（浮点相等）时保持发射顺序，
        // 输出因此逐帧可复现（截图基线依赖这条）。
        self.scratch
            .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let alpha_count = self.scratch.len();
        for (_, quad) in &self.scratch {
            self.instances.push(instance_of(quad));
        }

        // -- 加色：不用排 --
        let mut additive_count = 0usize;
        for quad in quads {
            if quad.blend != xgame_core::ParticleBlend::Additive {
                continue;
            }
            self.instances.push(instance_of(quad));
            additive_count += 1;
        }

        let alpha_end = alpha_count as u32;
        let total = self.instances.len() as u32;
        self.alpha_range = 0..alpha_end;
        self.additive_range = alpha_end..total;
        self.stats = ParticleDrawStats {
            quads: quads.len(),
            alpha: alpha_count,
            additive: additive_count,
            draws: usize::from(alpha_count > 0) + usize::from(additive_count > 0),
        };

        if let Some(buffer) = upload_instances(
            device,
            queue,
            &self.instance_buffer,
            &mut self.instance_capacity,
            "particle_instances",
            &self.instances,
        ) {
            self.instance_buffer = buffer;
        }
        true
    }

    /// 在**场景 pass** 里画粒子（调用方需已开好带深度附件的 pass）。
    ///
    /// 返回本次发出的 draw call 数（0 = 什么都没画）。
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> usize {
        if self.instances.is_empty() {
            return 0;
        }
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));

        let mut draws = 0;
        if !self.alpha_range.is_empty() {
            pass.set_pipeline(&self.alpha_pipeline);
            pass.draw(0..4, self.alpha_range.clone());
            draws += 1;
        }
        if !self.additive_range.is_empty() {
            pass.set_pipeline(&self.additive_pipeline);
            pass.draw(0..4, self.additive_range.clone());
            draws += 1;
        }
        draws
    }
}

fn instance_of(quad: &ParticleQuad) -> ParticleInstance {
    ParticleInstance {
        center_size: [
            quad.position[0],
            quad.position[1],
            quad.position[2],
            quad.size.max(0.0),
        ],
        color: quad.color,
    }
}

/// 从 `view_proj` 的逆矩阵里取出相机的世界空间右 / 上方向。
///
/// 做法：把 NDC 近平面上的「左-右」「下-上」两对点映射回世界空间，做差再
/// 归一化。透视相机的屏幕基在整个视锥里恒定（只有位置在变），所以这一对
/// 方向可以直接用在任意深度的粒子上。
///
/// 矩阵不可逆（退化相机）时退回世界轴 —— 画面不对，但不会出 NaN 四边形。
pub fn billboard_basis(view_proj: &Mat4) -> (Vec3, Vec3) {
    let inv = view_proj.inverse();
    if !inv.is_finite() {
        return (Vec3::X, Vec3::Y);
    }
    // wgpu 的 NDC z ∈ [0, 1]，0 就是近平面。
    let left = inv.project_point3(Vec3::new(-1.0, 0.0, 0.0));
    let right = inv.project_point3(Vec3::new(1.0, 0.0, 0.0));
    let bottom = inv.project_point3(Vec3::new(0.0, -1.0, 0.0));
    let top = inv.project_point3(Vec3::new(0.0, 1.0, 0.0));

    let right_dir = (right - left).normalize_or_zero();
    let up_dir = (top - bottom).normalize_or_zero();
    if right_dir == Vec3::ZERO || up_dir == Vec3::ZERO {
        return (Vec3::X, Vec3::Y);
    }
    (right_dir, up_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_math::projection;

    /// 一台看向 -Z、上为 +Y 的相机：屏幕右 = +X，屏幕上 = +Y。
    #[test]
    fn billboard_basis_of_a_straight_camera_is_world_axes() {
        let view = projection::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
        let proj = projection::perspective(60f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let (right, up) = billboard_basis(&(proj * view));
        assert!((right - Vec3::X).length() < 1e-5, "right = {right:?}");
        assert!((up - Vec3::Y).length() < 1e-5, "up = {up:?}");
    }

    /// 相机绕 Y 转 90° 后朝向 -X：屏幕上方向不变，屏幕右变成 -Z。
    #[test]
    fn billboard_basis_follows_the_camera_yaw() {
        let view = projection::look_at(Vec3::new(5.0, 0.0, 0.0), Vec3::ZERO, Vec3::Y);
        let proj = projection::perspective(60f32.to_radians(), 1.0, 0.1, 100.0);
        let (right, up) = billboard_basis(&(proj * view));
        assert!(
            (right - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5,
            "right = {right:?}"
        );
        assert!((up - Vec3::Y).length() < 1e-5, "up = {up:?}");
    }

    /// 相机俯视（沿 -Y 看）：上方向退化到世界轴，但结果必须是有限单位向量。
    #[test]
    fn billboard_basis_stays_sane_for_a_top_down_camera() {
        let view = projection::look_at(Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO, Vec3::Z);
        let proj = projection::perspective(60f32.to_radians(), 1.0, 0.1, 100.0);
        let (right, up) = billboard_basis(&(proj * view));
        assert!(right.is_finite() && up.is_finite());
        assert!((right.length() - 1.0).abs() < 1e-4);
        assert!((up.length() - 1.0).abs() < 1e-4);
        // 正交：差一点也算（数值误差），但绝不能共线。
        assert!(right.dot(up).abs() < 1e-3, "right={right:?} up={up:?}");
    }

    #[test]
    fn degenerate_matrix_falls_back_to_world_axes() {
        let (right, up) = billboard_basis(&Mat4::ZERO);
        assert_eq!((right, up), (Vec3::X, Vec3::Y));
    }
}
