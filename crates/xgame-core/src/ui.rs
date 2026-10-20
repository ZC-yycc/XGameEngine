//! 屏幕空间 UI 组件的**纯数据**层。
//!
//! # 为什么 UI 是实体组件，而不是独立的 UI 树
//!
//! Unity 的 `Canvas` + `RectTransform` 给了我们一个很好的答案：UI 元素就是
//! 场景里的对象。这样层级面板、撤销栈、预制体、复制粘贴、Inspector 全部
//! 免费复用 —— 对一个小引擎来说，**少一套平行的编辑器模型**比少写几百行
//! 布局代码重要得多。
//!
//! 因此：
//!
//! - 一个实体挂上 [`UiElement`] 就是一个 UI 元素；
//! - 元素的**父矩形**来自最近的、同样挂了 [`UiElement`] 的祖先；没有这样的
//!   祖先时父矩形 = 整个视口（屏幕空间根节点）；
//! - 绘制顺序 = 实体层级的**深度优先先序遍历**（同层按实体在场景里的顺序），
//!   也就是"后面的盖住前面的"。同帧内的层级顺序是唯一真相，不引入 z 序字段。
//!
//! `transform` 对 UI 元素**不参与布局**：位置由锚点定义。理由与 Unity 相同 ——
//! 世界变换解决不了"窗口变窄时按钮要贴右边"这类需求，硬套只会让两套坐标
//! 互相打架。
//!
//! # 锚点模型（Unity RectTransform 的最小可用子集）
//!
//! ```text
//! rect.min = parent.min + anchor_min * parent.size + offset_min
//! rect.max = parent.min + anchor_max * parent.size + offset_max
//! ```
//!
//! - 固定尺寸贴左上：`anchor_min = anchor_max = (0, 0)`，偏移 `(x, y) .. (x+w, y+h)`
//! - 居中固定尺寸：锚点 `(0.5, 0.5)`，偏移 `(-w/2, -h/2) .. (w/2, h/2)`
//! - 四边拉伸（全屏背景）：锚点 `(0, 0) .. (1, 1)`，偏移全 0
//! - 底部通栏：锚点 `(0, 1) .. (1, 1)`，偏移 `(0, -h) .. (0, 0)`
//!
//! 故意**没有做** pivot 与旋转缩放：这两个特性只有做动画 / 世界空间 UI 时才
//! 需要，而它们会显著增加命中测试与文本绘制的复杂度。真要做的时候再加，
//! 不要提前为不存在的需求付利息（已知限制写在 `Docs/差距分析.md`）。
//!
//! # 运行态字段为什么也在这个类型里
//!
//! [`UiState`]（悬停 / 按下 / 本帧点击）是运行时每帧写回的数据，用
//! `#[serde(skip)]` 挡住、不落盘。放在这里而不是单独的 `HashMap<EntityId, _>`：
//!
//! 1. 脚本查询（`ui_clicked(ctx, id)`）只需要 `&Scene` —— 不用给 `GameCtx`
//!    加字段，也就不会波及每个构造点；
//! 2. 渲染器读它来画悬停 / 按下态，同一份数据只有一处真相；
//! 3. 实体复制 / 删除时状态跟着走，不会留下悬垂条目。
//!
//! 代价是 `Entity` 多了一个字段 —— 这是刻意的取舍，见决策记录。

use serde::{Deserialize, Serialize};

use crate::color::Color;

// ---------------------------------------------------------------------------
// 组件
// ---------------------------------------------------------------------------

