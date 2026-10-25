//! egui_tiles 集成——面板标签类型、树布局和行为。
//!
//! 构建类 Unity 的拆分布局，支持可拖拽的标签头。
//! 每个面板包裹在 `Tabs` 容器中，标签栏作为停靠/取消停靠的拖拽手柄。

use crate::panels::theme;
use crate::state::EditorState;
use egui_tiles::{
    Behavior, Linear, LinearDir, SimplificationOptions, TileId, Tiles, Tree, UiResponse,
};
use serde::{Deserialize, Serialize};
use strum::Display;
use xgame_derive::Panel;

/// 编辑器中所有可停靠的面板类型。
///
/// `Serialize` / `Deserialize` 是给布局持久化用的（`editor.ron`）：磁盘上的
/// 页签名就是变体名（`SceneView`），所以**变体名是数据格式的一部分**，
/// 改名等于让老布局失效（`is_usable_layout` 会回退默认布局，不会崩）。
#[derive(Debug, Clone, Display, PartialEq, Eq, Hash, Serialize, Deserialize, Panel)]
pub enum PanelTab {
    Hierarchy,
    /// 资源浏览器（P0 资产管线）：浏览 Resources/ 全部资产并拖进场景。
    Assets,
    Prefabs,
    SceneView,
    Inspector,
    Console,
    /// 帧分析（Profiler）：帧间隔 / 主循环耗时的滚动窗口 + 绘制读数。
    Profiler,
}

impl PanelTab {
    /// 所有面板（顺序 = 校验与测试里"一个都不能少"的清单）。
    pub const ALL: [PanelTab; 7] = [
        PanelTab::Hierarchy,
        PanelTab::Assets,
        PanelTab::Prefabs,
        PanelTab::SceneView,
        PanelTab::Inspector,
        PanelTab::Console,
        PanelTab::Profiler,
    ];

    /// 页签上显示的名字。
    ///
    /// 与枚举名分开：枚举名是代码里的稳定标识（`SceneView`），页签名是**给人看的**
    /// （UE 的视口叫 Viewport、日志面板叫 Output Log）。改名字不该牵动代码引用。
    pub fn title(&self) -> &'static str {
        match self {
            PanelTab::Hierarchy => "Hierarchy",
            PanelTab::Assets => "Assets",
            PanelTab::Prefabs => "Prefabs",
            PanelTab::SceneView => "Viewport",
            PanelTab::Inspector => "Inspector",
            PanelTab::Console => "Output Log",
            PanelTab::Profiler => "Profiler",
        }
    }
}

pub fn create_tile_tree() -> Tree<PanelTab> {
    let mut tiles = Tiles::default();

    // Insert pane tiles
    let hier = tiles.insert_pane(PanelTab::Hierarchy);
    let assets = tiles.insert_pane(PanelTab::Assets);
    let prefs = tiles.insert_pane(PanelTab::Prefabs);
    let scene = tiles.insert_pane(PanelTab::SceneView);
    let insp = tiles.insert_pane(PanelTab::Inspector);
    let cons = tiles.insert_pane(PanelTab::Console);
    let prof = tiles.insert_pane(PanelTab::Profiler);

    // Assets 与 Prefabs 共用左列下半格的两个页签：
    // 它们是同一类东西（"项目里有什么"），单独再切一格会把 Hierarchy 挤没。
    let asset_tabs = tiles.insert_tab_tile(vec![assets, prefs]);

    // ── Build containers with custom shares ──

    // Left column: Hierarchy (58%) | Assets+Prefabs (42%)
    // 与 Unity 一致：层级树在上，资源浏览器在下。
    let left_column = tiles.insert_container(
        Linear::new(LinearDir::Vertical, vec![hier, asset_tabs])
            .with_share(hier, 0.58)
            .with_share(asset_tabs, 0.42),
    );

    // Top row: Scene View (72%) | Inspector (28%)
    //
    // Inspector 从 25% 提到 28%：Transform 的三轴并排需要「标签列 58 + 3×(轴标 +
    // 输入框) + 6 个间距」。25% 时 1280 窗口只分到 250px，输入框被压到 37px
    // （数值勉强能看，但拖拽手感很差）；28% 能到 43px，1440 窗口正好稳住
    // 标准的 54px —— 与旧布局在 1440 下逐像素一致，窄窗口才真正受益。
    let top_row = tiles.insert_container(
        Linear::new(LinearDir::Horizontal, vec![scene, insp])
            .with_share(scene, 0.72)
            .with_share(insp, 0.28),
    );

    // Right column: top_row (75%) | Console+Profiler (25%)
    //
    // Profiler 与 Output Log 同一组：两者都是"底部横条"型的诊断面板，同时看
    // 日志和帧时间才是排查问题的常态。默认激活第一个（Output Log），
    // 老的截图与手感因此逐像素不变；要看帧分析就点页签（`--ui-profiler`）。
    let bottom_tabs = tiles.insert_tab_tile(vec![cons, prof]);
    let right_column = tiles.insert_container(
        Linear::new(LinearDir::Vertical, vec![top_row, bottom_tabs])
            .with_share(top_row, 0.75)
            .with_share(bottom_tabs, 0.25),
    );

    // Root: left_column (22%) | right_column (78%)
    let root = tiles.insert_container(
        Linear::new(LinearDir::Horizontal, vec![left_column, right_column])
            .with_share(left_column, 0.22)
            .with_share(right_column, 0.78),
    );

    Tree::new("editor_tree", root, tiles)
}

