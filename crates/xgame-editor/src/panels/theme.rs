//! 类 UE（Unreal Engine）暗色编辑器主题。
//!
//! 在启动时调用 `install(ctx)` 以应用 Slate 暗色风格的配色：整体灰阶比 Unity
//! 亮一档且更平（面板 #242424 / 页签 #2B2B2B），输入框反而**更暗**（#181818），
//! 按钮带轻微蓝灰调，强调色是 UE 标志性的选中蓝，边框和文字都压到 1px 细线；
//! `install_fonts(ctx)` 负责挂上系统中文字体（含一套粗体族，给分类标题用）。
//!
//! 所有会出现在界面上的颜色只在 [`colors`] 里定义一次 —— 面板代码一律引用
//! 这些常量（或 `ui.visuals()`），不要就地 `Color32::from_rgb`，否则下次
//! 调风格又得全局翻一遍。
//!
//! # 尺寸纪律（UE 信息密度的来源）
//!
//! 界面上所有"高度"都从本模块的常量取，别在面板里写魔法数：
//! 工具栏 34、页签条 26、分类标题 22、列表行 22、输入控件 20、状态栏 24、
//! 面板内边距 6。UE 看起来紧凑但不拥挤，靠的是**行高统一 + 标签列对齐 +
//! 控件高度一致**，而不是把字号压小。

use std::path::PathBuf;

use egui::Color32;

/// UE 暗色主题颜色常量（数值参考 UE4/UE5 编辑器 Slate 暗色主题）。
pub mod colors {
    use egui::Color32;

    /// 浮动窗口 / 最外层底色（UE 编辑器背景最深的一档）。
    pub const WINDOW_BG: Color32 = Color32::from_rgb(30, 30, 30);
    /// 停靠面板主体（Details / Outliner 面板的 #242424）。
    pub const PANEL_BG: Color32 = Color32::from_rgb(36, 36, 36);
    /// 页签条 / 分组标题栏（比面板亮一档，UE 的 Tab 背景 #2B2B2B）。
    pub const PANEL_HEADER_BG: Color32 = Color32::from_rgb(43, 43, 43);
    /// 页签条底色：比面板**暗**一档，让"页签"从背景里浮出来。
    pub const TAB_BAR_BG: Color32 = Color32::from_rgb(27, 27, 27);
    /// 未选中页签的悬停底色。
    pub const TAB_HOVER_BG: Color32 = Color32::from_rgb(37, 37, 37);
    /// 输入框 —— UE 的 TextBox 刻意比周围面板**暗**，让可编辑区域一眼可辨。
    pub const INPUT_BG: Color32 = Color32::from_rgb(24, 24, 24);
    /// 输入框边框：常态几乎看不见，聚焦时才亮蓝。
    pub const INPUT_BORDER: Color32 = Color32::from_rgb(52, 52, 52);
    /// 按钮常态（UE 按钮带一点蓝灰，不是纯中性灰）。
    pub const BUTTON_BG: Color32 = Color32::from_rgb(48, 48, 54);
    pub const BUTTON_HOVER: Color32 = Color32::from_rgb(62, 62, 70);
    pub const BUTTON_ACTIVE: Color32 = Color32::from_rgb(76, 76, 86);

    /// UE 标志性选中蓝（链接、按下态、hover 描边共用）。
    pub const ACCENT_BLUE: Color32 = Color32::from_rgb(23, 108, 196);
    pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(71, 149, 217);
    /// 列表选中行（UE Outliner 选中行是偏灰的深蓝，不是饱和蓝）。
    pub const SELECTION_BG: Color32 = Color32::from_rgb(35, 84, 128);
    /// 列表中悬停的行（比面板亮 6 阶，克制）。
    pub const ROW_HOVER_BG: Color32 = Color32::from_rgb(46, 46, 46);

