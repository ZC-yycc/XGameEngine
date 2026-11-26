//! 环境立方体的预过滤（GGX 5 级）+ BRDF 积分表：旧端 `sky.rs` 的逐行移植。
//!
//! 为什么要有这一层：场景的环境镜面项如果只用 L2 球谐近似，高光会被抹成一大片钝
//! 光，彩色介质面上会浮出偏蓝的假高光（第 52 轮像素对照实测的正是这个）。旧端用
//! split-sum 的预过滤立方体 + BRDF LUT，这里保持同一套数字，让环境镜面项与旧端
//! 收敛到同一画面。
//!
//! 关键约束：
//! - 全部采样方向来自固定 Hammersley 序列，没有随机数 ⇒ 同一帧反复渲染逐字节相同；
//! - 天空关闭时**一个 pass 都不跑**（`prepare` 直接返回 0），采样端也走
//!   `sky_params.x > 0.5` 守卫，所以关天空的项目与切片前逐字节一致；
//! - 天空设置（`SkyUniform` 的 80 字节）变了才重建，靠 FNV-1a 字节哈希比较。

use super::gpu::GpuContext;
use super::sky::SkyUniform;
use crate::SkySettings;

/// 预过滤立方体的级数（含第 0 级）。
pub(crate) const ENV_MIPS: u32 = 5;
/// 预过滤立方体每面的边长（第 0 级）。
pub(crate) const ENV_BASE_SIZE: u32 = 128;
/// BRDF 积分表的边长。
pub(crate) const BRDF_LUT_SIZE: u32 = 128;
/// 每 (level, face) 的预过滤参数字节数（3 个 vec4 = 48B）。
const PREFILTER_PARAMS_BYTES: u64 = 48;
/// 天空 uniform 的字节数（`SkyUniform` 5 个 vec4）。
const SKY_UNIFORM_BYTES: u64 = 80;

const ENV_WGSL: &str = concat!(
    include_str!("sky_common.wgsl"),
    include_str!("env_prefilter.wgsl")
);

/// 某一级的预过滤采样数：粗糙度越大，积分类越宽、越需要更多样本。
///
/// 表与旧端 `prefilter_samples` 逐项相同（96 / 48 / 24 / 16 / 12）——它决定了
/// 高光边缘的噪声形态，换样本数会让画面与旧端不可比。
pub(crate) fn prefilter_samples(level: u32) -> u32 {
    match level {
        0 => 96,
        1 => 48,
        2 => 24,
        3 => 16,
        _ => 12,
    }
}

/// 立方体一面的 (forward, right, up) 基向量，面序 = WebGPU 层序（+X, −X, +Y, −Y, +Z, −Z）。
///
/// 方向、y 轴朝向都与 `wgpu` 的立方体贴图约定一致，这样采样端 `textureSampleLevel`
/// 拿到的就是写进去的同一方向；与旧端 `cpu::cube_face_basis` 逐项相同。
pub(crate) fn cube_face_basis(face: u32) -> ([f32; 3], [f32; 3], [f32; 3]) {
    match face {
        0 => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        1 => ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        2 => ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        3 => ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        4 => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        _ => ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    }
}

/// 预过滤环境 + BRDF 表。按需 `prepare`，天空设置不变时零开销。
pub(crate) struct EnvMapStage {
    sampler: wgpu::Sampler,
    sky_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    params_stride: u64,
    prefilter_pipeline: wgpu::RenderPipeline,
    prefilter_bind: wgpu::BindGroup,
    brdf_pipeline: wgpu::RenderPipeline,
    env: wgpu::Texture,
    env_view: wgpu::TextureView,
    level_views: Vec<wgpu::TextureView>,
    brdf: wgpu::Texture,
    brdf_view: wgpu::TextureView,
    brdf_sized: bool,
    built_key: Option<u64>,
}

