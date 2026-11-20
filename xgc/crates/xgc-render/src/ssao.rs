//! SSAO（屏幕空间环境光遮蔽）：半分辨率深度遮蔽 + 可分离模糊 + 乘回场景色。
//!
//! # 在管线里的位置
//!
//! ```text
//! 场景 pass（HDR + 深度）→ [TAA 解析] → 【SSAO 乘算】→ 光晕 → tone-map → UI
//! ```
//!
//! AO 在半分辨率上算完、模糊、再用线性过滤上采样乘到全分辨率场景色上，
//! 所以这里与 [`crate::bloom`] 一样要自己持一张全分辨率 `Rgba16Float`
//! scratch：读写同一张纹理是 wgpu 明令禁止的，不能就地改场景色。
//!
//! # 为什么只吃深度
//!
//! 没有 G-buffer，法线由深度梯度重建（每轴取深度差更小的一侧，跨轮廓不把
//! 远处表面拉进来）。代价写清楚：平滑曲面上法线有梯度噪声（靠半分辨率 +
//! 模糊压掉），遮蔽是屏幕空间的（遮挡物移出屏幕贡献当场消失）。这是旧端
//! `crates/xgame-render/src/ssao.rs` 的同一取舍，换 G-buffer 之前无解。
//!
//! # 确定性噪声
//!
//! kernel 旋转角来自**像素坐标的整数哈希**（WGSL 的 `hash_angle`），没有噪声
//! 纹理、没有时间项、没有随机种子进 GPU；半球 kernel 由固定种子 LCG 在构建
//! 时生成一次。同机同输入两次渲染逐字节一致，`render-frame` 的可复现契约在
//! SSAO 开着时也成立（有 GPU 测试钉住）。
//!
//! # 关闭 = 零成本
//!
//! `enabled == false` 或 `strength <= 0` 时宿主整段跳过：不建 AO 纹理、不跑
//! pass、不写 uniform；关掉时把目标整份释放（只留管线与 uniform），画面与
//! 上线前逐位一致。

use std::sync::Arc;

use super::camera::{invert, Mat4, IDENTITY};
use super::gpu::GpuContext;

/// 半球 kernel 的采样数。16 是"质量够、每像素开销看得住"的常用档。
pub const KERNEL_SAMPLES: usize = 16;

/// 全部 pass 共享的 uniform 字节数（与 `ssao.wgsl` 的 `SsaoParams` 逐字节对齐）。
pub(crate) const PARAMS_BYTES: usize = 448;

const SSAO_WGSL: &str = include_str!("ssao.wgsl");
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

mod settings;

pub use settings::SsaoSettings;

/// 固定种子 LCG 生成半球 kernel（同一构建在任何机器上都是同一组向量）。
///
/// 与旧端 `hemisphere_kernel` 逐运算相同：种子 / 乘加常量写死，向 +z 半球
/// 取点，再按二次分布向原点聚拢（近处样本密，遮蔽边缘的渐变更稳）。
fn hemisphere_kernel() -> [[f32; 4]; KERNEL_SAMPLES] {
    let mut state: u32 = 0x2545_F491;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };
    let mut out = [[0.0f32; 4]; KERNEL_SAMPLES];
    for (index, slot) in out.iter_mut().enumerate() {
        let x = (next() as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let y = (next() as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let z = next() as f32 / u32::MAX as f32;
        let length = (x * x + y * y + z * z).sqrt();
        // 旧端用 `normalize_or_zero`：退化成长度 0 时直接取 +Z。
        let (mut vx, mut vy, mut vz) = (0.0, 0.0, 1.0);
        if length > 0.0 {
            vx = x / length;
            vy = y / length;
            vz = z / length;
        }
        let t = (index as f32 + 1.0) / KERNEL_SAMPLES as f32;
        let scale = 0.1 + 0.9 * t * t;
        *slot = [vx * scale, vy * scale, vz * scale, 0.0];
    }
    out
}

/// 只有「可逆且所有元素有限」的视图投影才允许真正乘遮蔽：`camera::invert`
/// 只挡奇异矩阵（NaN / Inf 的行列式比较为假），旧端 `determinant().is_finite()`
/// 这道守卫不能省，否则 `fs_ao` 的遮蔽值可能变成 NaN 再乘到源色上。退化时
/// 强度按 0 写进 uniform：AO 全 1，乘算等于抄源色，画面逐位不变。
fn invertible_view_projection(view_projection: Mat4) -> bool {
    view_projection.iter().all(|value| value.is_finite())
        && invert(view_projection)
            .is_some_and(|inverse| inverse.iter().all(|value| value.is_finite()))
}

/// 打包 `SsaoParams`（448 B）。顺序与 WGSL 字段顺序逐项一致。
#[allow(clippy::too_many_arguments)]
pub(crate) fn params_bytes(
    inverse_view_projection: Mat4,
    view_projection: Mat4,
    camera_position: [f32; 3],
    kernel: &[[f32; 4]; KERNEL_SAMPLES],
    radius: f32,
    bias: f32,
    strength: f32,
    size: (u32, u32),
    half: (u32, u32),
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PARAMS_BYTES);
    let mut push = |values: &[f32]| {
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    };
    push(&inverse_view_projection);
    push(&view_projection);
    push(&[
        camera_position[0],
        camera_position[1],
        camera_position[2],
        1.0,
    ]);
    for sample in kernel {
        push(sample);
    }
    push(&[radius, bias, strength, 0.0]);
    push(&[
        size.0 as f32,
        size.1 as f32,
        1.0 / size.0 as f32,
        1.0 / size.1 as f32,
    ]);
    push(&[
        half.0 as f32,
        half.1 as f32,
        1.0 / half.0 as f32,
        1.0 / half.1 as f32,
    ]);
    debug_assert_eq!(bytes.len(), PARAMS_BYTES);
    bytes
}

