//! 层（Layer）与碰撞矩阵 —— 物理过滤的**唯一**真相源。
//!
//! # 为什么要有这一层
//!
//! 没有层之前，实体之间"碰不碰"只取决于它们是否有碰撞体：想要"敌人的子弹
//! 不打敌人自己"，只能在脚本里对每个碰撞事件手工判断两个实体是谁。实体一多，
//! 这种判断散落在每个脚本里，既写不对也没法一次改全局（比如"让所有
//! 可拾取物不与敌人碰撞"）。
//!
//! 层把这件事变成**数据**：每个实体有一个层索引（0–31），项目里有一张
//! 32×32 的碰撞矩阵，物理引擎（rapier 的 `InteractionGroups`）在求解阶段
//! 就把不该碰的成对排除掉 —— 不是"碰上了再丢掉事件"，而是根本不会产生接触。
//! 这也是它能顺带省 CPU 的原因。
//!
//! # 语义（三条，都是刻意的）
//!
//! 1. **默认全通**：未写的层掩码 = `0xFFFF_FFFF`，与新实体默认的层 0 一起
//!    保证"老场景 / 不配层"的行为与引入层之前**逐位一致**。这是本仓库
//!    "新增能力默认关"纪律在数据上的落点。
//! 2. **矩阵是对称的**：`set_collides(a, b, on)` 同时写 `a→b` 与 `b→a`。
//!    rapier 的判定是 `(自成员 & 对方掩码) != 0 && (对方成员 & 自掩码) != 0`
//!    （两侧都要同意）。手改 RON 写出不对称的一对时，**生效语义是两侧相与**
//!    —— [`Layers::collides`] 与物理层用同一条规则，所以编辑器矩阵格子里
//!    显示的勾选状态就是实际行为。
//! 3. **只存覆盖**：`names` / `masks` 只记改过的层（`BTreeMap<u8, _>`），
//!    全默认的项目清单里这两节整个不出现。32 条全量写进 `project.ron`
//!    会让配置文件噪声盖过内容，而 99% 的项目只改两三个层。
//!
//! # 谁在用
//!
//! - 物理层（`xgame-physics`）只认位掩码，不认识"层"这个名字概念；
//!   翻译在运行时（`World::sync_bodies`）做一次，见 [`Layers::groups`]。
//! - 编辑器 Inspector 的层下拉、项目设置的矩阵网格读的也是这份数据。
//! - 脚本 `set_layer` / `layer` / `raycast_masked` 走同一份。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 层总数。与 rapier 的 `InteractionGroups`（两个 `u32`）严格对齐。
///
/// 改成 64 会让 `memberships` 溢出高位，所以这个数是**架构常量**，不是可调参数。
pub const LAYER_COUNT: usize = 32;

/// 默认层索引：新实体、老场景、脚本没有显式设置时都在这一层。
pub const DEFAULT_LAYER: u8 = 0;

/// 层名字与碰撞矩阵（落盘在 `project.ron` 的 `layers` 节）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Layers {
    /// 改过名字的层：索引 → 名字。没写的层显示成 `Layer N`。
    ///
    /// 用 `BTreeMap` 而不是 `Vec<String>`：顺序确定（序列化可 diff），
    /// 且不必为了给第 3 层起个名字就把另外 31 个空串写进文件。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<u8, String>,
    /// 有改动的层的碰撞掩码：索引 → 32 位掩码（第 N 位 = 与第 N 层碰撞）。
    ///
    /// 没写 = `0xFFFF_FFFF`（与所有层都碰），保证默认行为不变。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub masks: BTreeMap<u8, u32>,
}

impl Layers {
    /// 全默认：所有层互相碰撞、名字全空。
    pub fn all_collide() -> Self {
        Self::default()
    }

    /// 是不是"全默认"（没有改名、没有关掉任何一对）。
    ///
    /// `project.ron` 用它做 `skip_serializing_if`：没配层的项目文件里
    /// 连 `layers` 这个键都不出现。
    pub fn is_default(&self) -> bool {
        self.names.is_empty() && self.masks.is_empty()
    }

    /// 某层的碰撞掩码（未配置 = 与所有层碰撞）。
    pub fn mask(&self, layer: u8) -> u32 {
        self.masks.get(&layer).copied().unwrap_or(u32::MAX)
    }

    /// 某层的显示名；没起名返回空串。
    pub fn name(&self, layer: u8) -> &str {
        self.names.get(&layer).map(String::as_str).unwrap_or("")
    }

