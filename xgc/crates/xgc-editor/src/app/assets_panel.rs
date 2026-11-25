//! 项目资产浏览只回填预制体路径；选中条目不修改世界，不自动实例化。

use eframe::egui;

use super::EditorApp;
use crate::{assets_ops::MAX_ASSETS, AssetKind};

#[cfg(test)]
mod tests;

impl EditorApp {
    pub(super) fn assets_panel(&mut self, ui: &mut egui::Ui) {
        if self.state.project_root().is_none() {
            return;
        }
        egui::CollapsingHeader::new("Assets")
            .id_salt("xgc-assets")
            .default_open(false)
            .show(ui, |ui| {
                // 折叠时不扫盘；展开后只按用户刷新重扫，避免把目录 I/O 塞进重绘。
                if self.assets.is_none() {
                    self.refresh_assets();
                }
                ui.horizontal(|ui| {
                    if ui.small_button("Refresh").clicked() {
                        self.refresh_assets();
                    }
                    ui.label(format!(
                        "{} assets",
                        self.assets.as_ref().map_or(0, Vec::len)
                    ));
                });
                let Some(assets) = &self.assets else {
                    return;
                };
                if assets.is_empty() {
                    ui.label("No assets in project content folders.");
                }
                if assets.len() == MAX_ASSETS {
                    ui.label(format!("Display limit: {MAX_ASSETS} assets"));
                }
                let row_height = ui.spacing().interact_size.y;
                let max_height = (row_height * 8.0).min(ui.available_height() * 0.4);
                egui::ScrollArea::vertical()
                    .id_salt("xgc-assets-scroll")
                    .max_height(max_height)
                    .show_rows(ui, row_height, assets.len(), |ui, rows| {
                        for asset in &assets[rows] {
                            let label = format!("[{}] {}", asset.kind.label(), asset.path);
                            let selected = self.selected_asset.as_deref() == Some(&asset.path);
                            if ui
                                .add(
                                    egui::Button::new(label)
                                        .selected(selected)
                                        .frame(false)
                                        .truncate(),
                                )
                                .on_hover_text(&asset.path)
                                .clicked()
                            {
                                self.selected_asset = Some(asset.path.clone());
                                if asset.kind == AssetKind::Prefab {
                                    self.prefab_path = asset.path.clone();
                                }
                                self.status = Some(format!("asset: {}", asset.path));
                            }
                        }
                    });
            });
        ui.separator();
    }

    fn refresh_assets(&mut self) {
        let assets = self.state.assets();
        if self
            .selected_asset
            .as_ref()
            .is_some_and(|selected| !assets.iter().any(|asset| &asset.path == selected))
        {
            self.selected_asset = None;
        }
        self.assets = Some(assets);
    }
}
