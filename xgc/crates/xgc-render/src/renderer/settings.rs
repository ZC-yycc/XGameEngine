//! 各宿主通过同一入口把项目设置交给渲染器。

use crate::{
    sanitize_exposure, BloomSettings, FxaaSettings, GpuContext, RenderQuality, RenderSettings,
    ShadowSettings, SkySettings, SsaoSettings, TaaSettings,
};

impl super::Renderer {
    pub fn apply_project_settings(
        &mut self,
        context: &GpuContext,
        render: RenderSettings,
        quality: RenderQuality,
    ) {
        let render = render.sanitized();
        let quality = quality.resolve(context.scene_sample_count());
        self.set_sample_count(quality.msaa);
        self.shadow_resolution = quality.shadow_resolution;
        self.lod_bias = render.lod_bias * quality.lod_scale;
        self.exposure = render.exposure;
        self.tone_map = render.tone_map;
        self.sky = render.sky;
        self.bloom = render.bloom;
        self.fxaa = render.fxaa;
        self.shadow_settings = render.shadow;
        self.taa = render.taa;
        self.ssao = render.ssao;
    }

    pub(super) fn scene_clear_color(&self) -> wgpu::Color {
        if self.sky.enabled {
            let [r, g, b] = self
                .sky
                .horizon
                .map(|value| f64::from(value * self.sky.intensity));
            wgpu::Color { r, g, b, a: 1.0 }
        } else {
            self.clear_color
        }
    }

    /// 设置乘到各级距离上的倍率（包含质量档位的缩放）。非正值固定使用首个可用级。
    pub fn set_lod_bias(&mut self, bias: f32) {
        self.lod_bias = bias;
    }

    /// 设置天空（等价于旧 `project.ron` 的 `render.sky`）。
    pub fn set_sky(&mut self, sky: SkySettings) {
        self.sky = sky.sanitized();
    }

    /// 设置曝光（等价于旧 `project.ron` 的 `render.exposure`）。
    ///
    /// 曝光是曲线之前的线性光乘子，必须与光晕亮部提取用同一个值：0 是
    /// "全黑画面"而不是关闭态，非正 / 非有限值由 [`sanitize_exposure`] 落回
    /// 1.0。缺省 1.0 时帧输出逐字节不变。
    pub fn set_exposure(&mut self, exposure: f32) {
        self.exposure = sanitize_exposure(exposure);
    }

    /// 设置光晕（等价于旧 `project.ron` 的 `render.bloom`）。
    ///
    /// 旧端把光晕放在曝光后、ACES 前：过完曲线的高光没有余量，显示端再叠加
    /// 只会被裁成硬边。`intensity <= 0` 是关闭态，此时整条链不跑也不会被采样。
    pub fn set_bloom(&mut self, bloom: BloomSettings) {
        self.bloom = bloom.sanitized();
    }

    /// 设置 FXAA（等价于旧 `project.ron` 的 `render.fxaa`）。
    ///
    /// 位置与旧端一致：tone-map 之后、UI 之前 —— 抗锯齿只收拾场景边缘，
    /// HUD 文字保持锐利。默认关，关着时整段跳过、帧输出逐字节不变。
    pub fn set_fxaa(&mut self, fxaa: FxaaSettings) {
        self.fxaa = fxaa;
    }

    /// 设置方向光阴影的画质（等价于旧 `project.ron` 的 `render.shadow`）。
    ///
    /// 软阴影（PCSS）与级间混合都是可选增强：两个 0 时着色器走 3×3 PCF + 硬级间
    /// 切换的老路径，帧输出逐字节不变；非有限 / 负值由
    /// [`ShadowSettings::sanitized`] 落回 0。
    pub fn set_shadow_settings(&mut self, settings: ShadowSettings) {
        self.shadow_settings = settings.sanitized();
    }

    /// 设置时间抗锯齿（等价于旧 `project.ron` 的 `render.taa`）。
    ///
    /// 默认关：关着时不建历史纹理、不跑解析 pass、相机不抖动，帧输出逐字节不变。
    /// 开着时抖动只叠相机（阴影拟合 / 剔除仍用未抖动矩阵），且 FXAA 让位。
    pub fn set_taa(&mut self, settings: TaaSettings) {
        self.taa = settings.sanitized();
    }

    /// SSAO 只看 `enabled` 与 `strength > 0`：强度 0 与关掉等价（省四趟 pass）。
    pub fn set_ssao(&mut self, settings: SsaoSettings) {
        self.ssao = settings.sanitized();
    }
}
