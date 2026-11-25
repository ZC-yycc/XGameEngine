//! Inspector 只按反射 schema 绘制字段，遍历结束后才提交命令。

use eframe::egui;
use xgc_reflect::{FieldSchema, Value};

use super::EditorApp;

#[cfg(test)]
mod tests;

struct FieldEdit {
    path: String,
    value: Value,
}

enum InspectorAction {
    Add(String),
    Remove { component: u32, name: String },
    Edit { component: u32, field: FieldEdit },
}

impl EditorApp {
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Inspector");
        let Some(entity) = self.state.selected() else {
            ui.label("未选择实体");
            return;
        };
        ui.label(format!("Entity {}", entity.raw()));
        let editable = !self.state.is_playing();
        let mut action = None;
        ui.add_enabled_ui(editable, |ui| {
            egui::ComboBox::from_id_salt("xgc-add-component")
                .selected_text("Add Component")
                .show_ui(ui, |ui| {
                    for name in self.state.component_names() {
                        if ui.selectable_label(false, &name).clicked() {
                            action.get_or_insert(InspectorAction::Add(name));
                        }
                    }
                });
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            for component in self.state.inspector() {
                ui.push_id((entity.raw(), component.id), |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&component.name);
                        if ui
                            .add_enabled(editable, egui::Button::new("Remove").small())
                            .clicked()
                        {
                            action.get_or_insert(InspectorAction::Remove {
                                component: component.id,
                                name: component.name.clone(),
                            });
                        }
                    });
                    for field in component.fields {
                        let name = &field.schema.name;
                        if let Some(field) =
                            field_row(ui, &field.schema, &field.value, name, name, editable)
                        {
                            action.get_or_insert(InspectorAction::Edit {
                                component: component.id,
                                field,
                            });
                        }
                    }
                    ui.separator();
                });
            }
        });
        // 所有控件看同一份快照；增删列表项时，索引不会在遍历中途移动。
        if let Some(action) = action {
            self.status = match action {
                InspectorAction::Add(name) => Some(match self.state.add_component(entity, &name) {
                    Ok(()) => format!("已添加 {name}"),
                    Err(err) => err,
                }),
                InspectorAction::Remove { component, name } => {
                    Some(match self.state.remove_component(entity, component) {
                        Ok(()) => format!("已移除 {name}"),
                        Err(err) => err,
                    })
                }
                InspectorAction::Edit { component, field } => self
                    .state
                    .edit_field(entity, component, &field.path, field.value)
                    .err(),
            };
            ui.ctx().request_repaint();
        }
    }
}

fn field_row(
    ui: &mut egui::Ui,
    schema: &FieldSchema,
    value: &Value,
    label: &str,
    path: &str,
    editable: bool,
) -> Option<FieldEdit> {
    ui.push_id(path, |ui| match value {
        Value::Struct(fields) => {
            let response = egui::CollapsingHeader::new(label)
                .default_open(true)
                .show(ui, |ui| {
                    let mut edit = None;
                    for child in &schema.fields {
                        let Some(value) = fields.get(&child.name) else {
                            continue;
                        };
                        let child_path = format!("{path}.{}", child.name);
                        if let Some(child_edit) =
                            field_row(ui, child, value, &child.name, &child_path, editable)
                        {
                            edit.get_or_insert(child_edit);
                        }
                    }
                    edit
                });
            show_metadata(response.header_response, schema);
            response.body_returned.flatten()
        }
        Value::List(items) => list_rows(ui, schema, items, label, path, editable),
        leaf => {
            ui.horizontal(|ui| {
                let label = match &schema.unit {
                    Some(unit) => format!("{label} ({}, {unit})", schema.ty),
                    None => format!("{label} ({})", schema.ty),
                };
                show_metadata(ui.label(label), schema);
                ui.add_enabled_ui(editable, |ui| value_editor(ui, leaf))
                    .inner
                    .map(|value| FieldEdit {
                        path: path.to_string(),
                        value,
                    })
            })
            .inner
        }
    })
    .inner
}

