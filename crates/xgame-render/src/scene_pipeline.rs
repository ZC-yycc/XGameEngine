//! 共享 3D 场景渲染管线。
//!
//! 封装 wgpu 渲染管线、着色器、uniform 缓冲区和逐实体绘制逻辑。
//! 可在编辑器离屏渲染器和独立游戏二进制中复用。
//!
//! # 着色模型
//!
//! 采用 **Blinn-Phong**（环境光 + 漫反射 + 半程向量高光），单帧最多
//! [`MAX_LIGHTS`] 盏方向光 / 点光叠加。这不是 PBR——完整 PBR 需要 IBL 与
//! 能量守恒，属于后续"材质系统"的范畴；这一版先把最要紧的三件事做对：
//! **纹理真的被采样**、**高光真的存在**、**多盏灯真的能叠加**。
//!
//! # 绑定组布局
//!
//! ```text
//! group(0)  全局，整帧一份
//!   binding 0  camera    uniform
//!   binding 1  lights    uniform（环境光 + 光源数组）
//!   binding 2  model     uniform（动态偏移：每个实体占一段）
//!
//! group(1)  材质，每个材质槽一份
//!   binding 0  base color texture
//!   binding 1  sampler
//!   binding 2  material  uniform（反照率因子 + 高光参数）
//! ```
//!
//! 材质绑定组**惰性创建并缓存**（键是 `GltfPrimitive::material_id`），
//! 所以一次模型加载之后，每帧只是查表 + `set_bind_group`，不会重复建对象。

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use xgame_math::glam::{Mat4, Vec3};

use xgame_core::{Entity, Light, LightKind};
use crate::gltf_loader::{GltfMesh, GltfPrimitive, GltfVertex, MaterialParams};

/// 每帧最多渲染的实体数量。
const MAX_RENDER_ENTITIES: usize = 256;

/// 单帧支持的最大光源数。
///
/// 这个值在三处必须一致：本常量、[`LightsUniform`] 的数组长度、WGSL 里的
/// `MAX_LIGHTS`。改的时候别漏。
pub const MAX_LIGHTS: usize = 8;

// ---------------------------------------------------------------------------
// GPU uniform 类型（必须与 WGSL 布局逐字节对齐）
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
}

/// 单盏光源的 GPU 表示。
///
/// 方向光和点光共用一个结构体，靠 `pos_type.w` 区分（0=方向光，1=点光）——
/// 这样着色器里只需要一个循环，不必写两套分支逻辑。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct GpuLight {
    /// 方向光：xyz = 光线传播方向，w = 0。
    /// 点光：  xyz = 世界坐标位置，w = 1。
    pos_type: [f32; 4],
    /// rgb = 光色（线性），a = 强度。
    color_intensity: [f32; 4],
}

