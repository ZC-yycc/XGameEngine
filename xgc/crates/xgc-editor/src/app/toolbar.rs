//! 顶部工具栏：保存 / 预制体 / Play / Gizmo / 吸附。

use eframe::egui;

use super::{EditorApp, GizmoMode};

impl EditorApp {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("+ Entity").clicked() {
                self.status = Some(match self.state.spawn_entity("entity") {
                    Ok(id) => format!("已新建实体 {}", id.raw()),
                    Err(err) => err,
                });
            }
            if ui.button("Delete").clicked() {
                self.status = Some(match self.state.delete_selected() {
                    Ok(()) => "已删除".to_string(),
                    Err(err) => err,
                });
            }
            if ui.button("Undo").clicked() {
                self.status = Some(match self.state.undo() {
                    Ok(true) => "已撤销".to_string(),
                    Ok(false) => "没有可撤销操作".to_string(),
                    Err(err) => err,
                });
            }
            if ui.button("Redo").clicked() {
                self.status = Some(match self.state.redo() {
                    Ok(true) => "已重做".to_string(),
                    Ok(false) => "没有可重做操作".to_string(),
                    Err(err) => err,
                });
            }
            ui.separator();
            ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Translate, "Move");
            ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Rotate, "Rotate");
            ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Scale, "Scale");
            ui.checkbox(&mut self.snap, "Snap");
            ui.add_enabled(
                self.snap,
                egui::DragValue::new(&mut self.grid)
                    .speed(0.05)
                    .range(0.01..=10.0),
            );
            if ui.button("Save").clicked() {
                self.status = Some(match self.state.save() {
                    Ok(()) => "已保存".to_string(),
                    Err(err) => err,
                });
            }
            ui.add(egui::TextEdit::singleline(&mut self.prefab_path).hint_text("预制体路径"));
            for value in &mut self.prefab_position {
                ui.add(egui::DragValue::new(value).speed(0.1));
            }
            if ui
                .add_enabled(
                    !self.state.is_playing() && !self.prefab_path.is_empty(),
                    egui::Button::new("Prefab"),
                )
                .clicked()
            {
                match self
                    .state
                    .instantiate_prefab_file(std::path::Path::new(&self.prefab_path))
                {
                    Ok(id) => {
                        self.status = Some(
                            match self.state.set_entity_position(id, self.prefab_position) {
                                Ok(()) => format!("已实例化 {}", id.raw()),
                                Err(err) => err,
                            },
                        );
                    }
                    Err(err) => self.status = Some(err),
                }
            }
            if ui
                .add_enabled(self.state.can_revert_prefab(), egui::Button::new("Revert"))
                .clicked()
            {
                self.status = Some(match self.state.revert_selected_prefab() {
                    Ok(count) => format!("已还原 {count} 个实体"),
                    Err(err) => err,
                });
            }
            let in_project = self.state.project_root().is_some();
            if !in_project {
                ui.add(egui::TextEdit::singleline(&mut self.script_root).hint_text("脚本根目录"));
            }
            if ui
                .add_enabled(
                    in_project || !self.script_root.is_empty(),
                    egui::Button::new("Play Scripts"),
                )
                .clicked()
            {
                self.status = Some(
                    match self
                        .state
                        .play_with_scripts(std::path::Path::new(&self.script_root))
                    {
                        Ok(()) => "Play Scripts".to_string(),
                        Err(err) => err,
                    },
                );
            }
            if ui.button("Play").clicked() {
                self.status = Some(match self.state.play() {
                    Ok(()) => "Play".to_string(),
                    Err(err) => err,
                });
            }
            if ui.button("Step").clicked() {
                self.status = Some(match self.state.step() {
                    Ok(Some(record)) => {
                        format!("frame={} hash={:016x}", record.frame, record.state_hash)
                    }
                    Ok(None) => "未在 Play".to_string(),
                    Err(err) => err,
                });
            }
            if ui.button("Stop").clicked() {
                self.status = Some(match self.state.stop() {
                    Ok(()) => "已停止".to_string(),
                    Err(err) => err,
                });
            }
            let mode = if self.state.is_playing() {
                "PLAY"
            } else {
                "EDIT"
            };
            ui.label(format!(
                "{mode}  dirty={}  {}",
                self.state.is_dirty(),
                self.state
                    .path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<未命名>".to_string())
            ));
        });
        if let Some(status) = &self.status {
            ui.label(status);
        }
    }
}
