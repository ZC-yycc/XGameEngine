//! `render.ssao` 的设置值：默认关、消毒口径与旧端 `SsaoConfig` 同式。
//!
//! 单拆一个文件只为 `ssao.rs` 的行数预算；这里没有任何 GPU 状态。

/// 画面设置里的 SSAO 节：默认关，与 Bloom / TAA / FXAA 同一条纪律。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsaoSettings {
    /// 是否启用。关着时渲染器整段跳过，画面逐位不变。
    pub enabled: bool,
    /// 遮蔽强度（0 = 无遮蔽效果；上限见 [`Self::MAX_STRENGTH`]）。
    pub strength: f32,
    /// 采样半径（世界单位）。太大 = 远处物体也遮蔽近处；太小 = 看不出。
    pub radius: f32,
    /// 深度比较偏置（世界单位）：挡掉"表面遮蔽自己"的 acne。
    pub bias: f32,
}

impl Default for SsaoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 1.0,
            radius: 0.5,
            bias: 0.025,
        }
    }
}

impl SsaoSettings {
    /// 遮蔽强度上限：再大只是把接触阴影压成死黑，画面信息全丢。
    pub const MAX_STRENGTH: f32 = 4.0;
    /// 采样半径上限（世界单位）：10 米的遮蔽半径已经不是"接触阴影"了。
    pub const MAX_RADIUS: f32 = 10.0;
    /// 偏置上限：偏置比半径还大时遮蔽整体失效，但那也是合法的"关"法。
    pub const MAX_BIAS: f32 = 1.0;

    /// 开着且强度为正才算数：强度 0 与关掉等价（省下四趟全屏 pass）。
    pub fn is_enabled(&self) -> bool {
        self.enabled && self.strength > 0.0
    }

    /// 与默认值完全一致（供回归与将来落盘用的 `skip_serializing_if` 口径比较；
    /// 目前 CLI 只读不写画面设置）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 容错读取：非有限字段落回默认，合法但离谱的量级夹进安全范围（旧端
    /// `SsaoConfig::sanitized` 同式）。
    pub fn sanitized(self) -> Self {
        let clean = |value: f32, default: f32, max: f32| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                default
            }
        };
        let default = Self::default();
        Self {
            enabled: self.enabled,
            strength: clean(self.strength, default.strength, Self::MAX_STRENGTH),
            radius: clean(self.radius, default.radius, Self::MAX_RADIUS),
            bias: clean(self.bias, default.bias, Self::MAX_BIAS),
        }
    }
}
