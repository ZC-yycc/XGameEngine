//! egui_tiles 集成——面板标签类型、树布局和行为。
//!
//! 构建类 Unity 的拆分布局，支持可拖拽的标签头。
//! 每个面板包裹在 `Tabs` 容器中，标签栏作为停靠/取消停靠的拖拽手柄。

use egui_tiles::{
    Behavior, SimplificationOptions, Tiles, TileId, UiResponse, Tree,
    Linear, LinearDir,
};
use strum::Display;
use xgame_derive::Panel;
use crate::panels::theme;
use crate::state::EditorState;



/// 编辑器中所有可停靠的面板类型。
#[derive(Debug, Clone, Display, PartialEq, Panel)]
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

/// 把某个面板页签切到前台。
///
/// 给 `--ui-profiler` 这类截图开关用：面板在布局里存在，但默认不是激活页签时，
/// 截出来的就是邻居的内容 —— "改了界面却截不到"最容易发生在同一组的第二个页签上。
///
/// `make_active` 只按谓词命中 pane，祖先容器的激活链由 egui_tiles 自己补，
/// 所以这里不需要知道这棵树是怎么分组的（布局再改也不会失配）。
pub fn activate(tree: &mut Tree<PanelTab>, pane: PanelTab) {
    let hit = tree.make_active(|_, tile| {
        matches!(tile, egui_tiles::Tile::Pane(p) if *p == pane)
    });
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
        let all = [
            PanelTab::Hierarchy,
            PanelTab::Assets,
            PanelTab::Prefabs,
            PanelTab::SceneView,
            PanelTab::Inspector,
            PanelTab::Console,
            PanelTab::Profiler,
        ];
        for tab in all {
            assert!(
                tree.tiles.find_pane(&tab).is_some(),
                "{tab:?} 在布局里没有对应的 pane"
            );
        }
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
        assert_eq!(shared.first().copied(), Some(console), "默认激活的应当是 Output Log");
        assert!(tab_tile(profiler).is_none(), "pane 本身不是 tab 容器");

        assert!(tree.make_active(|_, tile| matches!(tile, egui_tiles::Tile::Pane(p) if *p == PanelTab::Profiler)));
        // 激活之后，那个 tab 容器的 active 应当换成 Profiler。
        let active = tree
            .tiles
            .iter()
            .find_map(|(_, tile)| match tile {
                egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs)) => tabs
                    .children
                    .contains(&profiler)
                    .then_some(tabs.active),
                _ => None,
            })
            .flatten();
        assert_eq!(active, Some(profiler));
    }
}
