//! LOD（距离分级）+ 绘制距离。
//!
//! # 为什么 `lod` 不是渲染器的私有概念
//!
//! "远处用低模、更远处干脆不画"是**内容层**的设定：哪一级用哪个模型、在多远
//! 换，都是美术 / 关卡的取舍，得跟着场景文件走、能被 `--check` 看见、能被
//! MCP 读写。渲染器只负责"按当前距离挑一级画出来"，判定规则本身放在 core
//! —— 与 `frustum` / 材质资产同一套分工。
//!
//! # 语义（三条，别推翻）
//!
//! 1. **`levels` 非空时 `Entity.model` 被忽略**。第 0 级就是最近的那一级，
//!    不再有"基础模型 + 追加级"的双轨写法 —— 否则"改 Model 却看不见变化"
//!    会变成常驻的困惑源。`lod` 为空（绝大多数实体）时 `model` 照旧是唯一来源，
//!    老场景逐位不变。
//! 2. **距离 = 相机位置到实体轴心（世界位置）的直线距离**，不是到包围盒表面的
//!    距离。轴心距离只看变换，不看网格 —— 判定因此可以在**查模型缓存之前**做完
//!    （远处实体的网格根本没加载也能判"不画"），而且同一实体在各级之间不会因为
//!    各级网格大小不同而来回抖。代价：模型轴心离几何中心很远时阈值会偏，作者
//!    按轴心调即可（Unity 用包围盒中心，这里刻意用轴心换"不抖 + 便宜"）。
//! 3. **超过最后一级的 `max_distance` = 不画**（绘制距离）。最后一级写 `0`
//!    （或任何非有限值）= 无限远，永不按距离剔除。
//!
//! # 判定规则（唯一一份）
//!
//! 给定距离 `d`，在**所有**"`d` 装得下"的级里取 `max_distance` **最小**的那一级。
//! 于是 `levels` 的书写顺序不影响结果（作者可以随手追加），并列时取靠前的。
//! 一个无限级永远只是兜底：只要有更近的有限级装得下，就不会选它。

use serde::{Deserialize, Serialize};

/// 一个 LOD 级：用哪个模型、生效到多远。
///
/// 字段全部 `serde(default)`：手写 RON 少写一项不会解析失败，`model` 空串的级
/// 在解析时被跳过（它没有几何可画，留着只会让"为什么这一级是空的"变成谜）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LodLevel {
    /// 这一级用的模型（项目相对路径或 `builtin://…`）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// 这一级生效的最大距离（世界单位，从相机量到实体轴心）。
    ///
    /// `0` 与非有限值（`NaN` / `±∞`）都表示**无限远** —— 它们是"最后一级不设上限"
    /// 的自然写法。负数同样按无限处理（而不是"永不生效"）：一个负数阈值多半是
    /// 手滑，让它变成"永远可见"比让它变成"永远不可见"更好救。
    #[serde(default)]
    pub max_distance: f32,
}

impl LodLevel {
    /// 这一级是不是"无限远"（不需要距离判定就能命中）。
    pub fn is_unlimited(&self) -> bool {
        !(self.max_distance.is_finite() && self.max_distance > 0.0)
    }

    /// 构造一个有限距离的级（测试与编辑器"加一级"用）。
    pub fn new(model: impl Into<String>, max_distance: f32) -> Self {
        Self {
            model: model.into(),
            max_distance,
        }
    }
}

/// 一个实体的 LOD 组。
///
/// `levels` 为空（= 绝大多数实体）时它**完全不存在**：不落盘、不参与判定、
/// 不额外加载任何模型。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LodGroup {
    /// 各级。**顺序不影响判定**（见模块文档），编辑器按 `max_distance` 排序展示。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<LodLevel>,
}

impl LodGroup {
    /// 没有任何可画的级（空列表，或全是空 `model` 的占位）。落盘时整段省略。
    pub fn is_empty(&self) -> bool {
        self.levels.iter().all(|level| level.model.is_empty())
    }

    /// 可画的级（跳过空 `model` 的占位），保持作者书写的下标。
    pub fn drawable_levels(&self) -> impl Iterator<Item = (usize, &LodLevel)> {
        self.levels
            .iter()
            .enumerate()
            .filter(|(_, level)| !level.model.is_empty())
    }