/// 一个 UI 元素（屏幕空间）。
///
/// 所有长度单位都是**物理像素**（视口纹理的像素），不随 DPI 缩放变化 ——
/// 相当于 Unity Canvas 的 "Constant Pixel Size"，也是它最常见的默认档。
/// 分辨率适配靠锚点完成；"设计分辨率缩放"（Canvas Scaler）尚未实现。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiElement {
    /// 控件类型。**只影响 Inspector 的默认值与语义**（按钮可交互、文本默认
    /// 有内容），渲染一律按下面的字段独立判断 —— 一个 `Panel` 只要填了
    /// `text` 就会画字。这样"血条 = 图片 + 填充"之类的组合不用新增类型。
    #[serde(default)]
    pub kind: UiKind,
    /// 锚点左下 / 上界（父矩形的归一化坐标，0..1 之外的值为合法溢出）。
    #[serde(default)]
    pub anchor_min: [f32; 2],
    /// 锚点右上 / 下界。
    #[serde(default)]
    pub anchor_max: [f32; 2],
    /// 相对 `anchor_min` 的像素偏移。
    #[serde(default)]
    pub offset_min: [f32; 2],
    /// 相对 `anchor_max` 的像素偏移（`offset_max = offset_min + size` 即固定尺寸）。
    #[serde(default = "default_offset_max")]
    pub offset_max: [f32; 2],
    /// 背景 / 边框 / 圆角 / 内边距。
    #[serde(default, skip_serializing_if = "UiStyle::is_default")]
    pub style: UiStyle,
    /// 文本样式与内容。
    #[serde(default, skip_serializing_if = "UiTextStyle::is_default")]
    pub text: UiTextStyle,
    /// 图片（有 `path` 才生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<UiImageStyle>,
    /// 自身是否可见。隐藏时子树一并隐藏（与 Unity 的 `SetActive` 同款语义）。
    #[serde(default = "default_true")]
    pub visible: bool,
    /// 是否接受指针事件。`false` = 纯装饰（点击会穿透到下层）。
    #[serde(default = "default_true")]
    pub interactable: bool,
    /// 运行时状态：不落盘，由 [`crate::ui`] 的交互步骤每帧写回。
    #[serde(skip)]
    pub state: UiState,
}

/// 控件类型。
///
/// 只有 `Button` 与 `Panel` 在交互上有区别（Button 默认 `interactable`），
/// 绘制上没有分支 —— 见 [`UiElement::kind`] 的说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UiKind {
    /// 纯色 / 圆角矩形块。
    #[default]
    Panel,
    /// 文本。
    Text,
    /// 图片。
    Image,
    /// 按钮（背景 + 居中文案）。
    Button,
}

impl UiKind {
    /// 给 Inspector 下拉用的英文短名（与场景文件里的 RON 名称一致）。
    pub const ALL: [UiKind; 4] = [UiKind::Panel, UiKind::Text, UiKind::Image, UiKind::Button];

    /// RON / Inspector 里显示的标签。
    pub fn label(self) -> &'static str {
        match self {
            UiKind::Panel => "Panel",
            UiKind::Text => "Text",
            UiKind::Image => "Image",
            UiKind::Button => "Button",
        }
    }
}

/// 背景与边框样式。
///
/// 颜色是**线性空间**（与 `Color` 的全局约定一致，见 `color.rs`）；
/// 渲染器在 sRGB 目标上由硬件编码，非 sRGB 目标手动编码。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiStyle {
    /// 背景色（线性 RGBA，`[r, g, b, a]`）。`a = 0` = 不画背景。
    ///
    /// 用 `[f32; 4]` 而不是 [`Color`]：RON 里 `(0.1, 0.2, 0.3, 1.0)` 比
    /// `(r: 0.1, g: 0.2, b: 0.3, a: 1.0)` 短一大截，与 `MaterialOverride`
    /// 的既有写法一致。**分量仍然是线性的**（与材质 / 灯光同一约定），
    /// 不是 sRGB —— 手写场景文件时要按线性值填，编辑器 Inspector 会转换。
    pub background: [f32; 4],
    /// 边框色（线性 RGBA）。`border_width = 0` 时无效。
    pub border_color: [f32; 4],
    /// 边框宽度（像素，画在矩形内侧）。
    pub border_width: f32,
    /// 圆角半径（像素）。SDF 抗锯齿，0 = 直角。
    pub corner_radius: f32,
    /// 内边距（像素）：文本与图片在内容矩形内再缩进这么多。
    pub padding: f32,
}

