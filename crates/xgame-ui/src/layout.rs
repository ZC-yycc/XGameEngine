//! 锚点布局：实体层级 + `UiElement` 锚点 → 像素矩形。
//!
//! # 只做一件事
//!
//! 这里**不读文本、不读图片、不碰字体**：一个 UI 元素的矩形完全由锚点与
//! 父矩形决定。这条性质让"无显卡 / 无中文字体的环境里按钮照样能点"成立，
//! 也让命中测试与渲染共用同一份几何（两套几何迟早会错位）。
//!
//! # 顺序
//!
//! 绘制顺序 = **深度优先先序**（父 → 子 → 下一个兄弟），同一层按实体在
//! `Scene::entities` 里的顺序。后画的盖住先画的，命中测试反向扫描。
//! 这是"唯一真相"：没有 z 序字段，也没有排序步骤。

use std::collections::HashMap;

use xgame_core::{EntityId, Scene, UiElement};
use xgame_math::glam::Vec2;

/// 轴对齐矩形（像素，左上原点，y 向下）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UiRect {
    /// 左上角。
    pub min: Vec2,
    /// 右下角。
    pub max: Vec2,
}

impl UiRect {
    /// 由两角构造。
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// 由左上角 + 尺寸构造。
    pub fn from_min_size(min: Vec2, size: Vec2) -> Self {
        Self {
            min,
            max: min + size,
        }
    }

    /// 从视口尺寸构造（左上角在原点）。
    pub fn viewport(size: Vec2) -> Self {
        Self {
            min: Vec2::ZERO,
            max: size,
        }
    }

    /// 尺寸。
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// 宽度。
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    /// 高度。
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// 中心点。
    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// 是否包含某点（左闭右开，与像素栅格一致）。
    pub fn contains(&self, point: Vec2) -> bool {
        point.x >= self.min.x
            && point.y >= self.min.y
            && point.x < self.max.x
            && point.y < self.max.y
    }

    /// 四周内缩（负数 = 外扩）。
    pub fn inset(&self, amount: f32) -> Self {
        Self {
            min: self.min + Vec2::splat(amount),
            max: self.max - Vec2::splat(amount),
        }
    }

    /// 退化成空矩形（宽或高 <= 0）。空矩形不参与命中测试与绘制。
    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }
}

/// 一次布局解算的结果条目 —— **只含几何与身份**，不借用场景。
///
/// 刻意不存 `&Entity`：命中测试之后要把 `UiState` 写回场景，若这里还借着
/// `&Scene`，写回就会撞借用检查器。存下标 + id 两样就够了，需要样式时
/// 调用方按 `index` 自己取。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutItem {
    /// 在 `Scene::entities` 里的下标。
    pub index: usize,
    /// 实体 id（哈希表查询用；`Scene` 可能被脚本增删，下标会变）。
    pub id: EntityId,
    /// 解算出的像素矩形。
    pub rect: UiRect,
    /// 嵌套深度（根为 0）。HUD 调试与缩进显示用。
    pub depth: u32,
}

/// UI 层级索引（每帧重建，容量复用）。
///
/// 为什么不直接每次 `scene.children(id)`：那是 O(n) 扫描，深层嵌套会退化成
/// O(n·depth)。这里一次 O(n) 建表，之后按下标访问。
#[derive(Default)]
pub struct UiTree {
    /// 实体下标 → 直接 UI 子元素下标（只装带 `ui` 的实体）。
    children: Vec<Vec<usize>>,
    /// 没有 UI 祖先的 UI 实体（屏幕空间根）。
    roots: Vec<usize>,
    /// 实体 id → 下标（建表用，容量复用）。
    index_of: HashMap<EntityId, usize>,
    /// 建表过程中每个 UI 实体的"最近 UI 祖先"下标。
    ancestor: Vec<Option<usize>>,
}

impl UiTree {
    /// 空树。
    pub fn new() -> Self {
        Self::default()
    }

    /// 重建索引。
    pub fn build(&mut self, scene: &Scene) {
        // 快路径：整个场景没有 UI 元素时，建表本身就不该发生。
        // 2000 个纯 3D 实体的场景每帧往 HashMap 里塞 2000 条是纯浪费 ——
        // 一次 `any` 扫描就够判断了。
        if !scene.entities.iter().any(|entity| entity.ui.is_some()) {
            self.index_of.clear();
            self.children.clear();
            self.roots.clear();
            self.ancestor.clear();
            return;
        }

        let count = scene.entities.len();
        self.index_of.clear();
        self.index_of.reserve(count);
        for (i, entity) in scene.entities.iter().enumerate() {
            self.index_of.insert(entity.id, i);
        }

        self.ancestor.clear();
        self.ancestor.resize(count, None);
        // `children` 的每个槽位都要清空但保留容量 —— `clear()` 会丢掉内部
        // Vec 的容量，那正是要复用的东西。
        self.children.resize_with(count, Vec::new);
        for slot in &mut self.children {
            slot.clear();
        }
        self.roots.clear();

        for (i, entity) in scene.entities.iter().enumerate() {
            if entity.ui.is_none() {
                continue;
            }
            // 沿 parent 往上找最近的、同样带 UI 的祖先。步数上限 = 实体数：
            // 手写 RON 里的父子环不能把这里变成死循环。
            let mut cursor = entity.parent;
            let mut steps = 0usize;
            let mut found = None;
            while let Some(parent_id) = cursor {
                steps += 1;
                if steps > count {
                    break;
                }
                let Some(&parent_index) = self.index_of.get(&parent_id) else {
                    break;
                };
                if scene.entities[parent_index].ui.is_some() {
                    found = Some(parent_index);
                    break;
                }
                cursor = scene.entities[parent_index].parent;
            }
            self.ancestor[i] = found;
            match found {
                Some(parent_index) => self.children[parent_index].push(i),
                None => self.roots.push(i),
            }
        }
    }

