//! 最小 eframe 编辑器：顶部工具栏、左侧层级、中央 Inspector。

use std::path::PathBuf;

use eframe::egui;

use crate::{EditorState, FieldRow};
use xgc_reflect::Value;

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
        let names = self.state.component_names();
        let mut chosen = None;
        egui::ComboBox::from_id_salt("xgc-add-component")
            .selected_text("Add Component")
            .show_ui(ui, |ui| {
                for name in &names {
                    if ui.selectable_label(false, name).clicked() {
                        chosen = Some(name.clone());
                    }
                }
            });
        if let Some(name) = chosen {
            self.status = Some(match self.state.add_component(id, &name) {
                Ok(()) => format!("已添加 {name}"),
                Err(err) => err,
            });
        }
        let components = self.state.inspector();
        for component in components {
            let mut remove = false;
            ui.horizontal(|ui| {
                ui.strong(&component.name);
                if ui.small_button("Remove").clicked() {
                    remove = true;
                }
            });
            if remove {
                self.status = Some(match self.state.remove_component(id, component.id) {
                    Ok(()) => format!("已移除 {}", component.name),
                    Err(err) => err,
                });
                continue;
            }
            for field in &component.fields {
                field_row(ui, &mut self.state, id, component.id, field);
            }
            ui.separator();
        }
    }
}

fn field_row(
    ui: &mut egui::Ui,
    state: &mut EditorState,
    entity: xgc_base::EntityId,
    component: u32,
    field: &FieldRow,
) {
    ui.horizontal(|ui| {
        ui.label(format!("{} ({})", field.name, field.ty));
        let Some(edited) = value_editor(ui, &field.value, &field.ty) else {
            return;
        };
        if edited != field.value {
            if let Err(err) = state.edit_field(entity, component, &field.name, edited) {
                ui.colored_label(egui::Color32::RED, err);
            }
        }
    });
}

fn value_editor(ui: &mut egui::Ui, value: &Value, ty: &str) -> Option<Value> {
    match value {
        Value::Bool(current) => {
            let mut edited = *current;
            ui.checkbox(&mut edited, "");
            edited.ne(current).then_some(Value::Bool(edited))
        }
        Value::I64(current) => {
            let mut edited = *current;
            ui.add(egui::DragValue::new(&mut edited).speed(1.0));
            edited.ne(current).then_some(Value::I64(edited))
        }
        Value::F64(current) => {
            let mut edited = *current;
            ui.add(egui::DragValue::new(&mut edited).speed(0.01));
            edited.ne(current).then_some(Value::F64(edited))
        }
        Value::String(current) => {
            let mut edited = current.clone();
            ui.text_edit_singleline(&mut edited);
            edited.ne(current).then_some(Value::String(edited))
        }
        Value::Entity(current) => {
            ui.label(format!("entity {current}"));
            None
        }
        Value::Vec2(current) => {
            let mut edited = *current;
            for component in &mut edited {
                ui.add(egui::DragValue::new(component).speed(0.01));
            }
            edited.ne(current).then_some(Value::Vec2(edited))
        }
        Value::Vec3(current) => {
            let mut edited = *current;
            for component in &mut edited {
                ui.add(egui::DragValue::new(component).speed(0.01));
            }
            edited.ne(current).then_some(Value::Vec3(edited))
        }
        Value::Vec4(current) | Value::Quat(current) => {
            let mut edited = *current;
            for component in &mut edited {
                ui.add(egui::DragValue::new(component).speed(0.01));
            }
            let next = if ty == "quat" {
                Value::Quat(edited)
            } else {
                Value::Vec4(edited)
            };
            (edited != *current).then_some(next)
        }
    }
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