fn list_rows(
    ui: &mut egui::Ui,
    schema: &FieldSchema,
    items: &[Value],
    label: &str,
    path: &str,
    editable: bool,
) -> Option<FieldEdit> {
    let response = egui::CollapsingHeader::new(format!("{label} ({})", items.len()))
        .id_salt("list")
        .default_open(true)
        .show(ui, |ui| {
            let element = schema.element.as_deref()?;
            let mut edit = None;
            for (index, value) in items.iter().enumerate() {
                let index_label = format!("[{index}]");
                let item_path = format!("{path}{index_label}");
                if let Some(item_edit) =
                    field_row(ui, element, value, &index_label, &item_path, editable)
                {
                    edit.get_or_insert(item_edit);
                }
                if ui
                    .add_enabled(
                        editable,
                        egui::Button::new(format!("Remove {index_label}")).small(),
                    )
                    .clicked()
                {
                    let mut next = items.to_vec();
                    next.remove(index);
                    edit.get_or_insert(FieldEdit {
                        path: path.to_string(),
                        value: Value::List(next),
                    });
                }
            }
            if ui
                .add_enabled(editable, egui::Button::new("Add item").small())
                .clicked()
            {
                let mut next = items.to_vec();
                next.push(element.default.clone());
                edit.get_or_insert(FieldEdit {
                    path: path.to_string(),
                    value: Value::List(next),
                });
            }
            edit
        });
    show_metadata(response.header_response, schema);
    response.body_returned.flatten()
}

fn show_metadata(response: egui::Response, schema: &FieldSchema) {
    let mut details = schema.doc.clone();
    if let (Some(min), Some(max)) = (schema.min, schema.max) {
        if !details.is_empty() {
            details.push('\n');
        }
        details.push_str(&format!("Range {min}–{max}"));
    }
    if !details.is_empty() {
        response.on_hover_text(details);
    }
}

fn value_editor(ui: &mut egui::Ui, value: &Value) -> Option<Value> {
    match value {
        Value::Bool(current) => {
            let mut edited = *current;
            ui.checkbox(&mut edited, "")
                .changed()
                .then_some(Value::Bool(edited))
        }
        Value::I64(current) => {
            let mut edited = *current;
            ui.add(egui::DragValue::new(&mut edited).speed(1.0))
                .changed()
                .then_some(Value::I64(edited))
        }
        Value::F64(current) => {
            let mut edited = *current;
            ui.add(egui::DragValue::new(&mut edited).speed(0.01))
                .changed()
                .then_some(Value::F64(edited))
        }
        Value::String(current) => {
            let mut edited = current.clone();
            ui.add(egui::TextEdit::singleline(&mut edited).desired_width(ui.available_width()))
                .changed()
                .then_some(Value::String(edited))
        }
        Value::Entity(current) => {
            ui.label(format!("entity {current}"));
            None
        }
        Value::Vec2(current) => {
            let mut edited = *current;
            edit_vector(ui, &mut edited).then_some(Value::Vec2(edited))
        }
        Value::Vec3(current) => {
            let mut edited = *current;
            edit_vector(ui, &mut edited).then_some(Value::Vec3(edited))
        }
        Value::Vec4(current) => {
            let mut edited = *current;
            edit_vector(ui, &mut edited).then_some(Value::Vec4(edited))
        }
        Value::Quat(current) => {
            let mut edited = *current;
            edit_vector(ui, &mut edited).then_some(Value::Quat(edited))
        }
        Value::List(_) | Value::Struct(_) => unreachable!("容器字段已递归展开"),
    }
}

fn edit_vector(ui: &mut egui::Ui, values: &mut [f64]) -> bool {
    let mut changed = false;
    for value in values {
        changed |= ui.add(egui::DragValue::new(value).speed(0.01)).changed();
    }
    changed
}
