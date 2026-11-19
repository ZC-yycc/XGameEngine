//! 画面设置入口：把 `project.ron` 读出来的各节容错后灌进 `Renderer`。
//!
//! 这些 setter 单独成文件是为了让 `renderer.rs` 守住单文件软上限；它们只碰字段、
//! 不参与渲染流程，读代码时可以先跳过。每个 setter 的"为什么"写在各自 doc 里。

use crate::bloom::BloomSettings;
use crate::exposure::sanitize_exposure;
use crate::fxaa::FxaaSettings;
use crate::shadow_settings::ShadowSettings;
use crate::sky::SkySettings;
use crate::taa::TaaSettings;

impl super::Renderer {
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
}