impl GpuLight {
    const ZERO: Self = Self {
        pos_type: [0.0; 4],
        color_intensity: [0.0; 4],
    };
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct LightsUniform {
    /// rgb = 环境光，a = 有效光源数量（着色器按它截断循环）。
    ambient: [f32; 4],
    lights: [GpuLight; MAX_LIGHTS],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ModelUniform {
    model: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct MaterialUniform {
    /// rgb = 基础色因子，a = 不透明度。
    base_color: [f32; 4],
    /// x = 高光强度，y = 光泽度（Phong 指数），zw 预留。
    params: [f32; 4],
}

impl MaterialUniform {
    /// 把 glTF 的 metallic-roughness 参数翻译成 Blinn-Phong 的两个经典参数。
    ///
    /// 两处换算都是为了"看起来对"而不是"物理正确"：
    /// - **高光强度**：绝缘体的菲涅尔反射率约 4%，金属接近全反射，所以在
    ///   `0.04 ~ 0.6` 之间按金属度插值（上限压在 0.6，否则全金属模型会白到失形）。
    /// - **光泽度**：`2/r⁴ - 2` 是 Blinn-Phong 指数与 GGX 粗糙度之间的常用近似，
    ///   再夹到 `[2, 512]`——下界防止 `pow(x, 0)` 出现未定义值，上界防止
    ///   高光小到看不见还白白吃算力。
    fn from_params(p: &MaterialParams) -> Self {
        let specular = 0.04 + p.metallic.clamp(0.0, 1.0) * 0.56;
        let r = p.roughness.clamp(0.02, 1.0);
        let shininess = (2.0 / (r * r * r * r) - 2.0).clamp(2.0, 512.0);
        Self {
            base_color: p.base_color,
            params: [specular, shininess, 0.0, 0.0],
        }
    }
}

/// 一帧的光照环境（环境光 + 光源列表）。
///
/// 渲染管线不直接依赖 `Scene`，而是收这个"视图"——编辑器、游戏、以及未来
/// 的预览窗口可以各自组装光照，不必都去构造一个完整的 `Scene`。
#[derive(Clone, Copy, Debug)]
pub struct SceneLighting<'a> {
    /// 全局环境光（线性 RGB）。
    pub ambient: [f32; 3],
    /// 场景光源。超过 [`MAX_LIGHTS`] 的部分会被忽略。
    pub lights: &'a [Light],
}

impl<'a> SceneLighting<'a> {
    pub fn new(ambient: [f32; 3], lights: &'a [Light]) -> Self {
        Self { ambient, lights }
    }
}

// ---------------------------------------------------------------------------
// 内联 WGSL 着色器
// ---------------------------------------------------------------------------

const SCENE_VERT_SRC: &str = r#"
struct CameraUniform { view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
struct ModelUniform  { model: mat4x4<f32> }

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(2) var<uniform> model: ModelUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
}
struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
}

// 手写 3x3 求逆：non-uniform scale 下法线必须用逆转置矩阵变换，
// 直接乘 model 会让法线歪掉、光照方向全错。
fn mat3_inverse(m: mat3x3<f32>) -> mat3x3<f32> {
    let a = m[0][0]; let b = m[0][1]; let c = m[0][2];
    let d = m[1][0]; let e = m[1][1]; let f = m[1][2];
    let g = m[2][0]; let h = m[2][1]; let i = m[2][2];
    let det = a * (e*i - f*h) - b * (d*i - f*g) + c * (d*h - e*g);
    let inv_det = 1.0 / det;
    return mat3x3<f32>(
        (e*i - f*h)*inv_det, (c*h - b*i)*inv_det, (b*f - c*e)*inv_det,
        (f*g - d*i)*inv_det, (a*i - c*g)*inv_det, (c*d - a*f)*inv_det,
        (d*h - e*g)*inv_det, (b*g - a*h)*inv_det, (a*e - b*d)*inv_det,
    );
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let world_pos = model.model * vec4<f32>(input.position, 1.0);
    let m = model.model;
    let m33 = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
    let normal_mat = transpose(mat3_inverse(m33));
    var out: VertexOutput;
    out.clip_pos = camera.view_proj * world_pos;
    out.world_normal = normal_mat * input.normal;
    out.world_pos = world_pos.xyz;
    out.color = input.color;
    out.tex_coord = input.tex_coord;
    return out;
}
"#;

const SCENE_FRAG_SRC: &str = r#"
const MAX_LIGHTS: u32 = 8u;

struct CameraUniform { view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
struct GpuLight      { pos_type: vec4<f32>, color_intensity: vec4<f32> }
struct LightsUniform { ambient: vec4<f32>, lights: array<GpuLight, 8> }
struct MaterialUniform { base_color: vec4<f32>, params: vec4<f32> }

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<uniform> lights: LightsUniform;
@group(1) @binding(0) var base_color_tex: texture_2d<f32>;
@group(1) @binding(1) var base_color_smp: sampler;
@group(1) @binding(2) var<uniform> material: MaterialUniform;

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // 反照率 = 贴图 × 材质因子 × 顶点色（三者都是"乘法叠加"的直觉语义）
    let sampled = textureSample(base_color_tex, base_color_smp, input.tex_coord);
    let albedo = sampled.rgb * material.base_color.rgb * input.color;
    let alpha = sampled.a * material.base_color.a;

    let N = normalize(input.world_normal);
    let V = normalize(camera.camera_pos.xyz - input.world_pos);

    var color = lights.ambient.rgb;
    let count = min(u32(lights.ambient.a), MAX_LIGHTS);