/// 四支全屏管线（共用同一个着色器模块，靠入口名区分）。
struct Pipelines {
    ao: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    apply: wgpu::RenderPipeline,
}

/// 半分辨率 AO 乒乓（模糊要两张）+ 全分辨率乘算 scratch。
struct Targets {
    size: (u32, u32),
    half: (u32, u32),
    ao_views: [wgpu::TextureView; 2],
    _ao_textures: [wgpu::Texture; 2],
    scratch: Arc<wgpu::Texture>,
    scratch_view: wgpu::TextureView,
}

/// SSAO 的 GPU 状态。与 TAA / 光晕同款：AO 依赖各自的深度与目标，三宿主各持一份。
pub(crate) struct SsaoStage {
    /// 场景 pass 的采样数：深度附件跟着它决定是否多采样（绑定类型与着色器声明都得换）。
    depth_sample_count: u32,
    pipelines: Option<Pipelines>,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    targets: Option<Targets>,
    kernel: [[f32; 4]; KERNEL_SAMPLES],
}

impl SsaoStage {
    pub(crate) fn new(context: &GpuContext) -> Self {
        let sampler = context.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-ssao-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-ssao-params"),
            size: PARAMS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            depth_sample_count: context.scene_sample_count(),
            pipelines: None,
            uniform,
            sampler,
            targets: None,
            kernel: hemisphere_kernel(),
        }
    }

    /// 关掉时把 AO / scratch 整份丢掉（管线与 uniform 留着避免重编译）。
    pub(crate) fn release(&mut self) {
        self.targets = None;
    }

    /// 当前 AO 目标的尺寸（诊断 / 回归：换尺寸重建、关掉即释放）。
    pub(crate) fn target_size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|targets| targets.size)
    }

    /// 记录 SSAO 四趟 pass，返回**乘算输出**与 pass 数。
    ///
    /// `view_projection` 用未抖动的那个（TAA 的抖动只影响深度，反投影误差
    /// 在半像素内，远小于采样半径）。矩阵退化时强度按 0 写进 uniform：
    /// AO 全部为 1，乘算等于把源色抄进 scratch，画面逐位不变。
    pub(crate) fn render(
        &mut self,
        context: &GpuContext,
        source: &Arc<wgpu::Texture>,
        depth: &wgpu::TextureView,
        view_projection: Mat4,
        camera_position: [f32; 3],
        settings: SsaoSettings,
    ) -> (Arc<wgpu::Texture>, usize) {
        let size = (source.width().max(1), source.height().max(1));
        self.ensure_pipelines(context);
        self.ensure_targets(context, size);

        let invertible = invertible_view_projection(view_projection);
        let inverse = if invertible {
            invert(view_projection).unwrap_or(IDENTITY)
        } else {
            IDENTITY
        };
        let strength = if invertible { settings.strength } else { 0.0 };
        let pipelines = self.pipelines.as_ref().expect("刚确保过管线");
        let targets = self.targets.as_ref().expect("刚确保过目标");
        let params = params_bytes(
            inverse,
            view_projection,
            camera_position,
            &self.kernel,
            settings.radius,
            settings.bias,
            strength,
            size,
            targets.half,
        );
        context.queue().write_buffer(&self.uniform, 0, &params);
        let device = context.device();
        let uniform = self.uniform.as_entire_binding();
        let ao_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-ssao-ao-bind"),
            layout: &pipelines.ao.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: uniform.clone(),
                },
            ],
        });
        let blur = |label: &str, layout: &wgpu::BindGroupLayout, input: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: uniform.clone(),
                    },
                ],
            })
        };
        let blur_h_bind = blur(
            "xgc-ssao-blur-h-bind",
            &pipelines.blur_h.get_bind_group_layout(0),
            &targets.ao_views[0],
        );
        let blur_v_bind = blur(
            "xgc-ssao-blur-v-bind",
            &pipelines.blur_v.get_bind_group_layout(0),
            &targets.ao_views[1],
        );
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let apply_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-ssao-apply-bind"),
            layout: &pipelines.apply.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&targets.ao_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: uniform,
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xgc-ssao-encoder"),
        });
        let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                          label: &str,
                          pipeline: &wgpu::RenderPipeline,
                          bind: &wgpu::BindGroup,
                          view: &wgpu::TextureView,
                          clear: wgpu::Color| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        };
        fullscreen(
            &mut encoder,
            "xgc-ssao-ao-pass",
            &pipelines.ao,
            &ao_bind,
            &targets.ao_views[0],
            wgpu::Color::WHITE,
        );
        fullscreen(
            &mut encoder,
            "xgc-ssao-blur-h-pass",
            &pipelines.blur_h,
            &blur_h_bind,
            &targets.ao_views[1],
            wgpu::Color::WHITE,
        );
        fullscreen(
            &mut encoder,
            "xgc-ssao-blur-v-pass",
            &pipelines.blur_v,
            &blur_v_bind,
            &targets.ao_views[0],
            wgpu::Color::WHITE,
        );
        fullscreen(
            &mut encoder,
            "xgc-ssao-apply-pass",
            &pipelines.apply,
            &apply_bind,
            &targets.scratch_view,
            wgpu::Color::BLACK,
        );
        context.queue().submit(Some(encoder.finish()));
        (targets.scratch.clone(), 4)
    }

    /// 场景采样数变了（`Renderer::set_sample_count`）：深度绑定与着色器声明要换，
    /// 已有的管线整份作废，下次 `ensure_pipelines` 按新采样数重建。
    pub(crate) fn set_sample_count(&mut self, sample_count: u32) {
        if self.depth_sample_count == sample_count {
            return;
        }
        self.depth_sample_count = sample_count;
        self.pipelines = None;
    }

    fn ensure_pipelines(&mut self, context: &GpuContext) {
        if self.pipelines.is_some() {
            return;
        }
        let device = context.device();
        // 场景 pass 开 MSAA 时深度附件是多采样纹理：声明换
        // `texture_depth_multisampled_2d`，`textureLoad(depth, pixel, 0)` 第三参
        // 在多采样下就是 sample index（旧端同样在源码里替换这一行）。
        let depth_sample_count = self.depth_sample_count;
        let source = if depth_sample_count > 1 {
            SSAO_WGSL.replace("texture_depth_2d", "texture_depth_multisampled_2d")
        } else {
            SSAO_WGSL.to_string()
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xgc-ssao"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(PARAMS_BYTES as u64),
            },
            count: None,
        };
        let depth = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: depth_sample_count > 1,
            },
            count: None,
        };
        let sampler = wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let ao_layout = layout("xgc-ssao-ao-layout", &[depth, uniform]);
        let blur_layout = layout("xgc-ssao-blur-layout", &[texture(1), uniform]);
        let apply_layout = layout(
            "xgc-ssao-apply-layout",
            &[texture(1), texture(2), sampler, uniform],
        );
        let make = |label: &str, entry: &str, layout: &wgpu::BindGroupLayout, format| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[layout],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let ao = make(
            "xgc-ssao-ao",
            "fs_ao",
            &ao_layout,
            wgpu::TextureFormat::R8Unorm,
        );
        let blur_h = make(
            "xgc-ssao-blur-h",
            "fs_blur_h",
            &blur_layout,
            wgpu::TextureFormat::R8Unorm,
        );
        let blur_v = make(
            "xgc-ssao-blur-v",
            "fs_blur_v",
            &blur_layout,
            wgpu::TextureFormat::R8Unorm,
        );
        let apply = make("xgc-ssao-apply", "fs_apply", &apply_layout, HDR_FORMAT);
        self.pipelines = Some(Pipelines {
            ao,
            blur_h,
            blur_v,
            apply,
        });
    }

    fn ensure_targets(&mut self, context: &GpuContext, size: (u32, u32)) {
        if self.targets.as_ref().map(|targets| targets.size) == Some(size) {
            return;
        }
        let device = context.device();
        let half = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let make_ao = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: half.0,
                    height: half.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let ao_textures = [make_ao("xgc-ssao-ao-a"), make_ao("xgc-ssao-ao-b")];
        let ao_views = [
            ao_textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            ao_textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        let scratch = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-ssao-scratch"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }));
        let scratch_view = scratch.create_view(&wgpu::TextureViewDescriptor::default());
        self.targets = Some(Targets {
            size,
            half,
            ao_views,
            _ao_textures: ao_textures,
            scratch,
            scratch_view,
        });
    }
}

#[cfg(test)]
mod tests;