impl EnvMapStage {
    pub(crate) fn new(context: &GpuContext) -> Self {
        let device = context.device();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-env-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sky_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-env-sky-uniform"),
            size: SKY_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // 30 个 (level, face) 各一份参数，按设备的 uniform 偏移对齐要求分槽 ——
        // 与旧端一样用 dynamic offset，不为每个面各建一张缓冲与绑定组。
        let alignment = device.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let params_stride = PREFILTER_PARAMS_BYTES.div_ceil(alignment) * alignment;
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-env-prefilter-params"),
            size: params_stride * u64::from(ENV_MIPS) * 6,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xgc-env"),
            source: wgpu::ShaderSource::Wgsl(ENV_WGSL.into()),
        });
        let prefilter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xgc-env-prefilter-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(SKY_UNIFORM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PREFILTER_PARAMS_BYTES),
                    },
                    count: None,
                },
            ],
        });
        let prefilter_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-env-prefilter-bind"),
            layout: &prefilter_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sky_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(PREFILTER_PARAMS_BYTES),
                    }),
                },
            ],
        });
        let prefilter_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xgc-env-prefilter-pipeline-layout"),
            bind_group_layouts: &[&prefilter_layout],
            push_constant_ranges: &[],
        });
        let prefilter_pipeline = env_pipeline(device, &shader, &prefilter_layout, "fs_prefilter");
        // BRDF 表只读屏幕 uv，不需要任何绑定。
        let brdf_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xgc-env-brdf-pipeline-layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let brdf_pipeline = env_pipeline(device, &shader, &brdf_layout, "fs_brdf");

        let env = env_texture(device, "xgc-env-cube");
        let env_view = env.create_view(&wgpu::TextureViewDescriptor {
            label: Some("xgc-env-cube-view"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            mip_level_count: Some(ENV_MIPS),
            array_layer_count: Some(6),
            ..Default::default()
        });
        let mut level_views = Vec::with_capacity((ENV_MIPS * 6) as usize);
        for level in 0..ENV_MIPS {
            for face in 0..6 {
                level_views.push(env.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("xgc-env-cube-face"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    base_array_layer: face,
                    array_layer_count: Some(1),
                    ..Default::default()
                }));
            }
        }
        let brdf = brdf_texture(device);
        let brdf_view = brdf.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            sampler,
            sky_buffer,
            params_buffer,
            params_stride,
            prefilter_pipeline,
            prefilter_bind,
            brdf_pipeline,
            env,
            env_view,
            level_views,
            brdf,
            brdf_view,
            brdf_sized: false,
            built_key: None,
        }
    }

    /// 天空设置变了就重建立方体与 BRDF 表；返回本帧跑的 pass 数（未重建 = 0）。
    pub(crate) fn prepare(
        &mut self,
        context: &GpuContext,
        settings: &SkySettings,
        sun: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        if !settings.enabled {
            return 0;
        }
        let uniform = SkyUniform::build(settings, sun);
        let mut sky_bytes = Vec::with_capacity(SKY_UNIFORM_BYTES as usize);
        uniform.write(&mut sky_bytes);
        let key = fnv1a(&sky_bytes);
        if self.built_key == Some(key) {
            return 0;
        }

        let device = context.device();
        let queue = context.queue();
        queue.write_buffer(&self.sky_buffer, 0, &sky_bytes);

        // 30 槽参数一次铺满：面基向量 + 该级粗糙度 + 采样数。
        let mut staging = vec![0u8; (self.params_stride * u64::from(ENV_MIPS) * 6) as usize];
        for level in 0..ENV_MIPS {
            let roughness = level as f32 / (ENV_MIPS - 1) as f32;
            let samples = prefilter_samples(level);
            for face in 0..6 {
                let (forward, right, up) = cube_face_basis(face);
                let slot = (level * 6 + face) as usize;
                let offset = slot * self.params_stride as usize;
                let fields = [
                    [forward[0], forward[1], forward[2], 0.0],
                    [right[0], right[1], right[2], roughness],
                    [up[0], up[1], up[2], samples as f32],
                ];
                for (index, value) in fields.into_iter().enumerate() {
                    for (axis, channel) in value.into_iter().enumerate() {
                        let at = offset + index * 16 + axis * 4;
                        staging[at..at + 4].copy_from_slice(&channel.to_le_bytes());
                    }
                }
            }
        }
        queue.write_buffer(&self.params_buffer, 0, &staging);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xgc-env-encoder"),
        });
        let mut passes = 0usize;
        for (slot, view) in self.level_views.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-env-prefilter-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            pass.set_pipeline(&self.prefilter_pipeline);
            let offset = (slot as u64 * self.params_stride) as u32;
            pass.set_bind_group(0, &self.prefilter_bind, &[offset]);
            pass.draw(0..3, 0..1);
            passes += 1;
        }
        if !self.brdf_sized {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-env-brdf-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.brdf_view,
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
            pass.set_pipeline(&self.brdf_pipeline);
            pass.draw(0..3, 0..1);
            passes += 1;
            self.brdf_sized = true;
        }
        queue.submit(Some(encoder.finish()));
        self.built_key = Some(key);
        passes
    }

    /// 采样端要的三件：Cube 视图 / BRDF 表视图 / 线性 Clamp 采样器。
    ///
    /// 顺带把两张纹理的规格钉一次：视图是从它们派生出来的，纹理本身必须由
    /// `EnvMapStage` 持有（释放了视图就悬空），断言同时说明"持有的是什么"。
    pub(crate) fn views(&self) -> (&wgpu::TextureView, &wgpu::TextureView, &wgpu::Sampler) {
        debug_assert_eq!(
            self.env.mip_level_count(),
            ENV_MIPS,
            "环境立方体级数必须与着色器常量一致"
        );
        debug_assert_eq!(self.env.depth_or_array_layers(), 6, "立方体必须有六层");
        debug_assert_eq!(
            self.brdf.width(),
            BRDF_LUT_SIZE,
            "BRDF 表边长必须与采样端约定一致"
        );
        (&self.env_view, &self.brdf_view, &self.sampler)
    }
}