    for (var i = 0u; i < MAX_LIGHTS; i = i + 1u) {
        if (i >= count) { break; }
        let light = lights.lights[i];

        var L: vec3<f32>;
        var atten = 1.0;
        if (light.pos_type.w < 0.5) {
            // 方向光：pos_type.xyz 是光线传播方向，入射方向取反
            L = normalize(-light.pos_type.xyz);
        } else {
            // 点光：按到光源的距离做平滑衰减（1/(1+kd+kd²) 的标准近似）
            let to_light = light.pos_type.xyz - input.world_pos;
            let dist = length(to_light);
            L = to_light / max(dist, 1e-4);
            atten = 1.0 / (1.0 + 0.09 * dist + 0.032 * dist * dist);
        }

        let ndotl = max(dot(N, L), 0.0);
        let radiance = light.color_intensity.rgb * light.color_intensity.a * atten;

        // Blinn-Phong 高光：用半程向量 H 代替反射向量，比 Phong 更快也更稳
        let H = normalize(L + V);
        let spec = pow(max(dot(N, H), 0.0), material.params.y) * material.params.x * ndotl;

        color = color + albedo * ndotl * radiance + spec * radiance;
    }

    return vec4<f32>(color, alpha);
}
"#;

// ---------------------------------------------------------------------------
// ScenePipeline
// ---------------------------------------------------------------------------

/// 一个材质槽的 GPU 资源。
struct MaterialBinding {
    bind_group: wgpu::BindGroup,
    /// 材质 uniform 缓冲。`BindGroup` 内部其实已经持有引用，这里显式留着是为了
    /// 语义清晰——删了这个槽就一起释放。
    _uniform: wgpu::Buffer,
}

/// 共享 3D 场景渲染管线。
///
/// 创建一次，每帧调用 [`render`](Self::render)。不持有离屏纹理或交换链——
/// 由调用者传入。
pub struct ScenePipeline {
    /// 不透明变体。
    pipeline: wgpu::RenderPipeline,
    /// 半透明变体：开启 alpha 混合、关闭深度写入（否则半透明物体会
    /// 把它后面的东西提前挡掉）。
    blend_pipeline: wgpu::RenderPipeline,

    /// 全局绑定组布局。构造完 `global_bind_group` 后就不再被直接使用，
    /// 但显式持有它，免得依赖"wgpu 内部 Arc 一定活着"这种隐含前提。
    _global_bgl: wgpu::BindGroupLayout,
    material_bgl: wgpu::BindGroupLayout,

    camera_buffer: wgpu::Buffer,
    lights_buffer: wgpu::Buffer,
    model_buffer: wgpu::Buffer,
    global_bind_group: wgpu::BindGroup,

    /// `material_id` → 材质槽 GPU 资源。
    material_cache: HashMap<u64, MaterialBinding>,

    uniform_alignment: u64,
}

impl ScenePipeline {
    /// 创建渲染管线、绑定组和 uniform 缓冲。
    ///
    /// `format` 是颜色附件的纹理格式（编辑器离屏或交换链）。
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let global_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_global_bgl"),
            entries: &[
                // 0: 相机（顶点阶段要 view_proj，片元阶段要相机位置算半程向量）
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 1: 光照
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 2: 模型矩阵（动态偏移：一个实体一段，避免每个实体一个绑定组）
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_material_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_camera_ub"),
            contents: bytemuck::cast_slice(&[CameraUniform {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0; 4],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_lights_ub"),
            contents: bytemuck::bytes_of(&LightsUniform {
                ambient: [0.06, 0.06, 0.08, 1.0],
                lights: [GpuLight::ZERO; MAX_LIGHTS],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let model_buffer_size = MAX_RENDER_ENTITIES as u64 * uniform_alignment;
        let model_buffer_init = vec![0u8; model_buffer_size as usize];
        let model_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_model_ub"),
            contents: &model_buffer_init,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_global_bg"),
            layout: &global_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: lights_buffer.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &model_buffer,
                        offset: 0,
                        size: Some(
                            std::num::NonZeroU64::new(
                                std::mem::size_of::<ModelUniform>() as u64,
                            )
                            .unwrap(),
                        ),
                    }),
                },
            ],
        });

