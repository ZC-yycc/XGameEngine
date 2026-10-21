//! 材质：资产（[`MaterialAsset`]）+ 逐实体覆盖（[`MaterialOverride`]）。
//!
//! # 为什么需要它
//!
//! 渲染侧的材质来自模型：glTF 文件自带 `baseColorFactor` / `metallicFactor` /
//! `roughnessFactor`，内置图元（`builtin://cube` 等）只有一份写死的默认值 ——
//! **白模**。于是"给这箱子换个颜色"这种事在场景层面无路可走，只能去改
//! glTF 源文件（或干脆接受一片白）。编辑器要能调、场景文件要能存、脚本要
//! 能改、智能体写 RON 时要有地方写 —— 这就是这个类型存在的理由。
//!
//! # 语义：字段级覆盖，`None` = 用模型自带值
//!
//! 不是"换一份材质"，而是在解析出的模型材质之上**逐字段**打补丁。
//! 这样 glTF 的贴图 / 面片材质照常参与（比如 Fox 的多材质身体），
//! 覆盖只影响它明确指定的那几个字段。
//!
//! 兼容策略与 `physics` / `animator` / `audio` 同款：旧场景文件没有
//! `material` 字段 → `None`，渲染行为与"材质覆盖功能上线前"逐位一致；
//! `skip_serializing_if` 让没配覆盖的实体在场景文件里不出现这一行。
//!
//! # 场景文件写法
//!
//! 与 `physics` / `audio` 等字段同款：RON 的 `Option` 显式写 `Some(...)`，
//! 未配置的字段整行省略（`skip_serializing_if`）：
//!
//! ```ron
//! // 给内置白模上色 + 调粗糙度；metallic 不写 = 用模型自带值
//! material: Some((base_color: Some((0.8, 0.25, 0.2, 1.0)), roughness: Some(0.6))),
//! ```
//!
//! # 取值约定
//!
//! - `base_color`：线性 RGBA 乘子（与 glTF `baseColorFactor` 同语义）。
//! - `metallic`：0 = 绝缘体，1 = 金属。
//! - `roughness`：0 = 镜面，1 = 完全漫反射。
//!
//! 渲染侧只在**使用点**夹取（见渲染管线的材质换算），这里存原值 ——
//! 存盘数据不替用户做"聪明"的修正，读出来还是写进去的那个数。

use serde::{Deserialize, Serialize};

/// 材质资产文件的扩展名（`Resources/Materials/Wood.mat`）。
pub const MATERIAL_EXTENSION: &str = "mat";

/// 一份可复用的材质资产（磁盘上的 `.mat`，RON 文本）。
///
/// # 与 [`MaterialOverride`] 的分工
///
/// `MaterialOverride` 是**某个实体的临时补丁**：只写想改的字段，存进场景文件。
/// `MaterialAsset` 是**项目里的共享材质**：一份文件、多个实体 / 多个场景引用，
/// 改一处全生效。渲染时的叠加顺序是：
///
/// ```text
/// 模型自带材质  →  MaterialAsset（若实体引用了材质资产）  →  MaterialOverride（若实体写了覆盖）
/// ```
///
/// 因此实体可以"用木材质的粗糙度，但这一个箱子染成红色"——两层各管各的。
///
/// # 字段语义
///
/// - `base_color`：线性 RGBA **乘子**（与 [`MaterialOverride::base_color`] 同义）。
///   默认白色 = 不改模型原色。
/// - `metallic` / `roughness`：**替换**模型的对应参数（它们是材料的身份，
///   乘一个系数没有直觉含义）。默认值取引擎内置图元那套"绝缘体 + 中等粗糙"。
/// - `albedo`：可选的漫反射贴图（项目相对路径）。**空串 = 用 1×1 白色**，
///   而不是保留模型自带贴图 —— 材质资产是一份完整材质，引用了就由它说了算，
///   否则"换材质"会变成"有时换、有时不换"的玄学。想保留贴图就别引用材质资产。
///
/// 贴图 / 颜色都不在这里夹取：渲染侧使用点统一夹（与覆盖同款策略），
/// 存盘数据保持用户写的原值。唯一的例外是 [`sanitized`]：它只处理
/// NaN / 无穷这种"必然画坏"的值，不做业务上的"聪明修正"。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialAsset {
    /// 基础色乘子（线性 RGBA）。
    #[serde(default = "white")]
    pub base_color: [f32; 4],
    /// 金属度（0 = 绝缘体，1 = 金属）。
    #[serde(default)]
    pub metallic: f32,
    /// 粗糙度（0 = 镜面，1 = 完全漫反射）。
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    /// 漫反射贴图的项目相对路径。空串 = 纯色。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub albedo: String,
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
            albedo: String::new(),
        }
    }
}

impl MaterialAsset {
    /// 解析 `.mat` 文本。
    pub fn from_ron(text: &str) -> Result<Self, String> {
        ron::from_str(text).map_err(|e| format!("{e}"))
    }

