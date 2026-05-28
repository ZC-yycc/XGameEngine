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
use crate::state::EditorState;



/// 编辑器中所有可停靠的面板类型。
#[derive(Debug, Clone, Display, PartialEq, Panel)]
pub enum PanelTab {
    Hierarchy,
    SceneView,
    Inspector,
    Console,
}

pub fn create_tile_tree() -> Tree<PanelTab> {
    let mut tiles = Tiles::default();

    // Insert pane tiles
    let hier = tiles.insert_pane(PanelTab::Hierarchy);
    let scene = tiles.insert_pane(PanelTab::SceneView);
    let insp = tiles.insert_pane(PanelTab::Inspector);
    let cons = tiles.insert_pane(PanelTab::Console);

    // ── Build containers with custom shares ──

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

    // Root: Hierarchy (20%) | right_column (80%)
    let root = tiles.insert_container(
        Linear::new(LinearDir::Horizontal, vec![hier, right_column])
            .with_share(hier, 0.20)
            .with_share(right_column, 0.80),
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
        // 8px inner margin so panel elements don't sit flush against the tile edges.
        egui::Frame::default()
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                pane.show(self.state, ui);
            });
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &PanelTab) -> egui::WidgetText {
        format!("{}", pane).into()
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
}