    /// UE 前景色 #B9B9B9 —— 不用纯白，长时间盯界面不刺眼。
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(191, 191, 191);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(128, 128, 128);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(100, 100, 100);
    pub const TEXT_ACCENT: Color32 = Color32::from_rgb(66, 149, 226);
    /// 选中行 / 分类标题上的高亮文字。
    pub const TEXT_BRIGHT: Color32 = Color32::from_rgb(228, 228, 228);

    /// 边框 / 分隔线（UE 的边框很弱，只求"分开了"，不求"看得清"）。
    pub const SEPARATOR: Color32 = Color32::from_rgb(56, 56, 56);
    /// 更弱的一档（列表内部、分类标题下沿）。
    pub const SEPARATOR_SOFT: Color32 = Color32::from_rgb(44, 44, 44);

    /// 视口里没有渲染纹理时的占位底色（UE 视口无内容时接近纯黑）。
    pub const VIEWPORT_BG: Color32 = Color32::from_rgb(22, 22, 22);

    /// 变换轴的 X/Y/Z 三色（UE 的轴色：红 / 绿 / 蓝，压暗一档融入暗色界面）。
    pub const AXIS_X: Color32 = Color32::from_rgb(214, 96, 96);
    pub const AXIS_Y: Color32 = Color32::from_rgb(122, 186, 112);
    pub const AXIS_Z: Color32 = Color32::from_rgb(96, 146, 214);

    /// 徽标色：脚本（青）/ 物理（橙）/ 预制体（紫）。
    pub const BADGE_SCRIPT: Color32 = Color32::from_rgb(92, 184, 178);
    pub const BADGE_PHYSICS: Color32 = Color32::from_rgb(214, 158, 88);
    pub const BADGE_PREFAB: Color32 = Color32::from_rgb(150, 128, 200);

    /// 播放态绿（UE 的 Play 三角）。
    pub const PLAY_GREEN: Color32 = Color32::from_rgb(106, 191, 106);

    /// 警告黄 / 错误红（UE 日志的 F5C211 / E53543，比纯黄纯红柔和）。
    pub const WARN: Color32 = Color32::from_rgb(242, 194, 61);
    pub const ERROR: Color32 = Color32::from_rgb(229, 67, 73);
}

// ---------------------------------------------------------------------------
// 尺寸常量
// ---------------------------------------------------------------------------

/// 工具栏高度。
pub const TOOLBAR_HEIGHT: f32 = 34.0;
/// 页签条高度（egui_tiles 的 tab_bar_height）。
pub const TAB_BAR_HEIGHT: f32 = 26.0;
/// Inspector 分类标题条高度。
pub const CATEGORY_HEIGHT: f32 = 22.0;
/// 分类条与上一组内容之间的留白（分组靠留白切开，不靠更粗的线）。
pub const SECTION_GAP: f32 = 6.0;
/// 列表行高度（Hierarchy / Prefabs / Console 统一）。
pub const ROW_HEIGHT: f32 = 22.0;
/// 输入控件（DragValue / TextEdit / ComboBox）的标准高度。
pub const FIELD_HEIGHT: f32 = 20.0;
/// 底部状态栏高度。
pub const STATUS_BAR_HEIGHT: f32 = 24.0;
/// 面板内边距。
pub const PANEL_MARGIN: f32 = 6.0;
/// Inspector 标签列宽度。
pub const LABEL_WIDTH: f32 = 58.0;

/// 粗体字体族的名字（`FontFamily::Name("bold")`）。
pub const BOLD_FAMILY: &str = "bold";

/// 粗体字体族。
pub fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name(BOLD_FAMILY.into())
}

/// 粗体文本（分类标题 / 页签 / 工具栏标题用）。
pub fn bold(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).family(bold_family())
}

/// 数值输入框的标准宽度（Inspector 里三个轴向并排还能放下）。
pub const FIELD_WIDTH: f32 = 54.0;

/// 轴标（X/Y/Z 彩色字母）的占位宽度。向量行的控件从"轴标 + 间距"之后开始，
/// 标量行也要留出同样一截，两类行的输入框才会落在同一条竖线上。
pub const AXIS_TAG_WIDTH: f32 = 9.0;