impl Default for UiStyle {
    /// 默认样式 = **无背景、无边框、无内边距**。
    ///
    /// 这样"只写 `kind: Text` 的元素"在场景文件里不需要 `style` 字段，
    /// 手写 RON 与 diff 都干净。面板 / 按钮的底色由各自构造器显式给出。
    fn default() -> Self {
        Self {
            background: [0.0; 4],
            border_color: Color::from_rgba8(70, 74, 84, 255).to_array(),
            border_width: 0.0,
            corner_radius: 4.0,
            padding: 0.0,
        }
    }
}

impl UiStyle {
    /// 无背景（= 默认样式，保留这个名字给"语义明确"的调用点）。
    pub fn transparent() -> Self {
        Self::default()
    }

    /// 面板底色（深灰半透明 + 6px 内边距）。
    pub fn panel() -> Self {
        Self {
            background: Color::from_rgba8(28, 30, 36, 216).to_array(),
            padding: 6.0,
            ..Self::default()
        }
    }

    /// serde 用：默认样式不写进场景文件（"少写一行是一行"）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 夹取到合法范围。坏数据（负半径 / NaN）不允许流进渲染器。
    pub fn sanitized(&self) -> Self {
        Self {
            background: self.background, 
            border_color: sanitize_color(self.border_color),
            border_width: finite_or(self.border_width, 0.0).clamp(0.0, 64.0),
            corner_radius: finite_or(self.corner_radius, 0.0).clamp(0.0, 512.0),
            padding: finite_or(self.padding, 0.0).clamp(0.0, 256.0),
        }
    }
}

/// 文本样式与内容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiTextStyle {
    /// 文本内容（支持 `\n` 硬换行）。
    pub content: String,
    /// 字号（像素）。
    pub size: f32,
    /// 颜色（线性 RGBA）。
    pub color: [f32; 4],
    /// 水平对齐（相对内容矩形）。
    pub align: UiAlign,
    /// 垂直对齐。
    pub valign: UiVAlign,
    /// 超出宽度时自动换行。`false` = 单行溢出（不裁剪，见已知限制）。
    pub wrap: bool,
    /// 使用粗体字重（系统粗体缺失时静默退回常规字重）。
    pub bold: bool,
}

impl Default for UiTextStyle {
    fn default() -> Self {
        Self {
            content: String::new(),
            size: 20.0,
            color: Color::WHITE.to_array(),
            align: UiAlign::Left,
            valign: UiVAlign::Middle,
            wrap: true,
            bold: false,
        }
    }
}

impl UiTextStyle {
    /// serde 用：默认文本样式不写进场景文件。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 夹取字号等数值。
    pub fn sanitized(&self) -> Self {
        Self {
            content: self.content.clone(),
            size: finite_or(self.size, Self::default().size).clamp(1.0, 512.0),
            color: sanitize_color(self.color),
            ..self.clone()
        }
    }
}

/// 水平对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UiAlign {
    /// 左对齐。
    #[default]
    Left,
    /// 居中。
    Center,
    /// 右对齐。
    Right,
}

impl UiAlign {
    /// Inspector 下拉标签。
    pub fn label(self) -> &'static str {
        match self {
            UiAlign::Left => "Left",
            UiAlign::Center => "Center",
            UiAlign::Right => "Right",
        }
    }
}

/// 垂直对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UiVAlign {
    /// 顶部。
    Top,
    /// 居中（默认：一行文案在按钮里看起来最自然）。
    #[default]
    Middle,
    /// 底部。
    Bottom,
}

impl UiVAlign {
    /// Inspector 下拉标签。
    pub fn label(self) -> &'static str {
        match self {
            UiVAlign::Top => "Top",
            UiVAlign::Middle => "Middle",
            UiVAlign::Bottom => "Bottom",
        }
    }
}

/// 图片元素。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiImageStyle {
    /// 图片路径（**项目相对路径**，与 `Entity::model` 同款约定）。
    pub path: String,
    /// 着色乘子（白色 = 原图，线性 RGBA）。
    pub tint: [f32; 4],
    /// 填充比例 `0..=1`：从左往右裁剪，做血条 / 进度条用。
    pub fill: f32,
    /// 保持宽高比（按内容矩形做最大内接，居中）。
    pub keep_aspect: bool,
}