/// 校验一个**从磁盘读回来的**布局是否还能用。
///
/// 磁盘是外部输入：老版本写的布局可能少了后来新增的面板，手改过的文件可能有
/// 悬空 tile / 环 / 重复 pane。`egui_tiles` 的 `Tree::ui` 假定这些不变量成立，
/// 直接喂坏树轻则少几个面板，重则整帧 panic —— 所以恢复布局前必须过这一道。
///
/// 判据（全部满足才可用）：
/// 1. 根节点存在，且从根能走到的每个 tile 都真实存在；
/// 2. 没有环 / 重复引用（同一个 tile 被走到两次）；
/// 3. 没有不可达的孤儿 tile（它们会跟着下次保存越积越多）；
/// 4. 每个 [`PanelTab::ALL`] 变体恰好出现一次 —— 少一个就永远打不开那个面板，
///    多一个则说明文件被改坏了。
pub fn is_usable_layout(tree: &Tree<PanelTab>) -> bool {
    let Some(root) = tree.root() else {
        return false;
    };

    let mut seen = std::collections::HashSet::new();
    let mut panes = std::collections::HashSet::new();
    let mut stack = vec![root];

    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            return false;
        }
        match tree.tiles.get(id) {
            Some(egui_tiles::Tile::Pane(pane)) => {
                if !panes.insert(pane.clone()) {
                    return false;
                }
            }
            Some(egui_tiles::Tile::Container(container)) => {
                let mut children = container.children().peekable();
                // 空容器是没有任何意义的中间层，也说明布局来自坏数据。
                if children.peek().is_none() {
                    return false;
                }
                stack.extend(children.copied());
            }
            None => return false,
        }
    }

    if tree.tiles.iter().any(|(id, _)| !seen.contains(id)) {
        return false;
    }

    PanelTab::ALL.iter().all(|tab| panes.contains(tab))
}

/// 把读回来的布局重新挂到编辑器固定的树 id 上。
///
/// `Tree` 的 `id` 决定 egui 持久化状态（页签拖拽 / 分隔条）的 key。旧文件里
/// 可能是别的 id，或干脆被手改过；重建一次只花几个分配，换来"布局状态一定
/// 挂在当前树 id 下"。
pub fn normalize_layout(mut tree: Tree<PanelTab>) -> Tree<PanelTab> {
    let Some(root) = tree.root() else {
        return tree;
    };
    let tiles = std::mem::take(&mut tree.tiles);
    Tree::new("editor_tree", root, tiles)
}

/// 把某个面板页签切到前台。
///
/// 给 `--ui-profiler` 这类截图开关用：面板在布局里存在，但默认不是激活页签时，
/// 截出来的就是邻居的内容 —— "改了界面却截不到"最容易发生在同一组的第二个页签上。
///
/// `make_active` 只按谓词命中 pane，祖先容器的激活链由 egui_tiles 自己补，
/// 所以这里不需要知道这棵树是怎么分组的（布局再改也不会失配）。
pub fn activate(tree: &mut Tree<PanelTab>, pane: PanelTab) {
    let hit = tree.make_active(|_, tile| matches!(tile, egui_tiles::Tile::Pane(p) if *p == pane));
    if !hit {
        log::warn!("面板 {:?} 不在当前布局里，无法激活", pane);
    }
}

// ── Helper: set share on a linear container builder ──

trait LinearBuilder {
    fn with_share(self, tile: TileId, share: f32) -> Self;
}

impl LinearBuilder for Linear {
    fn with_share(mut self, tile: TileId, share: f32) -> Self {
        self.shares.set_share(tile, share);
        self
    }
}

// ── Behavior ──

/// egui_tiles 的 [`Behavior`]——将每个面板标签映射到其内容。
pub struct EditorBehavior<'a> {
    pub state: &'a mut EditorState,
}