    /// 下拉框 / 列表里的一行文字：`0  Default`、`3  Layer 3`。
    ///
    /// 名字为空时显示 `Layer N` 而不是空串 —— 层号本身是必须可见的信息，
    /// 空名字的层在列表里会变成一排无法区分的空白行。
    pub fn label(&self, layer: u8) -> String {
        let name = self.name(layer);
        if name.is_empty() {
            format!("{layer}  Layer {layer}")
        } else {
            format!("{layer}  {name}")
        }
    }

    /// 改某层的名字（空串 = 退回默认显示）。
    pub fn set_name(&mut self, layer: u8, name: impl Into<String>) {
        if layer as usize >= LAYER_COUNT {
            return;
        }
        let name = name.into();
        if name.is_empty() {
            self.names.remove(&layer);
        } else {
            self.names.insert(layer, name);
        }
    }

    /// 两层之间是否碰撞。
    ///
    /// **两侧都要同意**（与 rapier 的 `InteractionGroups::test` 同一条规则）：
    /// 手改 RON 写出一对不对称的掩码时，这里给出的就是物理层实际会做的判定。
    pub fn collides(&self, a: u8, b: u8) -> bool {
        if a as usize >= LAYER_COUNT || b as usize >= LAYER_COUNT {
            return false;
        }
        let bit_a = 1u32 << a;
        let bit_b = 1u32 << b;
        (self.mask(a) & bit_b) != 0 && (self.mask(b) & bit_a) != 0
    }

    /// 打开 / 关闭两层之间的碰撞（**同时写两侧**，见模块文档第 2 条）。
    pub fn set_collides(&mut self, a: u8, b: u8, on: bool) {
        if a as usize >= LAYER_COUNT || b as usize >= LAYER_COUNT {
            return;
        }
        let (bit_a, bit_b) = (1u32 << a, 1u32 << b);
        let mut mask_a = self.mask(a);
        let mut mask_b = self.mask(b);
        if on {
            mask_a |= bit_b;
            mask_b |= bit_a;
        } else {
            mask_a &= !bit_b;
            mask_b &= !bit_a;
        }
        self.write_mask(a, mask_a);
        self.write_mask(b, mask_b);
    }

    /// 写掩码，全默认时删掉条目（保持"只存覆盖"的落盘形态）。
    fn write_mask(&mut self, layer: u8, mask: u32) {
        if mask == u32::MAX {
            self.masks.remove(&layer);
        } else {
            self.masks.insert(layer, mask);
        }
    }

    /// 物理层要的一对位掩码：`(memberships, filter)`。
    ///
    /// `memberships` 只有本位，`filter` 是本层掩码 —— 交给 rapier 后，
    /// 两个碰撞体只有在**互相都在对方掩码里**时才互动，正是矩阵语义。
    pub fn groups(&self, layer: u8) -> (u32, u32) {
        let layer = sanitize_layer(layer);
        (1u32 << layer, self.mask(layer))
    }

    /// 容错：丢掉越界索引、修掉空名字、把空掩码补成全通？
    ///
    /// **不补空掩码**：`0` 是用户明确表达的"这层不和任何东西碰"，
    /// 把它改回全通等于吞掉一个有意义的配置。
    pub fn sanitized(&self) -> Self {
        let mut names = BTreeMap::new();
        for (layer, name) in &self.names {
            if (*layer as usize) < LAYER_COUNT && !name.trim().is_empty() {
                names.insert(*layer, name.trim().to_string());
            }
        }
        let mut masks = BTreeMap::new();
        for (layer, mask) in &self.masks {
            if (*layer as usize) < LAYER_COUNT && *mask != u32::MAX {
                masks.insert(*layer, *mask);
            }
        }
        Self { names, masks }
    }
}