impl Default for UiImageStyle {
    fn default() -> Self {
        Self {
            path: String::new(),
            tint: Color::WHITE.to_array(),
            fill: 1.0,
            keep_aspect: true,
        }
    }
}

impl UiImageStyle {
    /// 夹取填充比例。
    pub fn sanitized(&self) -> Self {
        Self {
            path: self.path.clone(),
            tint: sanitize_color(self.tint),
            fill: finite_or(self.fill, 1.0).clamp(0.0, 1.0),
            keep_aspect: self.keep_aspect,
        }
    }
}

/// 每帧由交互步骤写回的运行时状态。
///
/// 语义与 Unity 的 `IPointerEnterHandler` / `IPointerDownHandler` / `Button.onClick`
/// 一一对应，只是这里退化成"轮询标志位"而不是回调列表 —— 脚本层用
/// `ui_clicked(ctx, id)` 读它，组件层用 [`UiElement::state`] 读它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UiState {
    /// 指针是否停在元素上（含被上层元素遮挡时 = false）。
    pub hovered: bool,
    /// 指针在本元素上按下且尚未松开（拖出后仍保持，直到松手）。
    pub pressed: bool,
    /// **本帧**完成了一次"在元素上按下并在元素上松开"。
    pub clicked: bool,
}

// ---------------------------------------------------------------------------
// 默认值
// ---------------------------------------------------------------------------

fn default_offset_max() -> [f32; 2] {
    [200.0, 100.0]
}

fn default_true() -> bool {
    true
}

/// `f32::is_finite` 的兜底：坏值（NaN / 无穷）替换成给定默认值。
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

// ---------------------------------------------------------------------------
// 构造器
// ---------------------------------------------------------------------------

impl Default for UiElement {
    fn default() -> Self {
        Self::panel(200.0, 100.0)
    }
}

impl UiElement {
    /// 一个贴在父矩形左上角的固定尺寸矩形。
    pub fn panel(width: f32, height: f32) -> Self {
        Self {
            kind: UiKind::Panel,
            anchor_min: [0.0, 0.0],
            anchor_max: [0.0, 0.0],
            offset_min: [0.0, 0.0],
            offset_max: [width, height],
            style: UiStyle::panel(),
            text: UiTextStyle::default(),
            image: None,
            visible: true,
            interactable: false,
            state: UiState::default(),
        }
    }