    /// 根元素下标（屏幕空间）。
    pub fn roots(&self) -> &[usize] {
        &self.roots
    }

    /// 某实体的直接 UI 子元素下标。
    pub fn children(&self, index: usize) -> &[usize] {
        self.children
            .get(index)
            .map(|v| v.as_slice())
            .unwrap_or_default()
    }
}

/// 嵌套深度上限。
///
/// 正常 UI 不会超过十几层；上限只用来兜住"手写 RON 造出千层嵌套"导致
/// 递归爆栈 —— 超出的子树直接不画（并告警一次），而不是让整个编辑器崩掉。
const MAX_DEPTH: u32 = 64;

/// 解算整棵 UI 树，按绘制顺序写入 `out`（会先清空）。
///
/// `viewport` 是视口像素尺寸（左上角原点）。没有 UI 祖先的元素以它为父矩形。
pub fn visit(scene: &Scene, tree: &UiTree, viewport: Vec2, out: &mut Vec<LayoutItem>) {
    out.clear();
    let root_rect = UiRect::viewport(viewport);
    let mut warned = false;
    for &index in tree.roots() {
        walk(scene, tree, index, root_rect, 0, out, &mut warned);
    }
}

fn walk(
    scene: &Scene,
    tree: &UiTree,
    index: usize,
    parent: UiRect,
    depth: u32,
    out: &mut Vec<LayoutItem>,
    warned: &mut bool,
) {
    let Some(entity) = scene.entities.get(index) else {
        return;
    };
    let Some(ui) = entity.ui.as_ref() else {
        return;
    };
    // 隐藏 = 整棵子树都不存在（Unity 的 SetActive(false) 语义）。
    if !ui.visible {
        return;
    }
    if depth > MAX_DEPTH {
        if !*warned {
            *warned = true;
            log::warn!("UI 嵌套超过 {MAX_DEPTH} 层，超出的子树被跳过（检查父子关系是否成环）");
        }
        return;
    }

    let rect = resolve(parent, ui);
    out.push(LayoutItem {
        index,
        id: entity.id,
        rect,
        depth,
    });

    for &child in tree.children(index) {
        walk(scene, tree, child, rect, depth + 1, out, warned);
    }
}

/// 锚点 → 矩形。这是整个 UI 布局的唯一公式。
///
/// ```text
/// rect.min = parent.min + anchor_min * parent.size + offset_min
/// rect.max = parent.min + anchor_max * parent.size + offset_max
/// ```
pub fn resolve(parent: UiRect, ui: &UiElement) -> UiRect {
    let size = parent.size();
    let anchor_min = Vec2::from(ui.anchor_min);
    let anchor_max = Vec2::from(ui.anchor_max);
    let offset_min = Vec2::from(ui.offset_min);
    let offset_max = Vec2::from(ui.offset_max);
    UiRect {
        min: parent.min + anchor_min * size + offset_min,
        max: parent.min + anchor_max * size + offset_max,
    }
}