    /// 距离 `distance` 下应该用哪一级 —— 返回**作者下标**与模型路径。
    ///
    /// `None` = 这个距离上不该画：没有可画的级，或者已经超出全部有限级
    /// （且没有无限级兜底）。
    ///
    /// `bias` 是**全局偏置**（`Project.render.lod_bias`）：`> 1` 把切换点推远
    /// （高配机器 / 想要更好画质），`0 < bias < 1` 拉近（低配）。`bias <= 0`
    /// 表示"**关掉 LOD**"：永远用第 0 级，且不做距离剔除 —— 这是排查"远处物体
    /// 突然变样"时最需要的那个开关。`bias` 非有限时按 1 处理（坏配置退回默认，
    /// 而不是让整场景消失）。
    pub fn resolve(&self, distance: f32, bias: f32) -> Option<(usize, &str)> {
        // 坏偏置（NaN / ±∞）按 1 处理：一份坏配置不该让整场景变样。
        let bias = if bias.is_finite() { bias } else { 1.0 };
        if bias <= 0.0 {
            // bias <= 0：关掉 LOD —— 永远第 0 级，且不做距离剔除。
            // `drawable_levels` 按下标升序，第一个就是"最靠前的可画级"。
            return self
                .drawable_levels()
                .next()
                .map(|(index, level)| (index, level.model.as_str()));
        }

        // 距离本身坏掉（NaN）时按"最近"处理：宁可画最精细的一级，
        // 也不要让一个坏变换把整个物体变成隐形。
        let distance = if distance.is_finite() {
            distance.max(0.0)
        } else {
            0.0
        };

        let mut best: Option<(usize, &LodLevel, f32)> = None;
        let mut unlimited: Option<(usize, &LodLevel)> = None;
        for (index, level) in self.drawable_levels() {
            if level.is_unlimited() {
                // 多个无限级时取靠前的那个（书写顺序在这里是唯一可辨别的意图）。
                if unlimited.is_none() {
                    unlimited = Some((index, level));
                }
                continue;
            }
            let limit = level.max_distance * bias;
            if distance <= limit && best.is_none_or(|(_, _, current)| limit < current) {
                best = Some((index, level, limit));
            }
        }
        match best {
            Some((index, level, _)) => Some((index, level.model.as_str())),
            None => unlimited.map(|(index, level)| (index, level.model.as_str())),
        }
    }

