//! 粒子渲染：一次实例化 draw 画完同一种混合方式的全部粒子。
//!
//! 输入是模拟层输出的 [`ParticleQuad`]（世界空间中心 + 世界单位半边长 + 线性
//! 颜色 + 混合方式）。每个粒子是朝向相机的正方形，片元里用软圆片 SDF 出边。
//!
//! 与旧端相同的三条约定：
//!
//! 1. **画在场景 pass 侧**（几何之后、UI 之前）：粒子要被几何遮住（深度测试）、
//!    要参与色调映射；颜色在线性空间写（加色火焰 `rgb > 1` 靠它过曝）。
//! 2. **朝向从 `view_proj` 逆矩阵现算**：取 NDC 近平面上的「左-右」「下-上」
//!    两对点映射回世界空间做差，得到相机 right / up —— 入口不必多传相机基。
//! 3. **Alpha 粒子按深度从远到近排序**（预乘混合顺序敏感）；加色粒子与顺序无关。

use xgc_world::{ParticleBlend, ParticleQuad};

use super::camera::{invert, transform_point, Mat4};
use super::gpu::GpuContext;

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
    // 世界空间里展开：朝向完全由相机基决定，与粒子自身"没有朝向"一致。
    let world = inst.center_size.xyz
        + u.right.xyz * (corner.x * half)
        + u.up.xyz * (corner.y * half);
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(world, 1.0);
    out.local = corner;
    out.color = inst.color;
    return out;
}

// XGC 直接写显示目标（主场景着色器也在片元末做 ACES），所以这里镜像同一条曲线。
// 旧端是在 HDR 场景合成后统一过曲线；XGC 尚无 HDR 中间目标，这个顺序差异会留到
// 后处理切片收敛，但不能把线性粒子原样混进已 tone-map 的场景。
fn aces(linear: vec3<f32>) -> vec3<f32> {
    var c = max(linear, vec3<f32>(0.0)) / 0.6;
    c = vec3<f32>(
        0.59719 * c.x + 0.35458 * c.y + 0.04823 * c.z,
        0.07600 * c.x + 0.90834 * c.y + 0.01566 * c.z,
        0.02840 * c.x + 0.13383 * c.y + 0.83777 * c.z,
    );
    let a = c * (c + vec3<f32>(0.0245786)) - vec3<f32>(0.000090537);
    let b = c * (vec3<f32>(0.983729) * c + vec3<f32>(0.432951)) + vec3<f32>(0.238081);
    c = a / b;
    c = vec3<f32>(
        1.60475 * c.x - 0.53108 * c.y - 0.07367 * c.z,
        -0.10208 * c.x + 1.10813 * c.y - 0.00605 * c.z,
        -0.00327 * c.x - 0.07276 * c.y + 1.07602 * c.z,
    );
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    if (d > 1.0) {
        discard;
    }
    // 软边：内圈 0.35 之外开始淡出；全程线性淡出会让火苗像雾，硬边会露锯齿。
    let coverage = 1.0 - smoothstep(0.35, 1.0, d);
    // 预乘输出：rgb 先乘 alpha。
    let alpha = in.color.a * coverage;
    return vec4<f32>(aces(in.color.rgb) * alpha, alpha);
}
"#;

/// 两块管线的粒子绘制器（Alpha / 加色），按颜色格式缓存。
pub(crate) struct ParticlePipeline {
    alpha: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
}

pub(crate) struct ParticleDrawRequest<'a> {
    pub target: &'a wgpu::Texture,
    pub depth: &'a wgpu::TextureView,
    pub view_proj: Mat4,
    pub camera_pos: [f32; 3],
    pub quads: &'a [ParticleQuad],
    pub clear: Option<wgpu::Color>,
}

struct Billboard {
    right: [f32; 3],
    up: [f32; 3],
}

