//! 主场景帧 uniform：光照 / 天空 / 阴影参数 + 定长光源表。
//!
//! 从 `instanced.rs` 按行数预算拆出：实例化绘制只关心"怎么画"，这里关心
//! "每帧往 GPU 送什么"。字段顺序与 `scene_shading.wgsl` 的 `Frame` 一一对应，
//! 改这里必须同步 WGSL（`write` 末尾的 `debug_assert_eq!` 兜住字节数）。

use super::camera::Mat4;
use super::gpu::GpuContext;
use super::lights::{self, GpuLight, MAX_LIGHTS};
use super::point_shadow::{PointShadow, POINT_SHADOW_UNIFORM_BYTES};
use super::shadow::{ShadowCascades, SHADOW_CASCADES, SHADOW_DEPTH_BIAS};
use xgc_world::SceneEnvironment;

/// 帧 uniform 布局：view-proj 64B、相机 / ambient 2 个 vec4、9 个 vec4 的天空球谐、
/// 天空开关 vec4、4 级光视图投影 256B、切分 / 视线 / 参数 / 阴影画质 4 个 vec4、
/// 光源盏数 vec4、8 盏灯各 48B（共 976B），再加点光立方体阴影段（光位置 + 参数，
/// 32B）共 1008B。六面视图投影只在阴影 pass 的每面 64B 帧缓冲里，不进这里。
///
/// 光源本身只在 `lights[]` 里出现一次：以前另有 sun_dir / sun_color 两个 vec4，
/// 多光源之后它们会与 `lights[0]` 重复且可能不一致，已删。
pub(crate) const FRAME_UNIFORM_BYTES: u64 =
    576 + 16 + (MAX_LIGHTS as u64) * lights::LIGHT_UNIFORM_BYTES + POINT_SHADOW_UNIFORM_BYTES;

/// 帧级光照参数：环境光 + 全部可用光源（`lights` / `light_count` 进 uniform）。
///
/// `sun_*` 三个字段不进 uniform，只给 CPU 侧消费：天空太阳盘与自动取景的顺光需要
/// "主方向光"这一个标量摘要。着色永远走 `lights[]`，所以两者不会各自漂移。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameLighting {
    pub(crate) eye: [f32; 3],
    pub(crate) ambient: [f32; 3],
    pub(crate) sun_direction: [f32; 3],
    pub(crate) sun_intensity: f32,
    pub(crate) sun_color: [f32; 3],
    /// 天空漫反射环境（E/π 口径的 L2 球谐）；`None` = 天空关，只剩 `ambient`。
    pub(crate) sky_irradiance: Option<[[f32; 4]; 9]>,
    /// 4 级方向光级联；`None` = 这一帧没有可用阴影。
    pub(crate) shadow_cascades: Option<ShadowCascades>,
    /// 软阴影（PCSS）的灯光尺寸；0 = 硬阴影（与引入 PCSS 之前逐位一致）。
    pub(crate) shadow_softness: f32,
    /// 级间混合带宽占该级跨度的比例；0 = 两级硬接。
    pub(crate) shadow_cascade_blend: f32,
    /// 本帧参与着色的光源（与旧端 `LightsUniform` 同布局），空位留 0。
    pub(crate) lights: [GpuLight; MAX_LIGHTS],
    /// 有效光源数；着色器按它提前退出循环。
    pub(crate) light_count: u32,
    /// 本帧投射立方体阴影的那盏点光；`None` = 关（uniform 段写 0，着色器整段跳过）。
    pub(crate) point_shadow: Option<PointShadow>,
}

impl FrameLighting {
    pub(crate) fn from_environment(environment: &SceneEnvironment, eye: [f32; 3]) -> Self {
        let sun = environment.sun();
        let packed = lights::pack(environment);
        Self {
            eye,
            ambient: environment.ambient,
            sun_direction: sun
                .map(|light| light.normalized_direction())
                .unwrap_or([0.0, -1.0, 0.0]),
            sun_intensity: sun.map(|light| light.intensity).unwrap_or(0.0),
            sun_color: sun.map(|light| light.color).unwrap_or([1.0; 3]),
            sky_irradiance: None,
            shadow_cascades: None,
            shadow_softness: 0.0,
            shadow_cascade_blend: 0.0,
            lights: packed.lights,
            light_count: packed.count,
            point_shadow: None,
        }
    }