    /// 用于展示 / 文档的"最远可见距离"：最后一个有限级的阈值 × bias；没有就 `None`
    /// （= 无限）。
    pub fn max_visible_distance(&self) -> Option<f32> {
        self.drawable_levels()
            .filter(|(_, level)| !level.is_unlimited())
            .map(|(_, level)| level.max_distance)
            .fold(None, |acc: Option<f32>, d| Some(acc.map_or(d, |a| a.max(d))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(levels: &[(&str, f32)]) -> LodGroup {
        LodGroup {
            levels: levels
                .iter()
                .map(|(model, distance)| LodLevel::new(*model, *distance))
                .collect(),
        }
    }

    /// 基本语义：最近的装得下的那一级。
    #[test]
    fn resolve_picks_smallest_covering_limit() {
        let lod = group(&[("near", 10.0), ("mid", 30.0), ("far", 100.0)]);
        assert_eq!(lod.resolve(1.0, 1.0), Some((0, "near")));
        assert_eq!(lod.resolve(10.0, 1.0), Some((0, "near")), "边界归近的一级");
        assert_eq!(lod.resolve(10.1, 1.0), Some((1, "mid")));
        assert_eq!(lod.resolve(30.0, 1.0), Some((1, "mid")));
        assert_eq!(lod.resolve(99.0, 1.0), Some((2, "far")));
    }

    /// 书写顺序不影响判定 —— 作者可以随手追加一级，不必手工排序。
    #[test]
    fn resolve_ignores_author_order() {
        let sorted = group(&[("near", 10.0), ("mid", 30.0), ("far", 100.0)]);
        let shuffled = group(&[("far", 100.0), ("near", 10.0), ("mid", 30.0)]);
        for distance in [0.0, 5.0, 10.0, 29.0, 30.0, 99.0, 100.0] {
            assert_eq!(
                sorted.resolve(distance, 1.0).map(|(_, m)| m),
                shuffled.resolve(distance, 1.0).map(|(_, m)| m),
                "距离 {distance} 下两种书写顺序必须选同一条模型"
            );
        }
        // 但返回的下标是**作者下标**（渲染器按它回查模型）。
        assert_eq!(shuffled.resolve(1.0, 1.0), Some((1, "near")));
    }

    /// 超出最后一级 = 不画；最后一级写 0 = 无限远，永不按距离剔除。
    #[test]
    fn draw_distance_culls_and_zero_is_unlimited() {
        let finite = group(&[("near", 10.0), ("far", 50.0)]);
        assert_eq!(finite.resolve(50.0, 1.0), Some((1, "far")));
        assert_eq!(finite.resolve(50.1, 1.0), None, "超出绘制距离就不画");
        assert_eq!(finite.resolve(1e9, 1.0), None);

        let unlimited = group(&[("near", 10.0), ("far", 0.0)]);
        assert_eq!(unlimited.resolve(1e9, 1.0), Some((1, "far")));

        // NaN / 负数的阈值也按"无限"处理（手滑的负数不该让物体永久消失）。
        let sloppy = group(&[("near", 10.0), ("far", f32::NAN), ("last", -5.0)]);
        assert_eq!(sloppy.resolve(1e9, 1.0), Some((1, "far")));
    }

    /// 无限级只是兜底：有更近的有限级装得下时不会选它。
    #[test]
    fn unlimited_level_is_only_a_fallback() {
        let lod = group(&[("unlimited", 0.0), ("near", 5.0)]);
        assert_eq!(lod.resolve(1.0, 1.0), Some((1, "near")));
        assert_eq!(lod.resolve(500.0, 1.0), Some((0, "unlimited")));
    }

    /// 空 `model` 的级是占位，不参与判定（也不会被当成"不画"的级）。
    #[test]
    fn empty_model_levels_are_skipped() {
        let mut lod = group(&[("", 10.0), ("far", 100.0)]);
        assert_eq!(lod.resolve(1.0, 1.0), Some((1, "far")));
        assert!(!lod.is_empty(), "有可画的级就不算空");

        lod.levels[1].model.clear();
        assert!(lod.is_empty(), "全是空模型 = 没有 LOD");
        assert_eq!(lod.resolve(1.0, 1.0), None);
    }

    /// 偏置：>1 把切换点推远，<1 拉近，<=0 关掉 LOD（永远第 0 级、不剔除）。
    #[test]
    fn bias_scales_thresholds_and_zero_disables() {
        let lod = group(&[("near", 10.0), ("far", 100.0)]);
        assert_eq!(lod.resolve(15.0, 1.0), Some((1, "far")));
        assert_eq!(lod.resolve(15.0, 2.0), Some((0, "near")), "bias 2 = 阈值翻倍");
        assert_eq!(
            lod.resolve(6.0, 0.5),
            Some((1, "far")),
            "bias 0.5 = 阈值减半"
        );

        // 关掉 LOD：距离再远也是第 0 级、也不剔除。
        assert_eq!(lod.resolve(1e9, 0.0), Some((0, "near")));
        assert_eq!(lod.resolve(1e9, -3.0), Some((0, "near")));

        // 坏偏置（NaN / ∞）按 1 处理：坏配置不该让整场景变样。
        assert_eq!(lod.resolve(15.0, f32::NAN), Some((1, "far")));
        assert_eq!(lod.resolve(15.0, f32::INFINITY), Some((1, "far")));
    }

    /// 坏距离（NaN）按最近处理：坏变换不该让物体隐形。
    #[test]
    fn nan_distance_uses_nearest_level() {
        let lod = group(&[("near", 10.0), ("far", 0.0)]);
        assert_eq!(lod.resolve(f32::NAN, 1.0), Some((0, "near")));
    }

    /// 最远可见距离（文档 / 编辑器提示用）。
    #[test]
    fn max_visible_distance_reports_the_hard_limit() {
        assert_eq!(group(&[("a", 10.0), ("b", 40.0)]).max_visible_distance(), Some(40.0));
        assert_eq!(group(&[("a", 10.0), ("b", 0.0)]).max_visible_distance(), Some(10.0));
        assert_eq!(group(&[("b", 0.0)]).max_visible_distance(), None);
        assert_eq!(LodGroup::default().max_visible_distance(), None);
    }
}
