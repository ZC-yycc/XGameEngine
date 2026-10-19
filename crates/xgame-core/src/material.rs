//! 逐实体材质覆盖。
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
}