    /// 一段文本（无背景、默认中部左对齐）。
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            kind: UiKind::Text,
            offset_max: [320.0, 32.0],
            style: UiStyle::transparent(),
            text: UiTextStyle {
                content: content.into(),
                ..UiTextStyle::default()
            },
            ..Self::panel(320.0, 32.0)
        }
    }

    /// 一张图片。空路径在渲染时被跳过（Inspector 里填路径即可）。
    pub fn image(path: impl Into<String>) -> Self {
        Self {
            kind: UiKind::Image,
            offset_max: [256.0, 256.0],
            style: UiStyle::transparent(),
            image: Some(UiImageStyle {
                path: path.into(),
                ..UiImageStyle::default()
            }),
            ..Self::panel(256.0, 256.0)
        }
    }

    /// 一个按钮：默认填满父矩形的一角，背景更亮、文案居中。
    pub fn button(label: impl Into<String>) -> Self {
        Self {
            kind: UiKind::Button,
            offset_max: [160.0, 40.0],
            style: UiStyle {
                background: Color::from_rgba8(38, 92, 168, 255).to_array(),
                border_color: Color::from_rgba8(96, 148, 214, 255).to_array(),
                border_width: 1.0,
                corner_radius: 6.0,
                padding: 6.0,
            },
            text: UiTextStyle {
                content: label.into(),
                align: UiAlign::Center,
                valign: UiVAlign::Middle,
                ..UiTextStyle::default()
            },
            interactable: true,
            ..Self::panel(160.0, 40.0)
        }
    }

    // -- 锚点 / 尺寸的链式设置 --

    /// 设置锚点区间（父矩形归一化坐标）。
    pub fn anchored(mut self, anchor_min: [f32; 2], anchor_max: [f32; 2]) -> Self {
        self.anchor_min = anchor_min;
        self.anchor_max = anchor_max;
        self
    }

    /// 设置偏移区间（像素）。
    pub fn with_offset(mut self, offset_min: [f32; 2], offset_max: [f32; 2]) -> Self {
        self.offset_min = offset_min;
        self.offset_max = offset_max;
        self
    }

    /// 固定尺寸 + 相对锚点的左上角位置（锚点较 `anchor_min`）。
    ///
    /// 这是最常见的一类摆放，单独给一个方法，免得每次手算
    /// `offset_max = offset_min + size`。
    pub fn at(mut self, anchor: [f32; 2], x: f32, y: f32, width: f32, height: f32) -> Self {
        self.anchor_min = anchor;
        self.anchor_max = anchor;
        self.offset_min = [x, y];
        self.offset_max = [x + width, y + height];
        self
    }

    /// 四边拉伸到父矩形（可用 `inset` 收边）。
    pub fn stretch(mut self, inset: f32) -> Self {
        self.anchor_min = [0.0, 0.0];
        self.anchor_max = [1.0, 1.0];
        self.offset_min = [inset, inset];
        self.offset_max = [-inset, -inset];
        self
    }

    /// 设定文本内容（Builder 版）。
    pub fn with_text(mut self, content: impl Into<String>) -> Self {
        self.text.content = content.into();
        self
    }

    /// 设定背景色（Builder 版，线性 RGBA）。
    pub fn with_background(mut self, background: [f32; 4]) -> Self {
        self.style.background = background;
        self
    }

    // -- 查询与清洗 --

    /// 布局矩形尺寸（固定尺寸意图下等于 `offset_max - offset_min`）。
    pub fn size(&self) -> [f32; 2] {
        [
            self.offset_max[0] - self.offset_min[0],
            self.offset_max[1] - self.offset_min[1],
        ]
    }

    /// 是否应该参与命中测试。
    pub fn is_hittable(&self) -> bool {
        self.visible && self.interactable
    }

    /// 落盘内容是否相同 —— **忽略** [`UiState`]。
    ///
    /// 脏检查（`Entity::eq`）用它。直接派生 `PartialEq` 会把"鼠标停在按钮上"
    /// 也算成改动：编辑器标题立刻出现星号、关窗弹未保存提示，而用户什么都没改。
    /// 这正是把运行时字段塞进组件里最容易踩的坑，单独给出这个方法而不是
    /// 让调用方各自记得排除，就是为了让它只错一次。
    pub fn data_eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.anchor_min == other.anchor_min
            && self.anchor_max == other.anchor_max
            && self.offset_min == other.offset_min
            && self.offset_max == other.offset_max
            && self.style == other.style
            && self.text == other.text
            && self.image == other.image
            && self.visible == other.visible
            && self.interactable == other.interactable
    }

    /// 夹取所有数值，保证坏数据不会让布局 / 渲染出现 NaN。
    pub fn sanitized(&self) -> Self {
        let mut out = self.clone();
        out.anchor_min = [finite_or(out.anchor_min[0], 0.0), finite_or(out.anchor_min[1], 0.0)];
        out.anchor_max = [finite_or(out.anchor_max[0], 0.0), finite_or(out.anchor_max[1], 0.0)];
        out.offset_min = [finite_or(out.offset_min[0], 0.0), finite_or(out.offset_min[1], 0.0)];
        out.offset_max = [
            finite_or(out.offset_max[0], 0.0),
            finite_or(out.offset_max[1], 0.0),
        ];
        out.style = out.style.sanitized();
        out.text = out.text.sanitized();
        out.image = out.image.as_ref().map(|i| i.sanitized());
        out
    }
}