    /// 挂上 4 级方向光级联（`None` = 不采样阴影）。
    pub(crate) fn with_shadow(mut self, cascades: Option<ShadowCascades>) -> Self {
        self.shadow_cascades = cascades;
        self
    }

    /// 挂上阴影画质参数（软阴影尺寸 + 级间混合带宽）：0 / 0 就是老路径。
    pub(crate) fn with_shadow_quality(mut self, softness: f32, cascade_blend: f32) -> Self {
        self.shadow_softness = softness;
        self.shadow_cascade_blend = cascade_blend;
        self
    }

    /// 挂上天空环境照明：`None` = 天空关（片元整段跳过 SH 求值）。
    pub(crate) fn with_sky(mut self, sky_irradiance: Option<[[f32; 4]; 9]>) -> Self {
        self.sky_irradiance = sky_irradiance;
        self
    }

    /// 挂上点光立方体阴影：`None` = 关，`shadow_params.z` 全为 0，与没做这个功能前逐位一致。
    pub(crate) fn with_point_shadow(mut self, point: Option<PointShadow>) -> Self {
        self.point_shadow = point;
        self
    }

    /// 按 WGSL `Frame` 的字段顺序打包；不足 16 字节的部分用 0 补齐。
    fn write(&self, view_proj: Mat4, bytes: &mut Vec<u8>) {
        for value in view_proj {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.eye[0], self.eye[1], self.eye[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.ambient[0], self.ambient[1], self.ambient[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let sky = self.sky_irradiance.unwrap_or([[0.0; 4]; 9]);
        for coefficient in sky {
            for value in coefficient {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let enabled: f32 = if self.sky_irradiance.is_some() {
            1.0
        } else {
            0.0
        };
        for value in [enabled, 0.0, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let cascades = self.shadow_cascades;
        for cascade in 0..SHADOW_CASCADES {
            let matrix = cascades
                .map(|c| c.view_proj[cascade])
                .unwrap_or(super::camera::IDENTITY);
            for value in matrix {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let mut splits = [0.0f32; SHADOW_CASCADES];
        let mut forward = [0.0f32; 3];
        let mut count = 0.0f32;
        if let Some(cascades) = cascades {
            splits = cascades.splits;
            forward = cascades.forward;
            count = cascades.count as f32;
        }
        for value in splits {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [forward[0], forward[1], forward[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let (shadow_enabled, shadow_bias) = match cascades {
            Some(_) => (1.0, SHADOW_DEPTH_BIAS),
            None => (0.0, 0.0),
        };
        for value in [shadow_enabled, shadow_bias, 0.0, count] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        // x = 软阴影尺寸，y = 级间混合带宽；z / w 留给以后（显式写 0 而不是留空洞）。
        for value in [self.shadow_softness, self.shadow_cascade_blend, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        // x = 有效光源数；y / z / w 预留（显式写 0，保持 16 字节对齐）。
        for value in [self.light_count as f32, 0.0, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for light in &self.lights {
            light.write(bytes);
        }
        match self.point_shadow {
            Some(point) => bytes.extend_from_slice(&point.uniform_bytes()),
            // 关着时写 0 而不是跳过：绑定组布局与 WGSL 结构长度都必须固定。
            None => bytes.resize(bytes.len() + POINT_SHADOW_UNIFORM_BYTES as usize, 0),
        }
        debug_assert_eq!(bytes.len(), FRAME_UNIFORM_BYTES as usize);
    }
}

pub(crate) fn create_frame_layout(context: &GpuContext, label: &str) -> wgpu::BindGroupLayout {
    context
        .device()
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(FRAME_UNIFORM_BYTES),
                    },
                    count: None,
                },
                // 方向光阴影深度图：硬件比较采样（比较采样器配 textureSampleCompareLevel）。
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                // 比较采样器：方向光阴影与点光立方体阴影共用（都是 textureSampleCompareLevel）。
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                // 点光立方体阴影深度图。
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        })
}

pub(crate) fn create_frame_bind(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    view_proj: Mat4,
    lighting: &FrameLighting,
    shadow_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
    point_shadow_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    let mut bytes = Vec::with_capacity(FRAME_UNIFORM_BYTES as usize);
    lighting.write(view_proj, &mut bytes);
    debug_assert_eq!(bytes.len(), FRAME_UNIFORM_BYTES as usize);
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("xgc-scene-frame-buffer"),
        size: FRAME_UNIFORM_BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    context.queue().write_buffer(&buffer, 0, &bytes);
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-scene-frame-bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(point_shadow_view),
                },
            ],
        })
}