impl Behavior<PanelTab> for EditorBehavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut PanelTab) -> UiResponse {
        // 面板内边距统一走主题常量：所有面板的内容起点因此对齐。
        egui::Frame::default()
            .inner_margin(egui::Margin::same(theme::PANEL_MARGIN))
            .show(ui, |ui| {
                pane.show(self.state, ui);
            });
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &PanelTab) -> egui::WidgetText {
        pane.title().to_owned().into()
    }

    fn is_tab_closable(&self, _tiles: &Tiles<PanelTab>, _tile_id: TileId) -> bool {
        false
    }

    /// **关键**：将每个面板包裹在 `Tabs` 容器中，使标签栏作为
    /// 重排、拆分和取消停靠的拖拽手柄。
    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..SimplificationOptions::default()
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        theme::TAB_BAR_HEIGHT
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        theme::colors::TAB_BAR_BG
    }

    fn tab_title_spacing(&self, _visuals: &egui::Visuals) -> f32 {
        10.0
    }

    fn tab_bar_hline_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::new(1.0_f32, theme::colors::SEPARATOR_SOFT)
    }

    /// 自绘页签（覆盖 egui_tiles 的默认实现）。
    ///
    /// 默认实现的"选中 = 一个 1px 蓝框"太弱，看起来像没画完的控件。UE 的做法是：
    /// **选中页签用面板底色与内容连成一片**、顶部一条 2px 强调线，
    /// 未选中的页签融进更暗的页签条里、只有悬停才浮起来。
    ///
    /// 点击 / 拖拽的**语义**仍由 egui_tiles 处理（`Tabs` 读返回的 `Response`），
    /// 这里只负责画。
    fn tab_ui(
        &mut self,
        _tiles: &mut Tiles<PanelTab>,
        ui: &mut egui::Ui,
        id: egui::Id,
        tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Response {
        use theme::colors;

        // 标题仍然走 `tab_title_for_pane`（拿一份 pane 副本喂它，避免与
        // `&mut self` 的可变借用打架）。
        let pane = match _tiles.get(tile_id) {
            Some(egui_tiles::Tile::Pane(pane)) => pane.clone(),
            _ => unreachable!("页签对应的 pane 必须存在"),
        };
        let text = self.tab_title_for_pane(&pane);

        let font = if state.active {
            egui::FontId::new(12.5, theme::bold_family())
        } else {
            egui::FontId::proportional(12.5)
        };
        let galley = text.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, font);

        let x_margin = self.tab_title_spacing(ui.visuals());
        let width = galley.size().x + 2.0 * x_margin;
        let (_, tab_rect) = ui.allocate_space(egui::vec2(width, ui.available_height()));

        let response = ui
            .interact(tab_rect, id, egui::Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Grab);

        if !state.is_being_dragged {
            let painter = ui.painter();
            if state.active {
                painter.rect_filled(tab_rect, 0.0, colors::PANEL_BG);
                // 顶部 2px 强调线：UE 选中页签的标志。
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        tab_rect.left_top(),
                        egui::pos2(tab_rect.right(), tab_rect.top() + 2.0),
                    ),
                    0.0,
                    colors::ACCENT_LIGHT,
                );
                // 与内容区连成一片：擦掉页签条与内容之间的那条分隔线。
                painter.hline(
                    tab_rect.x_range(),
                    tab_rect.bottom(),
                    egui::Stroke::new(1.0_f32, colors::PANEL_BG),
                );
            } else if response.hovered() {
                painter.rect_filled(tab_rect, 0.0, colors::TAB_HOVER_BG);
            }

            let text_color = if state.active {
                colors::TEXT_BRIGHT
            } else if response.hovered() {
                colors::TEXT_PRIMARY
            } else {
                colors::TEXT_SECONDARY
            };
            let text_pos = egui::Align2::LEFT_CENTER
                .align_size_within_rect(galley.size(), tab_rect.shrink2(egui::vec2(x_margin, 0.0)))
                .min;
            painter.galley(text_pos, galley, text_color);
        }

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个 `PanelTab` 变体在布局里都必须有一个 pane —— 漏掉一个变体的表现是
    /// "能编译、能跑，但那个面板永远打不开"，`Panel` 派生也不会报错。
    #[test]
    fn every_tab_is_in_the_tree() {
        let tree = create_tile_tree();
        for tab in PanelTab::ALL {
            assert!(
                tree.tiles.find_pane(&tab).is_some(),
                "{tab:?} 在布局里没有对应的 pane"
            );
        }
        assert!(is_usable_layout(&tree), "出厂布局必须通过恢复校验");
    }

    /// 少一个面板的布局不可以用：恢复它会让那个面板永远打不开。
    #[test]
    fn layout_missing_a_pane_is_rejected() {
        let full = create_tile_tree();
        let profiler = full.tiles.find_pane(&PanelTab::Profiler).unwrap();

        let mut tiles = Tiles::default();
        let hierarchy = tiles.insert_pane(PanelTab::Hierarchy);
        let assets = tiles.insert_pane(PanelTab::Assets);
        let prefabs = tiles.insert_pane(PanelTab::Prefabs);
        let scene = tiles.insert_pane(PanelTab::SceneView);
        let inspector = tiles.insert_pane(PanelTab::Inspector);
        let console = tiles.insert_pane(PanelTab::Console);
        let left = tiles.insert_tab_tile(vec![hierarchy, assets, prefabs]);
        let right = tiles.insert_tab_tile(vec![scene, inspector, console]);
        let root = tiles.insert_tab_tile(vec![left, right]);
        let broken = Tree::new("editor_tree", root, tiles);

        assert_ne!(
            broken.tiles.find_pane(&PanelTab::Profiler),
            Some(profiler),
            "测试布局本来就不该有 Profiler"
        );
        assert!(!is_usable_layout(&broken));
        assert!(is_usable_layout(&create_tile_tree()));
    }

    /// 空树 / 根节点悬空的树不能通过校验（`Tree::ui` 会假定根存在）。
    #[test]
    fn empty_or_dangling_layout_is_rejected() {
        let mut empty_tiles = Tiles::default();
        let empty_root = empty_tiles.insert_tab_tile(Vec::new());
        let mut empty = Tree::new("editor_tree", empty_root, empty_tiles);
        empty.root = None; // `Tree::new` 不接受空根，但磁盘上的空树可以长这样。
        assert!(!is_usable_layout(&empty));

        let mut tiles = Tiles::default();
        let pane = tiles.insert_pane(PanelTab::Hierarchy);
        let root = tiles.insert_tab_tile(vec![pane]);
        let dangling = Tree::new("editor_tree", root, tiles);
        // 只有一个 pane 也不够：必须七个都在。
        assert!(!is_usable_layout(&dangling));
    }

    /// 同一个 pane 被挂两次：布局能构造出来，但 `Tree::ui` 会把它当两张页签，
    /// 保存后仍是一棵坏树。校验必须把重复引用挡住（`seen` 与 `panes` 两道集合
    /// 各自覆盖「同一 tile 被走两次」与「同一 pane 内容出现两次」）。
    #[test]
    fn duplicated_pane_is_rejected() {
        let mut tiles = Tiles::default();
        let ids = PanelTab::ALL
            .iter()
            .map(|pane| tiles.insert_pane(pane.clone()))
            .collect::<Vec<_>>();
        let mut children = ids.clone();
        children.push(ids[0]);
        let root = tiles.insert_tab_tile(children);
        let tree = Tree::new("editor_tree", root, tiles);

        assert!(
            !is_usable_layout(&tree),
            "七个面板都在、但有一个被重复引用，仍然不是合法布局"
        );
    }

    /// 不可达的孤儿 tile 也要拒绝：它们不参与显示，却会跟着下次保存继续堆。
    #[test]
    fn orphan_tile_is_rejected() {
        let mut tree = create_tile_tree();
        let _ = tree.tiles.insert_pane(PanelTab::Hierarchy);
        assert!(!is_usable_layout(&tree));
    }

    /// Profiler 与 Output Log 同组：默认激活的必须是 Output Log（老截图逐像素不变），
    /// 而 `activate` 能把它切上来（`--ui-profiler` 的唯一依赖）。
    #[test]
    fn profiler_shares_tabs_with_console_and_can_be_activated() {
        let mut tree = create_tile_tree();
        let console = tree.tiles.find_pane(&PanelTab::Console).unwrap();
        let profiler = tree.tiles.find_pane(&PanelTab::Profiler).unwrap();

        let tab_tile = |id: TileId| match tree.tiles.get(id) {
            Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) => {
                Some(tabs.children.clone())
            }
            _ => None,
        };
        // 两者挂在同一个 tab 容器下（页签组），而不是各自独占一格。
        let shared = tree
            .tiles
            .iter()
            .find_map(|(_, tile)| match tile {
                egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs)) => {
                    (tabs.children.contains(&console) && tabs.children.contains(&profiler))
                        .then(|| tabs.children.clone())
                }
                _ => None,
            })
            .expect("Console 与 Profiler 应当在同一个页签组里");
        assert_eq!(
            shared.first().copied(),
            Some(console),
            "默认激活的应当是 Output Log"
        );
        assert!(tab_tile(profiler).is_none(), "pane 本身不是 tab 容器");

        assert!(tree.make_active(
            |_, tile| matches!(tile, egui_tiles::Tile::Pane(p) if *p == PanelTab::Profiler)
        ));
        // 激活之后，那个 tab 容器的 active 应当换成 Profiler。
        let active = tree
            .tiles
            .iter()
            .find_map(|(_, tile)| match tile {
                egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs)) => {
                    tabs.children.contains(&profiler).then_some(tabs.active)
                }
                _ => None,
            })
            .flatten();
        assert_eq!(active, Some(profiler));
    }
}