/// 颜色夹到 `0..=1`（清掉 NaN）。
///
/// UI 颜色来自手写 RON / 脚本，越界值在渲染层会变成"发白 / 负数"，
/// 在入口夹掉比在着色器里兜底便宜。
fn sanitize_color(color: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (dst, src) in out.iter_mut().zip(color) {
        *dst = if src.is_finite() {
            src.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_style_ron_still_loads() {
        // 手写场景里只写最少字段：kind + 偏移，其余全靠 serde default。
        let ron = r#"(
            kind: Text,
            offset_max: (240.0, 40.0),
        )"#;
        let element: UiElement = ron::from_str(ron).expect("应能解析最小 UI 元素");
        assert_eq!(element.kind, UiKind::Text);
        assert_eq!(element.offset_max, [240.0, 40.0]);
        assert!(element.visible);
        assert!(element.interactable);
        assert_eq!(element.state, UiState::default());
    }

    #[test]
    fn runtime_state_never_serializes() {
        let mut element = UiElement::button("OK");
        element.state = UiState {
            hovered: true,
            pressed: true,
            clicked: true,
        };
        let ron = ron::ser::to_string(&element).expect("序列化");
        assert!(!ron.contains("hovered"), "悬停态不该落盘: {ron}");
        assert!(!ron.contains("clicked"), "点击态不该落盘: {ron}");
    }

    #[test]
    fn default_style_is_omitted_from_scene_file() {
        // 纯文本元素用默认样式（透明背景）—— 场景文件里不该出现 style / text 块。
        let element = UiElement::text("Hi");
        let ron = ron::ser::to_string(&element).expect("序列化");
        assert!(!ron.contains("style"), "默认样式不该出现在场景里: {ron}");
        assert!(ron.contains("Hi"), "文本内容要留下: {ron}");
    }

    #[test]
    fn button_defaults_are_interactable_and_centered() {
        let button = UiElement::button("Start");
        assert!(button.interactable);
        assert_eq!(button.text.align, UiAlign::Center);
        assert_eq!(button.text.content, "Start");
        assert_eq!(button.size(), [160.0, 40.0]);
    }

    #[test]
    fn sanitize_rejects_nan_and_out_of_range() {
        let mut element = UiElement::panel(100.0, 40.0);
        element.offset_min = [f32::NAN, f32::INFINITY];
        element.style.corner_radius = f32::NAN;
        element.style.padding = -8.0;
        element.text.size = 0.0;
        element.image = Some(UiImageStyle {
            fill: 3.0,
            ..UiImageStyle::default()
        });
        let clean = element.sanitized();
        assert_eq!(clean.offset_min, [0.0, 0.0]);
        assert_eq!(clean.style.corner_radius, 0.0);
        assert_eq!(clean.style.padding, 0.0);
        assert!(clean.text.size >= 1.0);
        assert_eq!(clean.image.unwrap().fill, 1.0);
    }

    #[test]
    fn state_changes_do_not_count_as_edits() {
        // 脏检查的语义是"落盘内容变了没有"。悬停 / 点击每帧都在变，
        // 一旦被算成改动，编辑器会永远显示"未保存"。
        let a = UiElement::button("OK");
        let mut b = a.clone();
        b.state = UiState {
            hovered: true,
            pressed: true,
            clicked: true,
        };
        assert_ne!(a, b, "结构化比较当然看得出运行时状态的区别");
        assert!(a.data_eq(&b), "data_eq 必须忽略运行时状态");

        let mut c = a.clone();
        c.text.content = "Cancel".into();
        assert!(!a.data_eq(&c), "文本变了必须算改动");
    }

    #[test]
    fn stretch_and_at_build_expected_offsets() {
        let full = UiElement::panel(1.0, 1.0).stretch(8.0);
        assert_eq!(full.anchor_min, [0.0, 0.0]);
        assert_eq!(full.anchor_max, [1.0, 1.0]);
        assert_eq!(full.offset_min, [8.0, 8.0]);
        assert_eq!(full.offset_max, [-8.0, -8.0]);

        let corner = UiElement::panel(1.0, 1.0).at([1.0, 0.0], -100.0, 12.0, 100.0, 30.0);
        assert_eq!(corner.anchor_min, [1.0, 0.0]);
        assert_eq!(corner.offset_max, [0.0, 42.0]);
        assert_eq!(corner.size(), [100.0, 30.0]);
    }
}