/// 命中测试：反向扫描（最上层优先），返回第一个可交互且包含该点的条目。
///
/// 不可见 / `interactable = false` / 空矩形都会被跳过 —— 装饰元素让点击
/// 穿透到下层，这是 HUD 里"背景板不挡按钮"的常规行为。
pub fn hit_test(scene: &Scene, items: &[LayoutItem], point: Vec2) -> Option<LayoutItem> {
    items.iter().rev().copied().find(|item| {
        let Some(ui) = scene
            .entities
            .get(item.index)
            .and_then(|entity| entity.ui.as_ref())
        else {
            return false;
        };
        ui.is_hittable() && !item.rect.is_empty() && item.rect.contains(point)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::{Entity, Scene, UiElement};

    /// 固定尺寸贴左上：偏移就是像素。
    #[test]
    fn fixed_top_left_uses_offsets_as_pixels() {
        let parent = UiRect::viewport(Vec2::new(1280.0, 720.0));
        let ui = UiElement::panel(100.0, 20.0).at([0.0, 0.0], 12.0, 34.0, 100.0, 20.0);
        let rect = resolve(parent, &ui);
        assert_eq!(rect.min, Vec2::new(12.0, 34.0));
        assert_eq!(rect.max, Vec2::new(112.0, 54.0));
    }

    /// 右下角锚点：窗口变宽时元素跟着右移（分辨率适配的全部意义）。
    #[test]
    fn bottom_right_anchor_follows_parent_resize() {
        let ui = UiElement::panel(80.0, 30.0).at([1.0, 1.0], -90.0, -40.0, 80.0, 30.0);
        let small = resolve(UiRect::viewport(Vec2::new(800.0, 600.0)), &ui);
        let large = resolve(UiRect::viewport(Vec2::new(1600.0, 900.0)), &ui);
        assert_eq!(small.max, Vec2::new(790.0, 590.0));
        assert_eq!(large.max, Vec2::new(1590.0, 890.0));
        assert_eq!(small.size(), large.size(), "固定尺寸不随窗口变化");
    }

    /// 四边拉伸：缩进量在四边都成立。
    #[test]
    fn stretch_keeps_inset_on_all_sides() {
        let ui = UiElement::panel(1.0, 1.0).stretch(16.0);
        let rect = resolve(UiRect::viewport(Vec2::new(1000.0, 500.0)), &ui);
        assert_eq!(rect.min, Vec2::new(16.0, 16.0));
        assert_eq!(rect.max, Vec2::new(984.0, 484.0));
    }

    /// 子元素相对**父矩形**，不是视口。
    #[test]
    fn nested_child_is_relative_to_parent_rect() {
        let parent_rect = UiRect::from_min_size(Vec2::new(100.0, 50.0), Vec2::new(400.0, 300.0));
        // 父矩形中心、固定 100×40
        let child = UiElement::panel(100.0, 40.0).at([0.5, 0.5], -50.0, -20.0, 100.0, 40.0);
        let rect = resolve(parent_rect, &child);
        assert_eq!(rect.min, Vec2::new(250.0, 180.0));
        assert_eq!(rect.max, Vec2::new(350.0, 220.0));
    }

    /// 树：父先于子、同层按实体顺序；隐藏的子树整棵跳过。
    #[test]
    fn visit_orders_parent_before_children_and_skips_hidden() {
        let mut scene = Scene::new("tree");
        let parent = Entity::new("Panel");
        let mut parent = parent;
        parent.ui = Some(UiElement::panel(400.0, 300.0).at([0.0, 0.0], 10.0, 10.0, 400.0, 300.0));
        let parent_id = parent.id;
        scene.entities.push(parent);

        let mut a = Entity::new("A");
        a.parent = Some(parent_id);
        a.ui = Some(UiElement::panel(10.0, 10.0));
        scene.entities.push(a);

        let mut b = Entity::new("B");
        b.parent = Some(parent_id);
        b.ui = Some(UiElement::panel(10.0, 10.0));
        b.ui.as_mut().unwrap().visible = false;
        let b_id = b.id;
        scene.entities.push(b);

        let mut c = Entity::new("C");
        c.parent = Some(b_id);
        c.ui = Some(UiElement::panel(10.0, 10.0));
        scene.entities.push(c);

        let mut tree = UiTree::new();
        tree.build(&scene);
        let mut items = Vec::new();
        visit(&scene, &tree, Vec2::new(1280.0, 720.0), &mut items);

        let names: Vec<&str> = items
            .iter()
            .map(|i| scene.entities[i.index].name.as_str())
            .collect();
        assert_eq!(names, vec!["Panel", "A"], "隐藏的 B 及其子 C 都不该出现");
        assert_eq!(items[0].depth, 0);
        assert_eq!(items[1].depth, 1);
        // 子元素相对父矩形：父在 (10,10) 起 400×300，子锚点 (0,0) 偏移 (0,0)。
        assert_eq!(items[1].rect.min, Vec2::new(10.0, 10.0));
    }

    /// 命中测试：后画的优先；不可交互的元素让点击穿透。
    #[test]
    fn hit_test_prefers_topmost_interactable() {
        let mut scene = Scene::new("hit");
        let mut back = Entity::new("Back");
        back.ui = Some(UiElement::panel(400.0, 400.0).at([0.0, 0.0], 0.0, 0.0, 400.0, 400.0));
        scene.entities.push(back);
        let mut button = Entity::new("Button");
        button.ui = Some(UiElement::button("OK").at([0.0, 0.0], 10.0, 10.0, 100.0, 40.0));
        scene.entities.push(button);

        let mut tree = UiTree::new();
        tree.build(&scene);
        let mut items = Vec::new();
        visit(&scene, &tree, Vec2::new(800.0, 600.0), &mut items);

        let hit = hit_test(&scene, &items, Vec2::new(50.0, 30.0)).expect("按钮应被命中");
        assert_eq!(scene.entities[hit.index].name, "Button");

        let miss = hit_test(&scene, &items, Vec2::new(700.0, 500.0));
        assert!(miss.is_none(), "面板不可交互，不该吃掉空白处的点击");
    }
}
