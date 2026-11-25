//! Map 与枚举沿用字段编辑事务，容器结构改动由整值替换表达。

use std::collections::BTreeMap;

use eframe::egui;
use xgc_reflect::{FieldPath, FieldSchema, Value};

use super::{field_row, show_metadata, FieldEdit};

pub(super) fn map_rows(
    ui: &mut egui::Ui,
    schema: &FieldSchema,
    entries: &BTreeMap<String, Value>,
    label: &str,
    path: &str,
    editable: bool,
) -> Option<FieldEdit> {
    let response = egui::CollapsingHeader::new(format!("{label} ({})", entries.len()))
        .id_salt("map")
        .default_open(true)
        .show(ui, |ui| {
            let element = schema.element.as_deref()?;
            let mut edit = None;
            for (key, value) in entries {
                let segment = FieldPath::key_segment(key);
                let entry_path = format!("{path}{segment}");
                if let Some(entry_edit) =
                    field_row(ui, element, value, &segment, &entry_path, editable)
                {
                    edit.get_or_insert(entry_edit);
                }
                let remove = ui
                    .push_id(("remove-entry", key), |ui| {
                        ui.add_enabled(
                            editable,
                            egui::Button::new(format!("Remove {segment}")).small(),
                        )
                        .clicked()
                    })
                    .inner;
                if remove {
                    let mut next = entries.clone();
                    next.remove(key);
                    edit.get_or_insert(FieldEdit {
                        path: path.to_string(),
                        value: Value::Map(next),
                    });
                }
            }
            let key = ui
                .add_enabled_ui(editable, |ui| map_key_input(ui, entries))
                .inner;
            if let Some(key) = key {
                let mut next = entries.clone();
                next.insert(key, element.default.clone());
                edit.get_or_insert(FieldEdit {
                    path: path.to_string(),
                    value: Value::Map(next),
                });
            }
            edit
        });
    show_metadata(response.header_response, schema);
    response.body_returned.flatten()
}

fn map_key_input(ui: &mut egui::Ui, entries: &BTreeMap<String, Value>) -> Option<String> {
    // 键草稿跨帧保留，只有点添加才提交字段事务。
    let id = ui.make_persistent_id("map-key");
    let mut key = ui
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_default();
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut key)
                .id(id)
                .hint_text("Key")
                .desired_width(ui.available_width()),
        )
        .changed();
    // 键按原文比较，重复的键只允许通过对应行修改其值。
    if ui
        .add_enabled(
            !entries.contains_key(&key),
            egui::Button::new("Add entry").small(),
        )
        .clicked()
    {
        ui.data_mut(|data| data.remove::<String>(id));
        return Some(key);
    }
    if changed {
        ui.data_mut(|data| data.insert_temp(id, key));
    }
    None
}

pub(super) fn enum_rows(
    ui: &mut egui::Ui,
    schema: &FieldSchema,
    variant: &str,
    payload: &Value,
    label: &str,
    path: &str,
    editable: bool,
) -> Option<FieldEdit> {
    let switch = ui
        .horizontal(|ui| {
            show_metadata(ui.label(label), schema);
            ui.add_enabled_ui(editable, |ui| {
                egui::ComboBox::from_id_salt("variant")
                    .selected_text(variant)
                    .show_ui(ui, |ui| {
                        if !editable {
                            ui.disable();
                        }
                        let mut switch = None;
                        for option in &schema.variants {
                            let selected = option.name == variant;
                            if ui.selectable_label(selected, &option.name).clicked() && !selected {
                                switch = Some(option);
                            }
                        }
                        switch
                    })
                    .inner
                    .flatten()
            })
            .inner
        })
        .inner;
    if let Some(next) = switch {
        // 变体名和缺省载荷同一次提交，避免中间状态把旧载荷写给新变体。
        return Some(FieldEdit {
            path: path.to_string(),
            value: Value::Enum {
                variant: next.name.clone(),
                payload: Box::new(next.default.clone()),
            },
        });
    }
    let active = schema.variant(variant)?;
    if matches!(payload, Value::Struct(fields) if fields.is_empty()) {
        return None;
    }
    let payload_path = format!("{path}.{variant}");
    field_row(ui, active, payload, variant, &payload_path, editable)
}