/// Inspect 面板的**分类条**（不可折叠版）：22px 高的横条 + 左侧 2px 强调条，
/// 下沿一条 1px 分隔线。
///
/// 与 [`category`] 的区别：这个只是标题，展开状态由调用方决定（有些段落
/// 比如 Parent / Prefab 本身只在特定条件下出现，折叠反而多一层状态）。
pub fn section_bar(ui: &mut egui::Ui, title: &str) {
    use colors::*;

    // 分组之间留一口气：条本身紧贴着上一行（比如 "World 1.57, …" 的下一像素
    // 就是 "Mesh" 条）会让人以为那是同一组的最后一行。UE 的 Details 也是
    // 靠这点留白把分类切开的，不靠更粗的线。
    ui.add_space(SECTION_GAP);

    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, CATEGORY_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, PANEL_HEADER_BG);
    painter.rect_filled(
        egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, CATEGORY_HEIGHT)),
        0.0,
        ACCENT_BLUE.gamma_multiply(0.8),
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0_f32, SEPARATOR_SOFT),
    );
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(12.5, bold_family()),
        TEXT_PRIMARY,
    );
}

/// 枢纽卡片的**内边距**（[`card_frame`] 用）。卡片里的内容要按
/// `available_width() - 2 * CARD_INNER_PAD` 定宽，否则会顶出卡片边框。
pub const CARD_INNER_PAD: f32 = 10.0;

/// 枢纽里的一张"卡片"（标题 + 一组表单）：面板底色 + 1px 边 + 10px 内边距。
///
/// 枢纽不用 `ui.group()`：它在暗色主题下画的是 `faint_bg_color`（比面板亮），
/// 四张卡叠起来像四块贴纸；UE 的项目浏览器用的是"和背景同色 + 一圈细边"。
pub fn card_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(colors::PANEL_BG)
        .inner_margin(egui::Margin::same(CARD_INNER_PAD))
        .rounding(egui::Rounding::same(3.0))
        .stroke(egui::Stroke::new(1.0_f32, colors::SEPARATOR))
}

/// 卡片标题：13px 粗体 + 下沿一条弱分隔线。
pub fn card_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .family(bold_family())
            .size(13.0)
            .color(colors::TEXT_BRIGHT),
    );
    ui.add_space(2.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0_f32, colors::SEPARATOR_SOFT),
    );
    ui.add_space(4.0);
}

/// 一行「标签 + 控件」：标签固定占 [`LABEL_WIDTH`]，控件从同一列开始。
///
/// Inspector 里所有行都必须走它 —— 手写 `ui.horizontal` 会让每行的标签宽度
/// 跟着文字长度变化，整列对不齐，看起来就是"排版歪了"。
pub fn field_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_WIDTH, FIELD_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                // **必须显式撑满标签列**：`allocate_ui_with_layout` 只按子 ui 的
                // `min_rect` 占位（≈ 标签文字本身的宽度），不撑开的话 "Type" 与
                // "Radius" 两行的值列会差出十几个像素 —— 一列控件左边缘参差不齐，
                // 这是"看着乱"最直接的来源。`set_min_width` 把占位拉回 LABEL_WIDTH。
                ui.set_min_width(LABEL_WIDTH);
                // 截断：状态机参数名 / 行为字段名是用户起的，长名字不该把
                // 控件列顶出去（悬停能看全）。
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(label).color(colors::TEXT_SECONDARY),
                    )
                    .truncate(),
                );
            },
        );
        add(ui)
    })
    .inner
}

/// 固定尺寸的数值输入框（高度与 [`FIELD_HEIGHT`] 一致）。
pub fn drag_field(
    ui: &mut egui::Ui,
    value: &mut f32,
    speed: f64,
    suffix: &str,
) -> egui::Response {
    ui.add_sized(
        egui::vec2(FIELD_WIDTH, FIELD_HEIGHT),
        egui::DragValue::new(value).speed(speed).suffix(suffix),
    )
}

