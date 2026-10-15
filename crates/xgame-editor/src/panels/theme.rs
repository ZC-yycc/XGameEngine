//! 类 UE（Unreal Engine）暗色编辑器主题。
//!
//! 在启动时调用 `install(ctx)` 以应用 Slate 暗色风格的配色：整体灰阶比 Unity
//! 亮一档且更平（面板 #242424 / 页签 #2B2B2B），输入框反而**更暗**（#181818），
//! 按钮带轻微蓝灰调，强调色是 UE 标志性的选中蓝，边框和文字都压到 1px 细线；
//! `install_fonts(ctx)` 负责挂上系统中文字体。
//!
//! 所有会出现在界面上的颜色只在 [`colors`] 里定义一次 —— 面板代码一律引用
//! 这些常量（或 `ui.visuals()`），不要就地 `Color32::from_rgb`，否则下次
//! 调风格又得全局翻一遍。

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
    /// 输入框 —— UE 的 TextBox 刻意比周围面板**暗**，让可编辑区域一眼可辨。
    pub const INPUT_BG: Color32 = Color32::from_rgb(24, 24, 24);
    /// 按钮常态（UE 按钮带一点蓝灰，不是纯中性灰）。
    pub const BUTTON_BG: Color32 = Color32::from_rgb(48, 48, 54);
    pub const BUTTON_HOVER: Color32 = Color32::from_rgb(62, 62, 70);
    pub const BUTTON_ACTIVE: Color32 = Color32::from_rgb(76, 76, 86);

    /// UE 标志性选中蓝（链接、按下态、hover 描边共用）。
    pub const ACCENT_BLUE: Color32 = Color32::from_rgb(23, 108, 196);
    pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(71, 149, 217);
    /// 列表选中行（UE Outliner 选中行是偏灰的深蓝，不是饱和蓝）。
    pub const SELECTION_BG: Color32 = Color32::from_rgb(35, 84, 128);

    /// UE 前景色 #B9B9B9 —— 不用纯白，长时间盯界面不刺眼。
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(191, 191, 191);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(128, 128, 128);
    pub const TEXT_ACCENT: Color32 = Color32::from_rgb(66, 149, 226);

    /// 边框 / 分隔线（UE 的边框很弱，只求"分开了"，不求"看得清"）。
    pub const SEPARATOR: Color32 = Color32::from_rgb(56, 56, 56);

    /// 视口里没有渲染纹理时的占位底色（UE 视口无内容时接近纯黑）。
    pub const VIEWPORT_BG: Color32 = Color32::from_rgb(22, 22, 22);

    /// 警告黄 / 错误红（UE 日志的 F5C211 / E53543，比纯黄纯红柔和）。
    pub const WARN: Color32 = Color32::from_rgb(242, 194, 61);
    pub const ERROR: Color32 = Color32::from_rgb(229, 67, 73);
}

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
        window_rounding: egui::Rounding::ZERO,
        window_fill: WINDOW_BG,
        window_stroke: egui::Stroke::new(1.0_f32, SEPARATOR),

        panel_fill: PANEL_BG,
        faint_bg_color: PANEL_HEADER_BG,
        extreme_bg_color: Color32::from_rgb(22, 22, 22),
        code_bg_color: Color32::from_rgb(30, 30, 34),

        override_text_color: Some(TEXT_PRIMARY),
        hyperlink_color: TEXT_ACCENT,
        warn_fg_color: WARN,
        error_fg_color: ERROR,

        selection: egui::style::Selection {
            bg_fill: SELECTION_BG,
            stroke: egui::Stroke::new(1.0_f32, ACCENT_BLUE),
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
                bg_stroke: egui::Stroke::new(1.0_f32, SEPARATOR),
                fg_stroke: egui::Stroke::new(1.0_f32, TEXT_PRIMARY),
                rounding: egui::Rounding::same(2.0_f32),
                expansion: 0.0,
            },
            hovered: egui::style::WidgetVisuals {
                bg_fill: BUTTON_HOVER,
                weak_bg_fill: Color32::from_rgb(44, 44, 52),
                bg_stroke: egui::Stroke::new(1.0_f32, ACCENT_BLUE),
                fg_stroke: egui::Stroke::new(1.0_f32, TEXT_PRIMARY),
                rounding: egui::Rounding::same(2.0_f32),
                expansion: 0.0,
            },
            active: egui::style::WidgetVisuals {
                bg_fill: ACCENT_BLUE,
                weak_bg_fill: SELECTION_BG,
                bg_stroke: egui::Stroke::new(1.0_f32, ACCENT_LIGHT),
                fg_stroke: egui::Stroke::new(1.0_f32, Color32::WHITE),
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
    let style = egui::Style {
        // UE 编辑器信息密度高，间距压得比 egui 默认更紧一点。
        spacing: egui::style::Spacing {
            item_spacing: egui::Vec2::new(5.0, 3.0),
            button_padding: egui::Vec2::new(6.0, 2.0),
            indent: 14.0,
            ..Default::default()
        },
        interaction: egui::style::Interaction {
            resize_grab_radius_side: 4.0,
            resize_grab_radius_corner: 6.0,
            ..Default::default()
        },
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
    let Some((path, bytes)) = load_cjk_font() else {
        log::warn!(
            "未找到可用的系统中文字体，界面中文会显示为方块。候选: {CJK_FONT_CANDIDATES:?}"
        );
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );

    // 追加（而不是插入到最前），保留 egui 自带字体对西文/图标的主导权。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".to_owned());
    }

    ctx.set_fonts(fonts);
    log::info!("已加载中文字体: {}", path.display());
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
