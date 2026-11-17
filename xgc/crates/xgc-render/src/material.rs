//! `.mat` 材质资产：只解析渲染用得上的字段。
//!
//! 旧引擎的叠加顺序是「模型材质 → `MaterialAsset` → 逐实体覆盖」：基色都是
//! **乘子**，metallic / roughness 是**替换**。XGC 目前只消费基色乘子，
//! metallic / roughness 先原样解析出来（BRDF 切片接上时直接用），不做二次解释。

use serde::Deserialize;

/// 一份共享材质（对应磁盘上的 `Resources/Materials/*.mat`）。
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MaterialAsset {
    /// 基础色乘子（线性 RGBA）；缺省白色 = 不改模型原色。
    #[serde(default = "white")]
    pub base_color: [f32; 4],
    /// 金属度（0 = 绝缘体，1 = 金属）；后续 BRDF 切片消费。
    #[serde(default)]
    pub metallic: f32,
    /// 粗糙度（0 = 镜面，1 = 完全漫反射）；后续 BRDF 切片消费。
    #[serde(default = "default_roughness")]
    pub roughness: f32,
}

fn white() -> [f32; 4] {
    [1.0; 4]
}

fn default_roughness() -> f32 {
    0.45
}

impl Default for MaterialAsset {
    fn default() -> Self {
        Self {
            base_color: white(),
            metallic: 0.0,
            roughness: default_roughness(),
        }
    }
}

impl MaterialAsset {
    /// 解析 `.mat` 文本（RON）。坏文本返回错误，由调用方决定降级策略。
    pub fn from_ron(text: &str) -> Result<Self, String> {
        ron::from_str(text).map_err(|error| format!("{error}"))
    }

    /// 加载边界清洗：NaN / 无穷换成缺省值，金属度 / 粗糙度夹到 0..1。
    ///
    /// 与旧引擎 `MaterialAsset::sanitized` 同一职责——坏数据不进 GPU；
    /// 存盘数据本身不在这里改写。
    pub fn sanitized(mut self) -> Self {
        for channel in &mut self.base_color {
            if !channel.is_finite() {
                *channel = 1.0;
            }
        }
        if !self.metallic.is_finite() {
            self.metallic = 0.0;
        }
        if !self.roughness.is_finite() {
            self.roughness = default_roughness();
        }
        self.metallic = self.metallic.clamp(0.0, 1.0);
        self.roughness = self.roughness.clamp(0.0, 1.0);
        self
    }
}
