//! UI 内建组件：屏幕空间矩形 + 文本 / 图片引用。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct UiElement {
    pub kind: String,
    pub anchor_min: [f32; 2],
    pub anchor_max: [f32; 2],
    pub offset_min: [f32; 2],
    pub offset_max: [f32; 2],
    pub visible: bool,
    pub interactable: bool,
    /// 背景色，**线性** RGB。旧脚本 UI 颜色 setter（`ui_set_bg_color`）收的是 sRGB、
    /// 在入口转成线性；通用字段写入（`xgc` 表的组件字段 set）按线性原值落库；场景文件也存线性值。
    /// 渲染端把它写进 sRGB 目标，由硬件做最后一次编码（别在这里再手写 sRGB）。
    pub background: [f32; 4],
    /// 边框色，线性 RGB，口径同 `background`。
    ///
    /// 已知偏差：缺省值仍是旧端 sRGB 数值（70/74/84 的编码值）而没换算成线性 ——
    /// `border_width` 缺省 0 所以当前不可见；改缺省值会动迁移出来的世界哈希，
    /// 留到专门对齐旧端 UI 缺省值的一刀。
    pub border_color: [f32; 4],
    pub border_width: f32,
    pub corner_radius: f32,
    pub padding: f32,
    pub text: String,
    pub text_size: f32,
    /// 文字色，线性 RGB，口径同 `background`。
    pub text_color: [f32; 4],
    pub bold: bool,
    pub wrap: bool,
    /// 水平对齐：left / center / right（旧端 `UiTextStyle.align`）。
    pub text_align: String,
    /// 垂直对齐：top / middle / bottom（旧端 `UiTextStyle.valign`，缺省 middle）。
    pub text_valign: String,
    pub image_path: String,
    pub image_mode: String,
    pub image_slice: f32,
    pub clip_children: bool,
}

impl Default for UiElement {
    fn default() -> Self {
        Self {
            kind: "panel".to_string(),
            anchor_min: [0.0; 2],
            anchor_max: [0.0; 2],
            offset_min: [0.0; 2],
            offset_max: [100.0, 40.0],
            visible: true,
            interactable: true,
            background: [0.0; 4],
            border_color: [0.2745, 0.2902, 0.3294, 1.0],
            border_width: 0.0,
            corner_radius: 4.0,
            padding: 0.0,
            text: String::new(),
            text_size: 20.0,
            text_color: [1.0; 4],
            bold: false,
            wrap: true,
            text_align: "left".to_string(),
            text_valign: "middle".to_string(),
            image_path: String::new(),
            image_mode: "stretch".to_string(),
            image_slice: 0.0,
            clip_children: true,
        }
    }
}

