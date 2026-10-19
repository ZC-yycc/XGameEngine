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
    Prefabs,
    SceneView,
    Inspector,
    Console,
}

impl PanelTab {
    /// 页签上显示的名字。
    ///
    /// 与枚举名分开：枚举名是代码里的稳定标识（`SceneView`），页签名是**给人看的**
    /// （UE 的视口叫 Viewport、日志面板叫 Output Log）。改名字不该牵动代码引用。
    pub fn title(&self) -> &'static str {
        match self {
            PanelTab::Hierarchy => "Hierarchy",
            PanelTab::Prefabs => "Prefabs",
            PanelTab::SceneView => "Viewport",
            PanelTab::Inspector => "Inspector",
            PanelTab::Console => "Output Log",
        }
    }
}

pub fn create_tile_tree() -> Tree<PanelTab> {
    let mut tiles = Tiles::default();

    // Insert pane tiles
    let hier = tiles.insert_pane(PanelTab::Hierarchy);
    let prefs = tiles.insert_pane(PanelTab::Prefabs);
    let scene = tiles.insert_pane(PanelTab::SceneView);
    let insp = tiles.insert_pane(PanelTab::Inspector);
    let cons = tiles.insert_pane(PanelTab::Console);

    // ── Build containers with custom shares ──

    // Left column: Hierarchy (60%) | Prefabs (40%)
    // 与 Unity 一致：层级树在上，资源浏览器（预制体）在下。
    let left_column = tiles.insert_container(
        Linear::new(LinearDir::Vertical, vec![hier, prefs])
            .with_share(hier, 0.60)
            .with_share(prefs, 0.40),
    );

    // Top row: Scene View (75%) | Inspector (25%)
    let top_row = tiles.insert_container(
        Linear::new(LinearDir::Horizontal, vec![scene, insp])
            .with_share(scene, 0.75)
            .with_share(insp, 0.25),
    );

    // Right column: top_row (75%) | Console (25%)
    let right_column = tiles.insert_container(
        Linear::new(LinearDir::Vertical, vec![top_row, cons])
            .with_share(top_row, 0.75)
            .with_share(cons, 0.25),
    );

    // Root: left_column (22%) | right_column (78%)
    let root = tiles.insert_container(
        Linear::new(LinearDir::Horizontal, vec![left_column, right_column])
            .with_share(left_column, 0.22)
            .with_share(right_column, 0.78),
    );

    Tree::new("editor_tree", root, tiles)
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