/// 从 `view_proj` 的逆矩阵现算相机 right / up。
fn billboard(inverse: Mat4) -> Billboard {
    let project = |x: f32, y: f32| -> [f32; 3] {
        let [px, py, pz, pw] = transform_point(inverse, [x, y, 0.0, 1.0]);
        if pw.abs() > 1.0e-6 {
            [px / pw, py / pw, pz / pw]
        } else {
            [px, py, pz]
        }
    };
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let normalize = |v: [f32; 3]| {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len.is_finite() && len > 1e-6 {
            [v[0] / len, v[1] / len, v[2] / len]
        } else {
            [0.0; 3]
        }
    };
    let right = normalize(sub(project(1.0, 0.0), project(-1.0, 0.0)));
    let up = normalize(sub(project(0.0, 1.0), project(0.0, -1.0)));
    if right == [0.0; 3] || up == [0.0; 3] {
        Billboard {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        }
    } else {
        Billboard { right, up }
    }
}

impl ParticlePipeline {
    pub(crate) fn new(context: &GpuContext, format: wgpu::TextureFormat) -> Self {
        let device = context.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xgc-particle-shader"),
            source: wgpu::ShaderSource::Wgsl(PARTICLE_WGSL.into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xgc-particle-uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(96),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xgc-particle-layout"),
            bind_group_layouts: &[&uniform_layout],
            push_constant_ranges: &[],
        });
        let pipeline = |label: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 32,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    // 粒子要被几何遮住，但不写深度（半透明不能挡后面的粒子）。
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let alpha = pipeline(
            "xgc-particle-alpha",
            wgpu::BlendState {
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
            },
        );
        let additive = pipeline(
            "xgc-particle-additive",
            wgpu::BlendState {
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
            },
        );
        Self {
            alpha,
            additive,
            uniform_layout,
        }
    }

    /// 画一批粒子到已存在的颜色 / 深度目标上（`LoadOp::Load`）。
    pub(crate) fn draw(
        &self,
        context: &GpuContext,
        request: &ParticleDrawRequest<'_>,
    ) -> Result<usize, String> {
        let target = request.target;
        let depth = request.depth;
        let view_proj = request.view_proj;
        let camera_pos = request.camera_pos;
        let quads = request.quads;
        let clear = request.clear;
        if quads.is_empty() {
            return Ok(0);
        }
        let Some(inverse) = invert(view_proj) else {
            return Ok(0);
        };
        let basis = billboard(inverse);
        // Alpha 按距离从远到近排序（预乘混合顺序敏感）；加色与顺序无关。
        let mut alpha: Vec<(f32, ParticleQuad)> = Vec::new();
        let mut additive: Vec<ParticleQuad> = Vec::new();
        for quad in quads {
            match quad.blend {
                ParticleBlend::Alpha => {
                    alpha.push((distance_sq(camera_pos, quad.position), *quad));
                }
                ParticleBlend::Additive => additive.push(*quad),
            }
        }
        alpha.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut bytes = Vec::with_capacity((alpha.len() + additive.len()) * 32);
        for (_, quad) in &alpha {
            push_instance(&mut bytes, quad);
        }
        for quad in &additive {
            push_instance(&mut bytes, quad);
        }
        let device = context.device();
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-particle-instances"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&instances, 0, &bytes);
        let mut uniforms = Vec::with_capacity(96);
        for value in view_proj {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        for value in [basis.right[0], basis.right[1], basis.right[2], 0.0] {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        for value in [basis.up[0], basis.up[1], basis.up[2], 0.0] {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-particle-uniforms-buffer"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&uniform, 0, &uniforms);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-particle-bind"),
            layout: &self.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xgc-particle-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-particle-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if clear.is_some() {
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
            pass.set_vertex_buffer(0, instances.slice(..));
            pass.set_bind_group(0, &bind, &[]);
            if !alpha.is_empty() {
                pass.set_pipeline(&self.alpha);
                pass.draw(0..4, 0..alpha.len() as u32);
            }
            if !additive.is_empty() {
                pass.set_pipeline(&self.additive);
                let start = alpha.len() as u32;
                pass.draw(0..4, start..start + additive.len() as u32);
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(usize::from(!alpha.is_empty()) + usize::from(!additive.is_empty()))
    }
}

fn distance_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

fn push_instance(bytes: &mut Vec<u8>, quad: &ParticleQuad) {
    for value in [
        quad.position[0],
        quad.position[1],
        quad.position[2],
        quad.size.max(0.0),
        quad.color[0],
        quad.color[1],
        quad.color[2],
        quad.color[3],
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}
