//! egui_tiles integration — panel tab types, tree layout, and behavior.
//!
//! Builds a Unity-inspired split layout with draggable tab headers.
//! Each pane is wrapped in a `Tabs` container so the tab bar acts as
//! a drag handle for docking / undocking.

use egui_tiles::{
    Behavior, SimplificationOptions, Tiles, TileId, UiResponse, Tree,
    Linear, LinearDir,
};
use crate::panels::state::EditorState;

/// All dockable panel types in the editor.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelTab {
    Hierarchy,
    SceneView,
    Inspector,
    Console,
}

impl PanelTab {
    pub fn title(&self) -> &'static str {
        match self {
            PanelTab::Hierarchy => "Hierarchy",
            PanelTab::SceneView => "Scene View",
            PanelTab::Inspector => "Inspector",
            PanelTab::Console => "Console",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            PanelTab::Hierarchy => "☰",
            PanelTab::SceneView => "🎥",
            PanelTab::Inspector => "🔍",
            PanelTab::Console => "⎚",
        }
    }
}

/// Build the initial Unity-style tile tree with proper share ratios.
///
/// Layout (approximate width/height shares):
///
/// ┌────────────┬──────────────────────┬──────────────┐
/// │ Hierarchy  │  Scene View          │  Inspector   │
/// │  (20%)     │  (60% of remaining)  │  (20%)       │
/// │            ├──────────────────────┤              │
/// │            │  Console             │              │
/// │            │  (25% of remaining)  │              │
/// └────────────┴──────────────────────┴──────────────┘
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

/// egui_tiles [`Behavior`] — maps each panel tab to its content.
pub struct EditorBehavior<'a> {
    pub state: &'a mut EditorState,
}

impl Behavior<PanelTab> for EditorBehavior<'_> {
    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: TileId,
        pane: &mut PanelTab,
    ) -> UiResponse {
        match pane {
            PanelTab::Hierarchy => crate::panels::hierarchy::show_content(self.state, ui),
            PanelTab::SceneView => crate::panels::scene_view::show_content(self.state, ui),
            PanelTab::Inspector => crate::panels::inspector::show_content(self.state, ui),
            PanelTab::Console => crate::panels::console::show_content(self.state, ui),
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &PanelTab) -> egui::WidgetText {
        format!("{}  {}", pane.icon(), pane.title()).into()
    }

    fn is_tab_closable(&self, _tiles: &Tiles<PanelTab>, _tile_id: TileId) -> bool {
        false
    }

    /// **Critical**: wrap every pane in a `Tabs` container so the tab bar
    /// serves as a drag handle for reordering, splitting, and undocking.
    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..SimplificationOptions::default()
        }
    }
}