/// 轴的字母颜色（X 红 / Y 绿 / Z 蓝，与 UE 一致）。
pub fn axis_color(axis: &str) -> Color32 {
    match axis {
        "X" => colors::AXIS_X,
        "Y" => colors::AXIS_Y,
        _ => colors::AXIS_Z,
    }
}

/// 轴字母标签（固定宽度，三个轴并排时起点一致）。
pub fn axis_label(ui: &mut egui::Ui, axis: &str) {
    ui.add_sized(
        egui::vec2(AXIS_TAG_WIDTH, FIELD_HEIGHT),
        egui::Label::new(
            egui::RichText::new(axis)
                .color(axis_color(axis))
                .size(11.0),
        ),
    );
}

/// 小一号的灰色说明文字（Inspector 里"这行是干什么的"）。
///
/// 面板里所有解释性文字统一走它，别就地写 `.weak().size(11.0)` ——
/// 一处改字号就得全局翻一遍，而且 `weak()` 在暗色主题下的对比度并不一致。
pub fn dim_note(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .size(11.0)
        .color(colors::TEXT_DIM)
}

/// 分类内容里的一行说明文字（弱提示，**会自动换行**）。
///
/// 为什么不能直接 `ui.label(dim_note(长中文))`：标签在横向流里默认按 **Extend**
/// 排版（不换行），文案一旦比面板宽，后半句就从右边缘被裁掉。中文没有空格，
/// 被裁的地方也不会留下任何"还有下文"的暗示，看着就像写漏了半句。
pub fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    // 自己排一次版（而不是交给 `Label::wrap()`）：`Label` 用的换行宽度是
    // 本 ui 的 `available_width()`，但它自己又从光标处起笔，两者在缩进过
    // 一次的子 ui 里会差出几像素 —— 中文长句于是正好差最后半个字，尾部
    // 被面板裁掉。先按"可用宽度 − 2px"排出 galley，再交给 Label 原样画。
    let wrap = (ui.available_width() - 2.0).max(48.0);
    let galley = ui.painter().layout(
        text.into(),
        egui::FontId::proportional(11.0),
        colors::TEXT_DIM,
        wrap,
    );
    ui.add(egui::Label::new(galley));
}

// ---------------------------------------------------------------------------
// Visuals
// ---------------------------------------------------------------------------