impl Component for UiElement {
    const ID: ComponentId = ComponentId(0x0601);
    const NAME: &'static str = "ui.element";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "屏幕空间 UI：矩形锚点 / 偏移、背景 / 边框、文本、图片"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("kind", FieldType::String, Value::String("panel".into()))
                .doc("panel / text / image / button"),
            FieldDescriptor::new("anchor_min", FieldType::Vec2, Value::Vec2([0.0; 2])),
            FieldDescriptor::new("anchor_max", FieldType::Vec2, Value::Vec2([0.0; 2])),
            FieldDescriptor::new("offset_min", FieldType::Vec2, Value::Vec2([0.0; 2])),
            FieldDescriptor::new("offset_max", FieldType::Vec2, Value::Vec2([100.0, 40.0])),
            FieldDescriptor::new("visible", FieldType::Bool, Value::Bool(true)),
            FieldDescriptor::new("interactable", FieldType::Bool, Value::Bool(true)),
            FieldDescriptor::new("background", FieldType::Vec4, Value::Vec4([0.0; 4])),
            FieldDescriptor::new(
                "border_color",
                FieldType::Vec4,
                Value::Vec4([0.2745, 0.2902, 0.3294, 1.0]),
            ),
            FieldDescriptor::new("border_width", FieldType::F32, Value::F64(0.0)),
            FieldDescriptor::new("corner_radius", FieldType::F32, Value::F64(4.0)),
            FieldDescriptor::new("padding", FieldType::F32, Value::F64(0.0)),
            FieldDescriptor::new("text", FieldType::String, Value::String(String::new())),
            FieldDescriptor::new("text_size", FieldType::F32, Value::F64(20.0)),
            FieldDescriptor::new("text_color", FieldType::Vec4, Value::Vec4([1.0; 4])),
            FieldDescriptor::new("bold", FieldType::Bool, Value::Bool(false)),
            FieldDescriptor::new("wrap", FieldType::Bool, Value::Bool(true)),
            FieldDescriptor::new(
                "text_align",
                FieldType::String,
                Value::String("left".into()),
            )
            .doc("left / center / right"),
            FieldDescriptor::new(
                "text_valign",
                FieldType::String,
                Value::String("middle".into()),
            )
            .doc("top / middle / bottom"),
            FieldDescriptor::new(
                "image_path",
                FieldType::String,
                Value::String(String::new()),
            ),
            FieldDescriptor::new("clip_children", FieldType::Bool, Value::Bool(true))
                .doc("是否把后代裁剪到本元素矩形"),
            FieldDescriptor::new(
                "image_mode",
                FieldType::String,
                Value::String("stretch".into()),
            )
            .doc("stretch / preserve（保持宽高比）/ cover（裁剪填满）/ nine_slice"),
            FieldDescriptor::new("image_slice", FieldType::F32, Value::F64(0.0))
                .doc("九宫格边宽（源图像素）；0 = 关闭"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "kind" => Some(Value::String(self.kind.clone())),
            "anchor_min" => Some(Value::Vec2(vec2_f64(self.anchor_min))),
            "anchor_max" => Some(Value::Vec2(vec2_f64(self.anchor_max))),
            "offset_min" => Some(Value::Vec2(vec2_f64(self.offset_min))),
            "offset_max" => Some(Value::Vec2(vec2_f64(self.offset_max))),
            "visible" => Some(Value::Bool(self.visible)),
            "interactable" => Some(Value::Bool(self.interactable)),
            "background" => Some(Value::Vec4(vec4_f64(self.background))),
            "border_color" => Some(Value::Vec4(vec4_f64(self.border_color))),
            "border_width" => Some(Value::F64(f64::from(self.border_width))),
            "corner_radius" => Some(Value::F64(f64::from(self.corner_radius))),
            "padding" => Some(Value::F64(f64::from(self.padding))),
            "text" => Some(Value::String(self.text.clone())),
            "text_size" => Some(Value::F64(f64::from(self.text_size))),
            "text_color" => Some(Value::Vec4(vec4_f64(self.text_color))),
            "bold" => Some(Value::Bool(self.bold)),
            "wrap" => Some(Value::Bool(self.wrap)),
            "text_align" => Some(Value::String(self.text_align.clone())),
            "text_valign" => Some(Value::String(self.text_valign.clone())),
            "image_path" => Some(Value::String(self.image_path.clone())),
            "image_mode" => Some(Value::String(self.image_mode.clone())),
            "image_slice" => Some(Value::F64(f64::from(self.image_slice))),
            "clip_children" => Some(Value::Bool(self.clip_children)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "kind" => self.kind = String::from(string(value, field)?),
            "anchor_min" => self.anchor_min = vec2(value, field)?,
            "anchor_max" => self.anchor_max = vec2(value, field)?,
            "offset_min" => self.offset_min = vec2(value, field)?,
            "offset_max" => self.offset_max = vec2(value, field)?,
            "visible" => self.visible = boolean(value, field)?,
            "interactable" => self.interactable = boolean(value, field)?,
            "background" => self.background = vec4(value, field)?,
            "border_color" => self.border_color = vec4(value, field)?,
            "border_width" => self.border_width = number(value, field)?,
            "corner_radius" => self.corner_radius = number(value, field)?,
            "padding" => self.padding = number(value, field)?,
            "text" => self.text = String::from(string(value, field)?),
            "text_size" => self.text_size = number(value, field)?,
            "text_color" => self.text_color = vec4(value, field)?,
            "bold" => self.bold = boolean(value, field)?,
            "wrap" => self.wrap = boolean(value, field)?,
            "text_align" => self.text_align = String::from(string(value, field)?),
            "text_valign" => self.text_valign = String::from(string(value, field)?),
            "image_path" => self.image_path = String::from(string(value, field)?),
            "clip_children" => self.clip_children = boolean(value, field)?,
            "image_mode" => {
                let mode = string(value, field)?;
                if !matches!(mode, "stretch" | "preserve" | "cover" | "nine_slice") {
                    return Err(format!("ui.element.image_mode 未知模式：{mode}"));
                }
                self.image_mode = mode.to_string();
            }
            "image_slice" => {
                self.image_slice = value.as_f32().ok_or_else(|| {
                    format!(
                        "ui.element.image_slice 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
            }
            _ => return Err(format!("ui.element 没有字段 {field}")),
        }
        Ok(())
    }
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .as_string()
        .ok_or_else(|| format!("ui.element.{field} 期望 string，得到 {}", value.type_name()))
}

fn boolean(value: &Value, field: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("ui.element.{field} 期望 bool，得到 {}", value.type_name()))
}

fn number(value: &Value, field: &str) -> Result<f32, String> {
    value
        .as_f32()
        .ok_or_else(|| format!("ui.element.{field} 期望 f32，得到 {}", value.type_name()))
}

fn vec2(value: &Value, field: &str) -> Result<[f32; 2], String> {
    let values = value
        .as_vec2()
        .ok_or_else(|| format!("ui.element.{field} 期望 vec2，得到 {}", value.type_name()))?;
    Ok([values[0], values[1]])
}

fn vec4(value: &Value, field: &str) -> Result<[f32; 4], String> {
    let values = value
        .as_vec4()
        .ok_or_else(|| format!("ui.element.{field} 期望 vec4，得到 {}", value.type_name()))?;
    Ok([values[0], values[1], values[2], values[3]])
}

fn vec2_f64(value: [f32; 2]) -> [f64; 2] {
    [f64::from(value[0]), f64::from(value[1])]
}

fn vec4_f64(value: [f32; 4]) -> [f64; 4] {
    [
        f64::from(value[0]),
        f64::from(value[1]),
        f64::from(value[2]),
        f64::from(value[3]),
    ]
}

/// 运行时布局结果：屏幕空间矩形（x / y / width / height）与可见性。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiLayout {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub visible: bool,
}

impl Default for UiLayout {
    fn default() -> Self {
        Self {
            rect: [0.0, 0.0, 100.0, 40.0],
            clip: [0.0, 0.0, 10_000.0, 10_000.0],
            visible: true,
        }
    }
}

impl Component for UiLayout {
    const ID: ComponentId = ComponentId(0x0602);
    const NAME: &'static str = "ui.layout";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "UI 布局结果：屏幕矩形 + 可见性，由 UiLayoutSystem 写入"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new(
                "rect",
                FieldType::Vec4,
                Value::Vec4([0.0, 0.0, 100.0, 40.0]),
            )
            .doc("x / y / width / height（像素）"),
            FieldDescriptor::new(
                "clip",
                FieldType::Vec4,
                Value::Vec4([0.0, 0.0, 10_000.0, 10_000.0]),
            )
            .doc("生效裁剪矩形"),
            FieldDescriptor::new("visible", FieldType::Bool, Value::Bool(true)),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "rect" => Some(Value::Vec4(vec4_f64(self.rect))),
            "clip" => Some(Value::Vec4(vec4_f64(self.clip))),
            "visible" => Some(Value::Bool(self.visible)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "rect" => self.rect = vec4(value, field)?,
            "clip" => self.clip = vec4(value, field)?,
            "visible" => self.visible = boolean(value, field)?,
            _ => return Err(format!("ui.layout 没有字段 {field}")),
        }
        Ok(())
    }
}