fn env_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    entry: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("xgc-env-pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn env_texture(device: &wgpu::Device, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: ENV_BASE_SIZE,
            height: ENV_BASE_SIZE,
            depth_or_array_layers: 6,
        },
        mip_level_count: ENV_MIPS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn brdf_texture(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("xgc-env-brdf-lut"),
        size: wgpu::Extent3d {
            width: BRDF_LUT_SIZE,
            height: BRDF_LUT_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// FNV-1a：只需要"变了没有"，不需要抗碰撞（与旧端同一算法）。
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    // 六面中心方向：uv = (0.5, 0.5) 时 forward + right·0 + up·0 归一化就是 forward。
    #[test]
    fn cube_faces_cover_the_six_axes_in_webgpu_order() {
        let expected = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        for (face, want) in expected.into_iter().enumerate() {
            let (forward, right, up) = cube_face_basis(face as u32);
            assert!((0..3).all(|axis| (forward[axis] - want[axis]).abs() < 1e-6));
            // right / up 必须与 forward 正交（否则面内方向会斜）。
            let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            assert!(
                dot(forward, right).abs() < 1e-6,
                "第 {face} 面 right 必须与 forward 正交"
            );
            assert!(
                dot(forward, up).abs() < 1e-6,
                "第 {face} 面 up 必须与 forward 正交"
            );
        }
    }

    #[test]
    fn prefilter_samples_follow_the_old_engine_table() {
        let table = [96, 48, 24, 16, 12];
        for (level, want) in table.into_iter().enumerate() {
            assert_eq!(prefilter_samples(level as u32), want);
        }
        // 越界也必须有确定值（`level` 只来自 0..ENV_MIPS，这里只是挡未来改动）。
        assert_eq!(prefilter_samples(ENV_MIPS), 12);
    }
}