/// 构建 UE 暗色 egui `Visuals`。
///
/// 与 Unity 暗色主题的两个刻意差异：
/// 1. `weak_bg_fill`（输入框 / 下拉等"可写"区域）比 `bg_fill` **暗**，Unity 是反过来的；
/// 2. `fg_stroke` 全部 1px —— UE 的文字不加粗描边，状态变化靠**底色**表达。
pub fn ue_dark_visuals() -> egui::Visuals {
    use colors::*;

    egui::Visuals {
        dark_mode: true,

        // UE 的面板和窗口都是方角，只有下拉/按钮有 2px 微圆角。
        window_rounding: egui::Rounding::same(3.0),
        window_fill: WINDOW_BG,
        window_stroke: egui::Stroke::new(1.0_f32, SEPARATOR),
        menu_rounding: egui::Rounding::same(2.0),

        panel_fill: PANEL_BG,
        faint_bg_color: PANEL_HEADER_BG,
        extreme_bg_color: INPUT_BG,
        code_bg_color: Color32::from_rgb(30, 30, 34),

        override_text_color: Some(TEXT_PRIMARY),
        hyperlink_color: TEXT_ACCENT,
        warn_fg_color: WARN,
        error_fg_color: ERROR,

        selection: egui::style::Selection {
            bg_fill: SELECTION_BG,
            stroke: egui::Stroke::new(1.0_f32, ACCENT_LIGHT),
        },

        widgets: egui::style::Widgets {
            noninteractive: egui::style::WidgetVisuals {
                bg_fill: PANEL_BG,
                weak_bg_fill: INPUT_BG,
                bg_stroke: egui::Stroke::new(1.0_f32, SEPARATOR),
                fg_stroke: egui::Stroke::new(1.0_f32, TEXT_SECONDARY),
                rounding: egui::Rounding::ZERO,
                expansion: 0.0,
            },
            inactive: egui::style::WidgetVisuals {
                bg_fill: BUTTON_BG,
                weak_bg_fill: INPUT_BG,
                // 输入框边框压到几乎看不见 —— UE 的"可编辑"靠更暗的底色表达，
                // 只有聚焦时才亮蓝（见 `install` 里的 interaction 设置）。
                bg_stroke: egui::Stroke::new(1.0_f32, INPUT_BORDER),
                fg_stroke: egui::Stroke::new(1.0_f32, TEXT_PRIMARY),
                rounding: egui::Rounding::same(2.0_f32),
                expansion: 0.0,
            },
            hovered: egui::style::WidgetVisuals {
                bg_fill: BUTTON_HOVER,
                weak_bg_fill: Color32::from_rgb(32, 32, 38),
                bg_stroke: egui::Stroke::new(1.0_f32, Color32::from_rgb(78, 78, 90)),
                fg_stroke: egui::Stroke::new(1.0_f32, TEXT_BRIGHT),
                rounding: egui::Rounding::same(2.0_f32),
                expansion: 0.0,
            },
            active: egui::style::WidgetVisuals {
                bg_fill: BUTTON_ACTIVE,
                weak_bg_fill: INPUT_BG,
                bg_stroke: egui::Stroke::new(1.0_f32, ACCENT_BLUE),
                fg_stroke: egui::Stroke::new(1.0_f32, TEXT_BRIGHT),
                rounding: egui::Rounding::same(2.0_f32),
                expansion: 0.0,
            },
            // egui 里 `open` 状态对应"展开中的树节点 / 打开的下拉"，
            // UE 用比常态亮一档的蓝灰底表达"这里正在展开"。
            open: egui::style::WidgetVisuals {
                bg_fill: BUTTON_ACTIVE,
                weak_bg_fill: INPUT_BG,
                bg_stroke: egui::Stroke::new(1.0_f32, ACCENT_BLUE),
                fg_stroke: egui::Stroke::new(1.0_f32, TEXT_PRIMARY),
                rounding: egui::Rounding::same(2.0_f32),
                expansion: 0.0,
            },
        },

        ..Default::default()
    }
}

/// 将 UE 暗色主题应用到 egui 上下文。
///
/// **启动时调用一次即可**（调用点在 `main.rs::EditorApp::new`）：历史上这里
/// 曾需要每帧重设，因为 eframe 每帧会按**系统主题**（这台机器是浅色）重设
/// egui 的 style，只在启动时设一次会被它顶掉。现在调用点先用
/// `ThemePreference::Dark` 关掉了跟随系统主题，覆盖不再发生，每帧重建整个
/// `egui::Style` 纯属浪费。真正昂贵的是 [`install_fonts`] 里的 `set_fonts`
/// （重建整张字形图集），那个同样只在启动时装一次。
pub fn install(ctx: &egui::Context) {
    use egui::TextStyle;

    let mut text_styles = std::collections::BTreeMap::new();
    text_styles.insert(TextStyle::Small, egui::FontId::proportional(10.5));
    text_styles.insert(TextStyle::Body, egui::FontId::proportional(12.5));
    text_styles.insert(TextStyle::Monospace, egui::FontId::monospace(11.5));
    text_styles.insert(TextStyle::Button, egui::FontId::proportional(12.5));
    // Heading 只给弹窗标题用；面板里的"分类标题"是另一套（22px 条 + 12.5 粗体），
    // 别把 Heading 塞进 Inspector —— 20px 的标题在 280px 宽的面板里像标语。
    text_styles.insert(TextStyle::Heading, egui::FontId::proportional(15.0));
    text_styles.insert(egui::TextStyle::Name("Bold".into()), egui::FontId::new(12.5, bold_family()));

    let style = egui::Style {
        // UE 编辑器信息密度高，间距压得比 egui 默认更紧一点。
        spacing: egui::style::Spacing {
            item_spacing: egui::Vec2::new(5.0, 3.0),
            button_padding: egui::Vec2::new(7.0, 3.0),
            indent: 10.0,
            interact_size: egui::Vec2::new(40.0, FIELD_HEIGHT),
            slider_width: 100.0,
            combo_width: 120.0,
            text_edit_width: 280.0,
            icon_width: 14.0,
            icon_width_inner: 8.0,
            scroll: egui::style::ScrollStyle {
                bar_width: 9.0,
                ..Default::default()
            },
            ..Default::default()
        },
        interaction: egui::style::Interaction {
            resize_grab_radius_side: 4.0,
            resize_grab_radius_corner: 6.0,
            show_tooltips_only_when_still: true,
            ..Default::default()
        },
        text_styles,
        visuals: ue_dark_visuals(),
        ..Default::default()
    };

    ctx.set_style(style);
}