/// 运行时交互状态：由 UiInteractionSystem 每帧写入。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiInteraction {
    pub hovered: bool,
    pub pressed: bool,
    pub clicked: bool,
    pub focused: bool,
}

impl Component for UiInteraction {
    const ID: ComponentId = ComponentId(0x0603);
    const NAME: &'static str = "ui.interaction";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "UI 交互状态：hover / press / click，由 UiInteractionSystem 写入"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("hovered", FieldType::Bool, Value::Bool(false)),
            FieldDescriptor::new("pressed", FieldType::Bool, Value::Bool(false)),
            FieldDescriptor::new("clicked", FieldType::Bool, Value::Bool(false)),
            FieldDescriptor::new("focused", FieldType::Bool, Value::Bool(false)),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "hovered" => Some(Value::Bool(self.hovered)),
            "pressed" => Some(Value::Bool(self.pressed)),
            "clicked" => Some(Value::Bool(self.clicked)),
            "focused" => Some(Value::Bool(self.focused)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "hovered" => self.hovered = boolean(value, field)?,
            "pressed" => self.pressed = boolean(value, field)?,
            "clicked" => self.clicked = boolean(value, field)?,
            "focused" => self.focused = boolean(value, field)?,
            _ => return Err(format!("ui.interaction 没有字段 {field}")),
        }
        Ok(())
    }
}

/// 输入快照：由宿主 / Agent 写入，UI 系统消费。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiInput {
    pub cursor: [f32; 2],
    pub click: bool,
    pub nav: &'static str,
    pub confirm: bool,
}

impl Default for UiInput {
    fn default() -> Self {
        Self {
            cursor: [-1.0, -1.0],
            click: false,
            nav: "none",
            confirm: false,
        }
    }
}

impl Component for UiInput {
    const ID: ComponentId = ComponentId(0x0604);
    const NAME: &'static str = "ui.input";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "UI 输入快照：cursor / click / nav / confirm；cursor = (-1,-1) 表示无指针"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("cursor", FieldType::Vec2, Value::Vec2([-1.0, -1.0])),
            FieldDescriptor::new("click", FieldType::Bool, Value::Bool(false)),
            FieldDescriptor::new("nav", FieldType::String, Value::String("none".into()))
                .doc("none / tab / up / down / left / right"),
            FieldDescriptor::new("confirm", FieldType::Bool, Value::Bool(false)),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "cursor" => Some(Value::Vec2(vec2_f64(self.cursor))),
            "click" => Some(Value::Bool(self.click)),
            "nav" => Some(Value::String(self.nav.to_string())),
            "confirm" => Some(Value::Bool(self.confirm)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "cursor" => self.cursor = vec2(value, field)?,
            "click" => self.click = boolean(value, field)?,
            "nav" => {
                self.nav = match string(value, field)? {
                    "none" => "none",
                    "tab" => "tab",
                    "up" => "up",
                    "down" => "down",
                    "left" => "left",
                    "right" => "right",
                    other => return Err(format!("ui.input.nav 未知方向：{other}")),
                };
            }
            "confirm" => self.confirm = boolean(value, field)?,
            _ => return Err(format!("ui.input 没有字段 {field}")),
        }
        Ok(())
    }
}