    /// 序列化为 `.mat` 文本（带换行，方便 diff / 手改）。
    pub fn to_ron(&self) -> Result<String, String> {
        let mut text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| format!("{e}"))?;
        text.push('\n');
        Ok(text)
    }

    /// 把必然画坏的数值（NaN / 无穷）换成默认值，并夹住金属度 / 粗糙度。
    ///
    /// 在**加载边界**调用（[`ProjectHandle::load_material`](crate::ProjectHandle::load_material)），
    /// 与 UI / 场景的 `sanitize` 同一职责：坏数据不进 GPU。
    pub fn sanitized(mut self) -> Self {
        for channel in &mut self.base_color {
            if !channel.is_finite() {
                *channel = 1.0;
            }
        }
        if !self.metallic.is_finite() {
            self.metallic = 0.0;
        }
        self.metallic = self.metallic.clamp(0.0, 1.0);
        if !self.roughness.is_finite() {
            self.roughness = default_roughness();
        }
        self.roughness = self.roughness.clamp(0.0, 1.0);
        self
    }
}

/// 逐实体材质覆盖（PBR metallic-roughness 参数，字段级）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct MaterialOverride {
    /// 基础色乘子（线性 RGBA），与模型自带的基础色**相乘**。
    ///
    /// 用乘子而不是直接替换：想"提亮 / 压暗一点"就乘个系数，
    /// 想"整个换色"把原色当白底乘上目标色也成立（内置图元原色就是白）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_color: Option<[f32; 4]>,
    /// 金属度覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metallic: Option<f32>,
    /// 粗糙度覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roughness: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 部分字段的覆盖：只写了的字段才是 `Some`，其余保持 `None`
    /// （"只改粗糙度"不该把金属度也钉死）。
    #[test]
    fn partial_override_deserializes_field_by_field() {
        let o: MaterialOverride =
            ron::from_str("(roughness: Some(0.2))").expect("应当可解析");
        assert_eq!(o.roughness, Some(0.2));
        assert_eq!(o.metallic, None);
        assert_eq!(o.base_color, None);

        // 整字段缺席（不是写 `None`，而是根本不写）走 serde default。
        let o: MaterialOverride =
            ron::from_str("(metallic: Some(0.5))").expect("应当可解析");
        assert_eq!(o.metallic, Some(0.5));
        assert_eq!(o.roughness, None);
    }

    /// 空表（`()`）是合法覆盖：全 None = 什么都不改。
    /// 这比"必须至少有一个字段"更好 —— 编辑器勾了覆盖再一个个调是常见路径。
    #[test]
    fn empty_override_is_all_none() {
        let o: MaterialOverride = ron::from_str("()").expect("空表应当可解析");
        assert_eq!(o, MaterialOverride::default());
    }

    /// 序列化时 `None` 字段不落盘（旧读方兼容 + 文件干净）。
    #[test]
    fn none_fields_are_not_serialized() {
        let o = MaterialOverride {
            metallic: Some(0.5),
            ..Default::default()
        };
        let text = ron::to_string(&o).expect("应当可序列化");
        assert!(text.contains("metallic"), "写了字段必须在: {text}");
        assert!(!text.contains("base_color"), "None 字段不该出现: {text}");
        assert!(!text.contains("roughness"), "None 字段不该出现: {text}");
    }

    /// 全字段往返。
    #[test]
    fn full_override_round_trips() {
        let o = MaterialOverride {
            base_color: Some([0.8, 0.2, 0.1, 1.0]),
            metallic: Some(1.0),
            roughness: Some(0.35),
        };
        let text = ron::to_string(&o).unwrap();
        let back: MaterialOverride = ron::from_str(&text).unwrap();
        assert_eq!(back, o);
    }

    // -- 材质资产 ---------------------------------------------------------

    /// 手写一份只写了颜色的 `.mat` 也该能读：其余字段走默认值。
    ///
    /// 这是 `.mat` 的第一条兼容契约 —— 用户 / 智能体不会每次写全四个字段。
    #[test]
    fn material_asset_fills_defaults() {
        let m: MaterialAsset = ron::from_str("(base_color: (0.8, 0.2, 0.1, 1.0))").unwrap();
        assert_eq!(m.base_color, [0.8, 0.2, 0.1, 1.0]);
        assert_eq!(m.metallic, 0.0);
        assert_eq!(m.roughness, 0.45);
        assert!(m.albedo.is_empty());

        let empty: MaterialAsset = ron::from_str("()").unwrap();
        assert_eq!(empty, MaterialAsset::default());
    }

    /// 全字段往返（含贴图引用）。空贴图不落盘。
    #[test]
    fn material_asset_round_trips_and_omits_empty_albedo() {
        let m = MaterialAsset {
            base_color: [0.3, 0.6, 0.2, 1.0],
            metallic: 0.25,
            roughness: 0.7,
            albedo: "Resources/Textures/Wood.png".into(),
        };
        let text = m.to_ron().unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(MaterialAsset::from_ron(&text).unwrap(), m);

        let plain = MaterialAsset::default();
        let text = plain.to_ron().unwrap();
        assert!(!text.contains("albedo"), "空贴图不该落盘: {text}");
    }

    /// 坏数据在加载边界被修好：NaN 颜色 / 越界金属度 / 无穷粗糙度。
    #[test]
    fn material_asset_sanitizes_garbage() {
        let m = MaterialAsset {
            base_color: [f32::NAN, 0.5, f32::INFINITY, 1.0],
            metallic: 9.0,
            roughness: f32::NAN,
            albedo: String::new(),
        }
        .sanitized();
        assert_eq!(m.base_color, [1.0, 0.5, 1.0, 1.0]);
        assert_eq!(m.metallic, 1.0);
        assert_eq!(m.roughness, 0.45);

        let m = MaterialAsset {
            metallic: -3.0,
            roughness: 4.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(m.metallic, 0.0);
        assert_eq!(m.roughness, 1.0);
    }
}