// ---------------------------------------------------------------------------
// 中文字体
// ---------------------------------------------------------------------------

/// 中文字体候选，按优先级排列。取第一个「存在且文件头有效」的。
///
/// 微软雅黑排第一是因为它是 Windows 中文界面的标准字体；`.ttf` 单体字体
/// （等线 / 黑体）排在 `.ttc` 集合后面只是习惯——两者都受支持。
const CJK_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑
    r"C:\Windows\Fonts\Deng.ttf",   // 等线（较新系统自带）
    r"C:\Windows\Fonts\simhei.ttf", // 黑体
    r"C:\Windows\Fonts\simsun.ttc", // 宋体（兜底）
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
];

/// 挂上系统中文字体，让界面能显示中文。
///
/// egui 内置字体（Ubuntu-Light / Hack）**不含任何 CJK 字形**，所以中文会渲染成
/// 一排「豆腐块」□。修法是把系统中文字体加进 `FontDefinitions`，并且**追加到
/// 字体族末尾**——西文和图标继续走 egui 自带字体（字形更精致、体积也更小），
/// 只有内置字体缺字时才回退到中文字体。
///
/// 必须在第一帧之前调用，且**只能调用一次**：`set_fonts` 会重建整张字形图集，
/// 每帧调用等于每帧重建一次。
pub fn install_fonts(ctx: &egui::Context) {
    let cjk = load_cjk_font();
    if cjk.is_none() {
        log::warn!(
            "未找到可用的系统中文字体，界面中文会显示为方块。候选: {CJK_FONT_CANDIDATES:?}"
        );
    }

    let mut fonts = egui::FontDefinitions::default();
    if let Some((_, bytes)) = cjk {
        fonts.font_data.insert(
            "cjk".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );

        // 追加（而不是插入到最前），保留 egui 自带字体对西文/图标的主导权。
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("cjk".to_owned());
        }
    }

    // ── 粗体族 ──
    //
    // UE 的分类标题、页签和工具栏标题都是**真粗体**；egui 的 `strong()` 只改颜色，
    // 不换字重。这里把系统粗体中文（微软雅黑 Bold）挂成一个命名族，正文回退链
    // 原样保留 —— 粗体字体里缺的字（图标、生僻字）仍然能落到常规字体上。
    //
    // 无论有没有找到中文粗体，这个族都**必须**注册：`FontFamily::Name` 指向未注册
    // 的族时 epaint 会直接 panic（`FontFamily::Name("bold") is not bound to any fonts`）。
    let proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut bold_chain = Vec::with_capacity(proportional.len() + 1);

    match load_bold_cjk_font() {
        Some((bold_path, bold_bytes)) => {
            fonts.font_data.insert(
                "cjk-bold".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bold_bytes)),
            );
            bold_chain.push("cjk-bold".to_owned());
            log::info!("已加载中文粗体: {}", bold_path.display());
        }
        // 没有粗体候选：退化成常规族的别名。
        None => log::warn!("未找到系统中文字体粗体，分类标题退回常规字重"),
    }
    bold_chain.extend(proportional);
    fonts
        .families
        .insert(egui::FontFamily::Name(BOLD_FAMILY.into()), bold_chain);

    ctx.set_fonts(fonts);
}

