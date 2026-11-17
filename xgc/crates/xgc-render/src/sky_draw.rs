//! 天空盒 pass：全屏三角形 + 解析天空求值（背景不再是一块清屏色）。
//!
//! 与旧引擎同一个落点：**场景 pass 内、不透明几何之后**画一张全屏三角形，
//! 顶点推到远平面（NDC z = 1.0），深度比较 `LessEqual`、不写深度 —— 有几何的
//! 像素被几何挡住，其余像素由解析天空填满（含太阳盘与光晕）。
//!
//! 方向由 `inv_view_proj` 从 NDC 反推（与旧引擎天空盒同一条路径），因此和相机
//! 的视场 / 纵横比 / 远近裁剪面天然一致。`sky.rs` 的 `sky_radiance` 是这里的
//! CPU 镜像，两边逐式对应。

use super::camera::Mat4;
use super::gpu::GpuContext;
use super::sky::SkyUniform;

/// 统一 uniform：inv_view_proj 64B + 相机位置 16B + 天空 5×16B = 160B。
const SKY_UNIFORM_BYTES: u64 = 160;

const SKY_WGSL: &str = r#"
struct SkyFrame {
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // 与 sky.rs 的 SkyUniform 字段顺序一致
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    ground: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: SkyFrame;

const SKY_PI: f32 = 3.141592653589793;
const SUN_DISK_RADIANCE: f32 = 40.0;
const SUN_GLOW: f32 = 0.35;

// 与 sky.rs 的 sky_radiance 逐式对应。
fn sky_radiance(dir: vec3<f32>) -> vec3<f32> {
    let t = clamp(dir.y, -1.0, 1.0);
    let up_t = pow(max(t, 0.0), 0.5);
    let down_t = pow(max(-t, 0.0), 0.5);
    let sky_c = mix(frame.horizon.rgb, frame.zenith.rgb, up_t);
    let gnd_c = mix(frame.horizon.rgb, frame.ground.rgb, down_t);
    var c = select(gnd_c, sky_c, t > 0.0);

    let cos_ang = clamp(dot(dir, frame.sun_dir.xyz), -1.0, 1.0);
    let ang = acos(cos_ang);
    let sun_rad = max(SKY_PI / 180.0 * frame.horizon.w, 1e-4);
    let disk = 1.0 - smoothstep(sun_rad * 0.6, sun_rad, ang);
    let glow = SUN_GLOW * pow(max(cos_ang, 0.0), 16.0);
    c = c + frame.sun_color.rgb * frame.sun_color.a * (SUN_DISK_RADIANCE * disk + glow);
    return c * frame.zenith.a;
}

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    // 全屏三角形：少一次顶点、没有斜接对角线上的像素级接缝。
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let xy = corners[index];
    var out: VsOut;
    out.position = vec4<f32>(xy, 1.0, 1.0);
    out.ndc = xy;
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    let far = frame.inv_view_proj * vec4<f32>(input.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - frame.eye.xyz);
    return vec4<f32>(sky_radiance(dir), 1.0);
}
"#;

/// 天空盒管线（按目标格式缓存；与实例化管线同寿）。
pub(crate) struct SkyPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuContext {
    pub(crate) fn create_sky_pipeline(&self, format: wgpu::TextureFormat) -> SkyPipeline {
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-sky"),
                source: wgpu::ShaderSource::Wgsl(SKY_WGSL.into()),
            });
        let layout = self
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xgc-sky-frame"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(SKY_UNIFORM_BYTES),
                    },
                    count: None,
                }],
            });
        let pipeline_layout =
            self.device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xgc-sky-layout"),
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                });
        let pipeline = self
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-sky-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                // 远平面 + LessEqual + 不写深度：有几何的像素被几何挡住。
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::LessEqual,
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
        SkyPipeline { pipeline, layout }
    }
}

impl SkyPipeline {
    /// 在给定 render pass 里画天空（**必须**在实例化 / 蒙皮 draw 之后）。
    pub(crate) fn draw(
        &self,
        context: &GpuContext,
        pass: &mut wgpu::RenderPass<'_>,
        inv_view_proj: Mat4,
        eye: [f32; 3],
        sky: &SkyUniform,
    ) {
        let mut bytes = Vec::with_capacity(SKY_UNIFORM_BYTES as usize);
        for value in inv_view_proj {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [eye[0], eye[1], eye[2], 1.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        sky.write(&mut bytes);
        let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-sky-frame-buffer"),
            size: SKY_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&buffer, 0, &bytes);
        let bind = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-sky-frame-bind"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
