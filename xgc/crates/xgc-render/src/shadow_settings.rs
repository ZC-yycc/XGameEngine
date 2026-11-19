//! 方向光阴影的**画质**参数（软阴影 / 级间混合），对应旧端 `project.ron` 的
//! `render.shadow` 节。
//!
//! **默认关**（两个 `0.0`）：与光晕 / 天空 / FXAA / 曝光同一条纪律 —— 画质增强
//! 默认不动画面。关着时着色器走 3×3 PCF + 硬级间切换的老路径，逐位一致。

/// 阴影画质：软阴影尺寸 + 级间混合带宽。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowSettings {
    /// 软阴影（PCSS）的灯光相对尺寸。
    ///
    /// `0.0`（默认）= 硬阴影：固定 3×3 PCF，与引入 PCSS 之前逐位一致；`> 0` =
    /// 先在盘里找遮挡物估出半影宽度，再按宽度做软化 PCF —— 贴着遮挡物的影子硬、
    /// 离得远的影子糊，这是固定核做不到的。
    pub softness: f32,
    /// 级间混合带宽（占该级远端切分距离的比例）。`0.0`（默认）= 两级硬接；
    /// `> 0` 时只改切分前一条薄带里的亮度过渡，把缝抹平。
    pub cascade_blend: f32,
}

impl ShadowSettings {
    /// 与默认值整体相等 = 关（旧端 `ShadowConfig::is_default` 同口径）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 是否有任何一项开着：决定着色器走新路径还是老的硬阴影路径。
    pub fn is_enabled(&self) -> bool {
        self.softness > 0.0 || self.cascade_blend > 0.0
    }

    /// 容错读取：`project.ron` 是**手改的文本**，负数 / NaN / 无穷不能直接交给
    /// 着色器（负半径会让 PCF 的采样点绕到反方向，NaN 会让整片影子消失）。
    ///
    /// 非有限值与负值一律当 0（= 该子项关），**不夹上界** —— 上界没有物理意义，
    /// 太大了只会糊成一片，不该由引擎替作者决定（与旧端 `ShadowConfig::sanitized`
    /// 逐项同口径）。
    pub fn sanitized(self) -> Self {
        let clean = |value: f32| {
            if value.is_finite() && value > 0.0 {
                value
            } else {
                0.0
            }
        };
        Self {
            softness: clean(self.softness),
            cascade_blend: clean(self.cascade_blend),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off() {
        let settings = ShadowSettings::default();
        assert_eq!(settings.softness, 0.0);
        assert_eq!(settings.cascade_blend, 0.0);
        assert!(settings.is_default());
        assert!(!settings.is_enabled());
    }

    #[test]
    fn illegal_values_fall_back_to_off() {
        // 旧端 `ShadowConfig::sanitized` 的单测逐条对过来：NaN / 无穷 / 负数
        // 都必须是"关"，而不是把 NaN 灌进着色器的盘半径。
        let garbage = ShadowSettings {
            softness: f32::NAN,
            cascade_blend: f32::NEG_INFINITY,
        }
        .sanitized();
        assert_eq!(garbage, ShadowSettings::default());
        let negative = ShadowSettings {
            softness: -0.5,
            cascade_blend: -0.1,
        }
        .sanitized();
        assert_eq!(negative, ShadowSettings::default());
        assert!(!negative.is_enabled());
    }

    #[test]
    fn legal_values_pass_through_without_an_upper_clamp() {
        let settings = ShadowSettings {
            softness: 0.5,
            cascade_blend: 0.05,
        }
        .sanitized();
        assert_eq!(settings.softness, 0.5);
        assert_eq!(settings.cascade_blend, 0.05);
        assert!(!settings.is_default());
        assert!(settings.is_enabled());
        // 上界刻意不夹：旧端认为"糊成一片"是作者的自由。
        assert_eq!(
            ShadowSettings {
                softness: 12.0,
                cascade_blend: 3.0,
            }
            .sanitized()
            .softness,
            12.0
        );
    }
}