/// 粗体候选（与常规候选同源，优先同一字体的 Bold 变体）。
const CJK_BOLD_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyhbd.ttc", // 微软雅黑 Bold
    r"C:\Windows\Fonts\Dengb.ttf",  // 等线 Bold
    r"C:\Windows\Fonts\simhei.ttf", // 黑体（本身就偏粗）
    "/System/Library/Fonts/PingFang.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
];

/// 依次探测粗体候选路径，返回第一个可用的字体文件。
fn load_bold_cjk_font() -> Option<(PathBuf, Vec<u8>)> {
    for candidate in CJK_BOLD_FONT_CANDIDATES {
        let path = PathBuf::from(candidate);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if !looks_like_font(&bytes) {
            log::warn!("跳过文件头不像字体的粗体候选: {}", path.display());
            continue;
        }
        return Some((path, bytes));
    }
    None
}

/// 依次探测候选路径，返回第一个可用的字体文件。
fn load_cjk_font() -> Option<(PathBuf, Vec<u8>)> {
    for candidate in CJK_FONT_CANDIDATES {
        let path = PathBuf::from(candidate);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if !looks_like_font(&bytes) {
            log::warn!("跳过文件头不像字体的候选: {}", path.display());
            continue;
        }
        return Some((path, bytes));
    }
    None
}

/// 判断字节流是不是字体文件。
///
/// 这一步不是多余的谨慎：`epaint` 解析字体失败时**直接 panic**
/// （`Error parsing ... TTF/OTF font file`），一旦把坏文件塞进 `FontDefinitions`，
/// 编辑器会带着这个 panic 启动失败，用户侧的表现只是「双击没反应」，极难排查。
/// 所以宁可先看一眼文件头（TrueType `0x00010000` / `true`、OpenType `OTTO`、
/// 字体集合 `ttcf`），把明显不是字体的文件挡在外面。
fn looks_like_font(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    let tag: [u8; 4] = bytes[..4].try_into().unwrap_or([0; 4]);
    matches!(
        tag,
        [0x00, 0x01, 0x00, 0x00] | [b't', b'r', b'u', b'e'] | [b'O', b'T', b'T', b'O'] | [b't', b't', b'c', b'f']
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 界面上真实出现过的中文（枢纽 + 工具栏 + 状态栏）。
    const SAMPLE: &str = "项目枢纽 新建 打开 最近 场景 资源 实体 构建 保存 关闭 位置 名称";

    /// 测 `install_fonts` 之后，中文是否真的有字形 —— 而不是渲染成一排方块。
    ///
    /// 这条断言直接盯住症状「无法显示中文字符」：只要字体候选全部失效
    /// （换系统 / 换机器），这里会先炸出来，而不是等用户看到方框才发现。
    #[test]
    fn installed_fonts_cover_chinese() {
        let Some((path, _)) = load_cjk_font() else {
            eprintln!("跳过：系统里找不到候选中文字体");
            return;
        };
        println!("[font check] 使用 {}", path.display());

        let ctx = egui::Context::default();
        install_fonts(&ctx);

        // 字体只有在 `Context::run` 跑过第一帧之后才存在。
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |_| {});

        let proportional = ctx.fonts(|f| f.has_glyphs(&egui::FontId::proportional(14.0), SAMPLE));
        let monospace = ctx.fonts(|f| f.has_glyphs(&egui::FontId::monospace(14.0), SAMPLE));

        assert!(proportional, "比例字体仍缺中文缺字");
        assert!(monospace, "等宽字体仍缺中文缺字");
    }

    /// 明确守住「字体只装一次」这条：装两次不该出问题，但候选探测要能重复调用。
    #[test]
    fn font_candidate_probe_is_stable() {
        let first = load_cjk_font().map(|(p, _)| p);
        let second = load_cjk_font().map(|(p, _)| p);
        assert_eq!(first, second, "候选探测结果不稳定");
    }
}