/// 把任意 `u8` 收敛成合法层索引（越界一律当 [`DEFAULT_LAYER`]）。
///
/// 用 `& 31` 而不是 `min(31)`：`& 31` 对 32 取模，`min` 会把 33–255 全部压到
/// 31 层。两种都不"对"，但取模至少不会让一堆越界实体挤在同一层上 ——
/// 越界本身由 `--check` 报出来，这里只保证不 panic、不越位。
pub fn sanitize_layer(layer: u8) -> u8 {
    layer & (LAYER_COUNT as u8 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_collide_everywhere() {
        let layers = Layers::default();
        for a in 0..LAYER_COUNT as u8 {
            for b in 0..LAYER_COUNT as u8 {
                assert!(layers.collides(a, b), "默认 {a}×{b} 应当碰撞");
            }
        }
        // 默认落盘是**空**的：老项目不该被写进一坨默认值。
        assert_eq!(ron::to_string(&Layers::default()).unwrap(), "()");
    }

    #[test]
    fn disabling_a_pair_is_symmetric() {
        let mut layers = Layers::default();
        layers.set_collides(2, 3, false);

        assert!(!layers.collides(2, 3));
        assert!(!layers.collides(3, 2), "矩阵必须对称写：两个方向都不碰");
        assert!(layers.collides(2, 2), "关掉的是 2×3，不是 2×2");
        assert!(layers.collides(3, 4));

        // 打开回去之后掩码回到全默认，条目从落盘里消失。
        layers.set_collides(2, 3, true);
        assert!(layers.collides(2, 3));
        assert!(layers.masks.is_empty(), "恢复全通后不该留下空壳条目");
    }

    #[test]
    fn asymmetric_hand_edit_means_both_sides_must_agree() {
        // 手改 RON 只关一半：a 不看 b，b 仍然看 a → 不碰（与 rapier 一致）。
        let mut layers = Layers::default();
        layers.masks.insert(1, !(1 << 2));
        assert!(!layers.collides(1, 2));
        assert_eq!(layers.groups(1), (1 << 1, !(1 << 2)));
    }

    #[test]
    fn zero_mask_blocks_everything_and_survives_sanitize() {
        let mut layers = Layers::default();
        layers.masks.insert(5, 0);
        let clean = layers.sanitized();
        assert_eq!(
            clean.mask(5),
            0,
            "用户明确配的「不碰任何东西」不能被修回全通"
        );
        assert!(!clean.collides(5, 5));
    }

    #[test]
    fn sanitize_drops_out_of_range_and_empty_names() {
        let mut layers = Layers::default();
        layers.names.insert(3, "  Player  ".into());
        layers.names.insert(3 + LAYER_COUNT as u8, "越界".into());
        layers.names.insert(4, "   ".into());
        let clean = layers.sanitized();
        assert_eq!(clean.name(3), "Player", "两侧空白要去掉");
        assert_eq!(clean.names.len(), 1);
    }

    #[test]
    fn labels_fall_back_to_layer_index() {
        let mut layers = Layers::default();
        assert_eq!(layers.label(7), "7  Layer 7");
        layers.set_name(7, "Enemy");
        assert_eq!(layers.label(7), "7  Enemy");
        layers.set_name(7, "");
        assert_eq!(layers.label(7), "7  Layer 7", "清空名字要退回默认标签");
    }

    #[test]
    fn out_of_range_layers_wrap_instead_of_panicking() {
        assert_eq!(sanitize_layer(32), 0);
        assert_eq!(sanitize_layer(33), 1);
        assert_eq!(sanitize_layer(255), 31);
        // 越界查询一律「不碰」，不 panic。
        let layers = Layers::default();
        assert!(!layers.collides(40, 0));
    }

    #[test]
    fn entity_layer_only_hits_disk_when_set() {
        use crate::scene::Entity;

        let plain = ron::to_string(&Entity::new("Box")).unwrap();
        assert!(
            !plain.contains("layer"),
            "0 层实体不该把 layer 写进场景文件（老文件的形态要保住）: {plain}"
        );

        let on_level_three = Entity::new("Player").with_layer(3);
        let text = ron::to_string(&on_level_three).unwrap();
        assert!(text.contains("layer"), "显式设置过的层必须落盘: {text}");
        let back: Entity = ron::from_str(&text).unwrap();
        assert_eq!(back.layer, 3);

        // 越界在 builder 处就收敛：手写 40 不会变成 panic 或越位下标。
        assert_eq!(Entity::new("X").with_layer(40).layer, 8);
    }

    #[test]
    fn roundtrip_keeps_overrides_only() {
        let mut layers = Layers::default();
        layers.set_name(1, "Player");
        layers.set_collides(1, 2, false);
        let text = ron::to_string(&layers).unwrap();
        assert!(text.contains("Player"), "改过的名字要落盘: {text}");
        assert!(
            !text.contains("Layer 0"),
            "没改过的层不该出现在文件里: {text}"
        );

        let back: Layers = ron::from_str(&text).unwrap();
        assert_eq!(back, layers);
        assert!(!back.collides(1, 2));
    }
}