        let pipeline = Self::build_pipeline(device, &global_bgl, &material_bgl, format, false);
        let blend_pipeline = Self::build_pipeline(device, &global_bgl, &material_bgl, format, true);

        Self {
            pipeline,
            blend_pipeline,
            _global_bgl: global_bgl,
            material_bgl,
            camera_buffer,
            lights_buffer,
            model_buffer,
            global_bind_group,
            material_cache: HashMap::new(),
            uniform_alignment,
        }
    }

    fn build_pipeline(
        device: &wgpu::Device,
        global_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        blend: bool,
    ) -> wgpu::RenderPipeline {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_vs"),
            source: wgpu::ShaderSource::Wgsl(SCENE_VERT_SRC.into()),
        });
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_fs"),
            source: wgpu::ShaderSource::Wgsl(SCENE_FRAG_SRC.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_pl"),
            bind_group_layouts: &[global_bgl, material_bgl],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(if blend { "scene_pipeline_blend" } else { "scene_pipeline" }),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs_main"),
                buffers: &[GltfVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    // 不透明用 REPLACE；半透明用标准的 src-alpha / 1-src-alpha 混合
                    blend: Some(if blend {
                        wgpu::BlendState::ALPHA_BLENDING
                    } else {
                        wgpu::BlendState::REPLACE
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // 半透明物体关掉背面剔除——否则看穿玻璃杯时会缺一半内壁
                cull_mode: if blend { None } else { Some(wgpu::Face::Back) },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: !blend,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    /// 把 [`SceneLighting`] 打包成 GPU 可用的光源数组。
    fn build_lights(lighting: &SceneLighting<'_>) -> LightsUniform {
        let mut uniform = LightsUniform {
            ambient: [lighting.ambient[0], lighting.ambient[1], lighting.ambient[2], 0.0],
            lights: [GpuLight::ZERO; MAX_LIGHTS],
        };

        let mut count = 0usize;
        for light in lighting.lights.iter().filter(|l| l.is_active()).take(MAX_LIGHTS) {
            let color = [
                light.color[0],
                light.color[1],
                light.color[2],
                light.intensity,
            ];
            uniform.lights[count] = match light.kind {
                LightKind::Directional => {
                    let d = light.normalized_direction();
                    GpuLight { pos_type: [d[0], d[1], d[2], 0.0], color_intensity: color }
                }
                LightKind::Point => GpuLight {
                    pos_type: [light.position[0], light.position[1], light.position[2], 1.0],
                    color_intensity: color,
                },
            };
            count += 1;
        }
        uniform.ambient[3] = count as f32;
        uniform
    }

    /// 为一个材质槽创建（或复用）绑定组。
    ///
    /// 惰性创建：第一次遇到某个 `material_id` 时建，之后整帧乃至整局复用。
    /// 这也是为什么材质 ID 要全局唯一——不同模型里即使材质完全一样，
    /// 各自持有自己的槽位，不会因为缓存键相同而串台。
    fn material_binding(&mut self, device: &wgpu::Device, prim: &GltfPrimitive) -> &wgpu::BindGroup {
        if !self.material_cache.contains_key(&prim.material_id) {
            let uniform = MaterialUniform::from_params(&prim.material);
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scene_material_ub"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            // 这里对 `self.material_bgl`（不可变）与 `self.material_cache`（可变）的
            // 借用不重叠——layout 的借用只在 create_bind_group 这一句里活着，
            // 返回的 BindGroup 持有的是自己的资源引用，不挂在 self 上。
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene_material_bg"),
                layout: &self.material_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&prim.texture.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&prim.texture.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            });
            self.material_cache
                .insert(prim.material_id, MaterialBinding { bind_group, _uniform: buffer });
        }
        &self.material_cache[&prim.material_id].bind_group
    }

    /// 执行一帧场景渲染。
    ///
    /// 写入 camera / lights / model uniform，按"先不透明、后半透明（远→近）"
    /// 的顺序发出绘制调用。调用者需提前用 `queue.submit()` 提交 `encoder`。
    ///
    /// # 关于 `world_matrices`
    ///
    /// 着色器要的是**世界矩阵**，而实体自己存的是**局部变换**（层级在
    /// `xgame_core` 里维护）。层级解算不放在渲染器里，是为了让"世界矩阵怎么算"
    /// 只有一份实现（`Scene::world_matrices`），渲染器只消费结果。
    /// 调用方传入的 `world_matrices` 必须与 `entities` 同序同长；长度不足的
    /// 位置退回该实体的局部矩阵，不会 panic。
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        entities: &[Entity],
        world_matrices: &[Mat4],
        model_cache: &HashMap<String, GltfMesh>,
        view_proj: &Mat4,
        camera_pos: &Vec3,
        lighting: SceneLighting<'_>,
    ) {
        // --- 每帧全局 uniform ---
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[CameraUniform {
                view_proj: view_proj.to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 1.0],
            }]),
        );
        queue.write_buffer(&self.lights_buffer, 0, bytemuck::bytes_of(&Self::build_lights(&lighting)));

        // --- 收集绘制项，同时写 model uniform ---
        // 不透明与半透明分开收集：半透明要按到相机的距离从远到近排序，
        // 否则两个半透明物体前后叠在一起时，谁的 alpha 生效取决于绘制顺序。
        let mut opaque: Vec<(usize, &GltfPrimitive)> = Vec::new();
        let mut blended: Vec<(f32, usize, &GltfPrimitive)> = Vec::new();

        for (i, entity) in entities.iter().enumerate() {
            if i >= MAX_RENDER_ENTITIES {
                break;
            }
            if entity.model.is_empty() {
                continue;
            }
            let Some(mesh) = model_cache.get(&entity.model) else {
                continue;
            };

            // 世界矩阵由调用方解算好（层级在 core 里维护）；越界时退回局部矩阵，
            // 保证即使调用方少传了一段也不会画出诡异的东西。
            let world = world_matrices
                .get(i)
                .copied()
                .unwrap_or_else(|| entity.transform.local_matrix());

            let offset = i as u64 * self.uniform_alignment;
            queue.write_buffer(
                &self.model_buffer,
                offset,
                bytemuck::cast_slice(&[ModelUniform {
                    model: world.to_cols_array_2d(),
                }]),
            );

            // 半透明排序用的是**世界坐标**——实体被父节点挪过位置之后，
            // 局部坐标早就不是它在屏幕上的深度了。
            let world_pos = Vec3::new(world.w_axis.x, world.w_axis.y, world.w_axis.z);
            let dist = (world_pos - *camera_pos).length_squared();
            for prim in &mesh.primitives {
                if prim.material.blend {
                    blended.push((dist, i, prim));
                } else {
                    opaque.push((i, prim));
                }
            }
        }

        // 远 → 近。`partial_cmp` 对 NaN 返回 None，退化成"保持原序"即可，
        // 不值得为一个坏变换把整帧干掉。
        blended.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // --- 渲染通道 ---
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.12, g: 0.12, b: 0.14, a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        // 不透明：写深度
        rpass.set_pipeline(&self.pipeline);
        for (i, prim) in &opaque {
            let offset = *i as u64 * self.uniform_alignment;
            rpass.set_bind_group(0, &self.global_bind_group, &[offset as u32]);
            let material = self.material_binding(device, prim);
            rpass.set_bind_group(1, material, &[]);
            rpass.set_vertex_buffer(0, prim.vertex_buffer.slice(..));
            rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
            rpass.draw_indexed(0..prim.num_indices, 0, 0..1);
        }

        // 半透明：不写深度，远→近
        if !blended.is_empty() {
            rpass.set_pipeline(&self.blend_pipeline);
            for (_, i, prim) in &blended {
                let offset = *i as u64 * self.uniform_alignment;
                rpass.set_bind_group(0, &self.global_bind_group, &[offset as u32]);
                let material = self.material_binding(device, prim);
                rpass.set_bind_group(1, material, &[]);
                rpass.set_vertex_buffer(0, prim.vertex_buffer.slice(..));
                rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
                rpass.draw_indexed(0..prim.num_indices, 0, 0..1);
            }
        }
    }
}
