//! 最小 eframe 编辑器：顶部工具栏、左侧层级、中央 Inspector。

use std::path::PathBuf;

use eframe::egui;

use crate::{EditorState, FieldRow};

pub struct EditorApp {
    state: EditorState,
    status: Option<String>,
}

impl EditorApp {
    pub fn new(state: EditorState) -> Self {
        Self {
            state,
            status: None,
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                self.status = Some(match self.state.save() {
                    Ok(()) => "已保存".to_string(),
                    Err(err) => err,
                });
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

    fn hierarchy(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hierarchy");
        let selected = self.state.selected();
        let rows = self.state.entities();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for row in rows {
                let label = match row.parent {
                    Some(parent) => format!("{}  (parent {})", row.name, parent.raw()),
                    None => row.name,
                };
                if ui
                    .selectable_label(
                        selected == Some(row.id),
                        format!("{}  {}", row.id.raw(), label),
                    )
                    .clicked()
                {
                    self.state.select(Some(row.id));
                }
            }
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Inspector");
        let Some(id) = self.state.selected() else {
            ui.label("未选择实体");
            return;
        };
        ui.label(format!("Entity {}", id.raw()));
        for component in self.state.inspector() {
            egui::CollapsingHeader::new(&component.name)
                .default_open(true)
                .show(ui, |ui| {
                    for field in &component.fields {
                        field_row(ui, field);
                    }
                });
        }
    }
}

fn field_row(ui: &mut egui::Ui, field: &FieldRow) {
    ui.horizontal(|ui| {
        ui.label(format!("{} ({})", field.name, field.ty));
        ui.label(format!("{:?}", field.value));
    });
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("xgc-toolbar").show(ctx, |ui| self.toolbar(ui));
        egui::SidePanel::left("xgc-hierarchy")
            .default_width(260.0)
            .show(ctx, |ui| self.hierarchy(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.inspector(ui));
    }
}

pub fn run(path: Option<PathBuf>) -> eframe::Result {
    let state = match path {
        Some(path) => match EditorState::open(&path) {
            Ok(state) => state,
            Err(err) => {
                eprintln!("{err}");
                return Ok(());
            }
        },
        None => EditorState::default(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "XGC Editor",
        options,
        Box::new(|_cc| Ok(Box::new(EditorApp::new(state)))),
    )
}
