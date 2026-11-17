//! 旧 `UiElement` RON → `ui.element` 组件数据。

use serde::Deserialize;
use xgc_world::{Component, ComponentData, UiElement, Value};

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldStyle {
    background: [f32; 4],
    border_color: [f32; 4],
    border_width: f32,
    corner_radius: f32,
    padding: f32,
}

impl Default for OldStyle {
    fn default() -> Self {
        Self {
            background: [0.0; 4],
            border_color: [0.2745, 0.2902, 0.3294, 1.0],
            border_width: 0.0,
            corner_radius: 4.0,
            padding: 0.0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldText {
    content: String,
    size: f32,
    color: [f32; 4],
    bold: bool,
    wrap: bool,
}

impl Default for OldText {
    fn default() -> Self {
        Self {
            content: String::new(),
            size: 20.0,
            color: [1.0; 4],
            bold: false,
            wrap: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct OldImage {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldUi {
    kind: Option<ron::Value>,
    anchor_min: [f32; 2],
    anchor_max: [f32; 2],
    offset_min: [f32; 2],
    offset_max: [f32; 2],
    style: OldStyle,
    text: OldText,
    image: Option<OldImage>,
    visible: bool,
    interactable: bool,
    clip_children: bool,
}

impl Default for OldUi {
    fn default() -> Self {
        Self {
            kind: None,
            anchor_min: [0.0; 2],
            anchor_max: [0.0; 2],
            offset_min: [0.0; 2],
            offset_max: [100.0, 40.0],
            style: OldStyle::default(),
            text: OldText::default(),
            image: None,
            visible: true,
            interactable: true,
            clip_children: true,
        }
    }
}

pub fn convert(value: ron::Value) -> Result<ComponentData, String> {
    let old: OldUi = value
        .into_rust()
        .map_err(|err| format!("ui.element 解析失败：{err}"))?;
    let image_path = old.image.map(|image| image.path).unwrap_or_default();
    let kind = old
        .kind
        .and_then(|kind| match kind {
            ron::Value::String(kind) => Some(kind.to_lowercase()),
            _ => None,
        })
        .unwrap_or_else(|| infer_kind(&image_path, &old.text.content, old.style.background[3]));
    // 旧端的 align / valign 是 RON 单元枚举，`ron::Value` 转换会把变体名丢成
    // unit（`into_rust` 无法还原）；只能按 kind 推断：按钮文案居中，其余左对齐，
    // 垂直一律居中（= 旧端 `UiVAlign::Middle` 缺省）。
    let text_align = if kind == "button" { "center" } else { "left" };
    Ok(ComponentData {
        component: UiElement::ID,
        fields: vec![
            ("kind".to_string(), Value::String(kind)),
            ("anchor_min".to_string(), Value::Vec2(vec2(old.anchor_min))),
            ("anchor_max".to_string(), Value::Vec2(vec2(old.anchor_max))),
            ("offset_min".to_string(), Value::Vec2(vec2(old.offset_min))),
            ("offset_max".to_string(), Value::Vec2(vec2(old.offset_max))),
            ("visible".to_string(), Value::Bool(old.visible)),
            ("interactable".to_string(), Value::Bool(old.interactable)),
            (
                "background".to_string(),
                Value::Vec4(vec4(old.style.background)),
            ),
            (
                "border_color".to_string(),
                Value::Vec4(vec4(old.style.border_color)),
            ),
            (
                "border_width".to_string(),
                Value::F64(f64::from(old.style.border_width)),
            ),
            (
                "corner_radius".to_string(),
                Value::F64(f64::from(old.style.corner_radius)),
            ),
            (
                "padding".to_string(),
                Value::F64(f64::from(old.style.padding)),
            ),
            ("text".to_string(), Value::String(old.text.content)),
            (
                "text_size".to_string(),
                Value::F64(f64::from(old.text.size)),
            ),
            ("text_color".to_string(), Value::Vec4(vec4(old.text.color))),
            ("bold".to_string(), Value::Bool(old.text.bold)),
            ("wrap".to_string(), Value::Bool(old.text.wrap)),
            // 旧端的 align / valign 是 RON 单元枚举，`ron::Value` 转换会把变体名
            // 丢成 unit（`into_rust` 无法还原）；只能按 kind 推断：按钮文案居中，
            // 其余左对齐，垂直一律居中（= 旧端 `UiVAlign::Middle` 缺省）。
            (
                "text_align".to_string(),
                Value::String(text_align.to_string()),
            ),
            (
                "text_valign".to_string(),
                Value::String("middle".to_string()),
            ),
            ("image_path".to_string(), Value::String(image_path)),
            ("clip_children".to_string(), Value::Bool(old.clip_children)),
        ],
    })
}

/// 旧 `UiKind` 是 RON 单元枚举，`ron::Value` 会丢掉变体名；按内容推断。
fn infer_kind(image_path: &str, text: &str, background_alpha: f32) -> String {
    if !image_path.is_empty() {
        "image".to_string()
    } else if !text.is_empty() && background_alpha > 0.0 {
        "button".to_string()
    } else if !text.is_empty() {
        "text".to_string()
    } else {
        "panel".to_string()
    }
}

fn vec2(value: [f32; 2]) -> [f64; 2] {
    [f64::from(value[0]), f64::from(value[1])]
}

fn vec4(value: [f32; 4]) -> [f64; 4] {
    [
        f64::from(value[0]),
        f64::from(value[1]),
        f64::from(value[2]),
        f64::from(value[3]),
    ]
}
