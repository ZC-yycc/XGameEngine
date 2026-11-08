//! 类 UE Output Log 的控制台面板——按严重等级显示日志消息。
//!
//! 排版纪律：每行一条消息、18px 行高、等级图标固定 11px 列宽（用矢量图标而不是
//! ℹ/⚠/✖ 字形 —— 字形宽度不一，等级列会左右错位），消息文字单行截断（悬停看全文），
//! 重复次数右对齐。日志刷屏时这里是每帧几百行的热点，所以一律不做换行测量。
//!
//! # 三来源
//!
//! 1. 编辑器自己 `state.log_*`（保存成功、切场景、撤销…）
//! 2. **引擎 `log::warn!` 等**，由 [`crate::logging`] 收进来（模块加载告警、资产
//!    缺失、脚本报错…）—— 这些以前只进 stderr，双击 exe 跑的人根本看不到
//! 3. 脚本 `log()`，本来就走 1 的同一条路
//!
//! 筛选（等级开关 + 关键词）只影响**显示**，不清消息：`Clear` 才是清。
//! 条数标签在筛选生效时显示 `显示数 / 总数`，否则只显示总数。
//!
//! # 两处"定位"
//!
//! 1. **源码**：脚本报错与引擎日志带 `脚本:行` / `文件:行`，点击打开源码查看器；
//! 2. **资产**：消息正文里出现的项目相对资产路径（`.glb` / `.png` / `.mat` …），
//!    点击在 Assets 面板里选中它 —— "模型加载失败"之后第一件事就是找到那个文件。

use crate::logging;
use crate::panels::{icons, theme};
use crate::state::{
    ConsoleFilter, EditorState, LogLevel, LogMessage, LogSource, CONSOLE_RENDER_LIMIT,
};

/// 等级 → (图标, 颜色)。颜色统一问 [`LogLevel::color`]，这里只决定图形。
fn level_style(level: LogLevel) -> (icons::Icon, egui::Color32) {
    let icon = match level {
        LogLevel::Info => icons::Icon::Dot,
        LogLevel::Warning => icons::Icon::Warning,
        LogLevel::Error => icons::Icon::Close,
        LogLevel::Success => icons::Icon::Check,
    };
    (icon, level.color())
}

/// 等级 → 统计下标（顺序与 `level_counts` 的返回数组一致）。
fn level_index(level: LogLevel) -> usize {
    match level {
        LogLevel::Info => 0,
        LogLevel::Warning => 1,
        LogLevel::Error => 2,
        LogLevel::Success => 3,
    }
}

/// 每个等级各有多少条。
///
/// 按**出现次数**求和（重复消息的 `×N` 一起计入），因为"这一局报了几次错"问的是
/// 次数而不是"屏上有几行"。`Clear` 之后自然归零 —— 统计的是缓冲区里的内容，
/// 与筛选无关（筛掉的那部分照样算，否则关掉 Info 会让总数莫名其妙地变小）。
fn level_counts(messages: &[LogMessage]) -> [u32; 4] {
    let mut counts = [0u32; 4];
    for message in messages {
        counts[level_index(message.level)] = counts[level_index(message.level)]
            .saturating_add(message.count);
    }
    counts
}

/// 资产后缀表：消息里认这些结尾的词当"可能是一个资产路径"。
///
/// 刻意只认**具体的资产类型**：`.rs` / `.txt` / 无后缀的一律不当资产，否则
/// 每一行带文件名的日志都会变成可点链接（实测观感很吵）。
const ASSET_SUFFIXES: [&str; 9] = [
    ".glb", ".gltf", ".png", ".mat", ".lua", ".wav", ".ogg", ".mp3", ".ron",
];

/// 这条消息**可能**提到资产吗（零分配预筛，逐行调用）。
fn mentions_asset(text: &str) -> bool {
    ASSET_SUFFIXES.iter().any(|suffix| text.contains(suffix))
}

/// 从消息里认出**项目相对的资产路径**，`exists` 决定它是不是真的在资产库里。
///
/// 逐 token 扫描（按空白与常见包裹符号切），每个候选先过
/// `normalize_project_relative`（越界 / 盘符 / Windows 非法写法一律拒），
/// 再问资产库认不认。只返回**第一个**命中的 —— 一条消息里有两三个路径时，
/// 逐个弹菜单比"点第一个"更烦。
pub(crate) fn asset_in_text(text: &str, exists: impl Fn(&str) -> bool) -> Option<String> {
    for token in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '\'' | '"' | '`' | '(' | ')' | '[' | ']' | ',' | '<' | '>' | '：' | '，' | '。'
            )
            // 冒号也要切：`Scripts/player.lua:42` 这种带行号的写法必须能认出前一半。
            || c == ':'
    }) {
        let token = token.trim_matches(['.', ';', '、']);
        if token.is_empty() || !ASSET_SUFFIXES.iter().any(|s| token.ends_with(s)) {
            continue;
        }
        let Ok(normalized) = xgame_core::project::normalize_project_relative(token) else {
            continue;
        };
        if exists(&normalized) {
            return Some(normalized);
        }
    }
    None
}

/// 一个等级开关。点亮的用等级本身的颜色，关掉的压成次级灰 —— 一眼看出"哪一类被滤掉了"。
///
/// `count` > 0 时把条数接在标签后面（`Error 3`）：这样"有多少条错误"不用点开
/// 就知道；为 0 时保持原样，窄窗口少几个像素。
fn level_toggle(
    ui: &mut egui::Ui,
    on: &mut bool,
    level: LogLevel,
    label: &str,
    count: u32,
    tooltip: &str,
) {
    let color = if *on {
        level.color()
    } else {
        theme::colors::TEXT_DIM
    };
    let text = if count > 0 {
        format!("{label} {count}")
    } else {
        label.to_owned()
    };
    let text = egui::RichText::new(text).size(11.0).color(color);
    if ui
        .selectable_label(*on, text)
        .on_hover_text(if count > 0 {
            format!("{tooltip}\n本次会话收到 {count} 条")
        } else {
            tooltip.to_owned()
        })
        .clicked()
    {
        *on = !*on;
    }
}

/// 标题栏：Clear / 四个等级开关 / 关键词 / 条数 / 日志文件入口。
///
/// `shown` 由调用方算好传进来（它就是同一帧要渲染的那份列表的长度）——
/// 这里再算一遍等于每帧白扫两遍消息表。
fn header(state: &mut EditorState, shown: usize, counts: [u32; 4], ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if icons::text_button(
            ui,
            "Clear",
            "清空控制台消息（不影响场景，也不影响日志文件）",
        )
        .clicked()
        {
            state.console_messages.clear();
        }
        icons::v_separator(ui);

        let filter: &mut ConsoleFilter = &mut state.console_filter;
        level_toggle(
            ui,
            &mut filter.info,
            LogLevel::Info,
            "Info",
            counts[level_index(LogLevel::Info)],
            "显示普通信息",
        );
        level_toggle(
            ui,
            &mut filter.warning,
            LogLevel::Warning,
            "Warn",
            counts[level_index(LogLevel::Warning)],
            "显示警告",
        );
        level_toggle(
            ui,
            &mut filter.error,
            LogLevel::Error,
            "Error",
            counts[level_index(LogLevel::Error)],
            "显示错误",
        );
        level_toggle(
            ui,
            &mut filter.success,
            LogLevel::Success,
            "OK",
            counts[level_index(LogLevel::Success)],
            "显示成功提示",
        );
        icons::v_separator(ui);

        ui.add(
            egui::TextEdit::singleline(&mut filter.query)
                .desired_width(130.0)
                .hint_text("过滤…")
                .font(egui::TextStyle::Small),
        )
        .on_hover_text("按子串过滤消息（ASCII 不区分大小写，中文按原样匹配）");

        // 借用到此为止：下面要读 `state` 的其它字段。
        let filtering = state.console_filter.is_active();
        let total = state.console_messages.len();
        let dropped = logging::dropped();

        // 右侧固定一列（240pt），右对齐、**不换行**。
        //
        // 不能用默认的 `Layout::right_to_left`：它的 `main_wrap` 是 true，1280 宽
        // 一挤就会把最后一项折到下一行去（截图实测过）。固定宽度 + 关掉换行，
        // 三种常用分辨率下位置逐像素稳定。
        let mut layout = egui::Layout::right_to_left(egui::Align::Center);
        layout.main_wrap = false;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width().max(240.0), theme::FIELD_HEIGHT),
            layout,
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // 右对齐布局里**先加的靠右**：日志文件入口 → 丢弃计数 → "messages"
                // → 条数，读起来才是「16 messages +6 略 … Log 目录」。
                log_folder_button(ui);

                // 队列溢出时说清楚：控制台里"少了"不是丢了系统日志，而是老消息被挤掉。
                if dropped > 0 {
                    ui.label(
                        egui::RichText::new(format!("+{dropped} 略"))
                            .size(10.5)
                            .color(theme::colors::WARN),
                    )
                    .on_hover_text(
                        "日志刷得太快，控制台队列（1024 条）丢掉的最旧条数。\n\
                         完整日志仍在磁盘上的 Editor.log。",
                    );
                }

                ui.label(
                    egui::RichText::new("messages")
                        .size(11.0)
                        .color(theme::colors::TEXT_DIM),
                );

                let (count, hint) = if filtering {
                    (format!("{shown} / {total}"), "显示数 / 总数（筛选生效中）")
                } else {
                    (format!("{total}"), "控制台里的消息总数")
                };
                ui.label(
                    egui::RichText::new(count)
                        .monospace()
                        .size(11.0)
                        .color(theme::colors::TEXT_SECONDARY),
                )
                .on_hover_text(hint);
            },
        );
    });
}

/// "Log 目录" 入口：双击 exe 跑的用户没有终端，唯一能留给他们的现场就是这个文件。
fn log_folder_button(ui: &mut egui::Ui) {
    let log_path = logging::log_file_path();
    let tip = match log_path {
        Some(path) => format!(
            "引擎日志（含 stderr 之外的告警）写到：\n{}\n点击打开所在目录",
            path.display()
        ),
        None => "配置目录不可用：本次运行的日志只有控制台这一份".to_owned(),
    };
    let label = egui::RichText::new("Log 目录")
        .size(11.0)
        .color(theme::colors::TEXT_SECONDARY);
    if ui
        .selectable_label(false, label)
        .on_hover_text(tip)
        .clicked()
    {
        if let Some(dir) = log_path.and_then(|path| path.parent()) {
            crate::panels::toolbar::open_in_file_manager(dir);
        }
    }
}

/// 在 `Ui` 内渲染控制台面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // 只取最近的 N 条（筛选在 `visible_console_indices` 里做，从尾部往前扫）。
    // 先算下标再画标题栏：标题栏要显示"显示数"，而画它需要 `&mut state`。
    let indices = state.visible_console_indices(CONSOLE_RENDER_LIMIT);
    // 统计覆盖**整个缓冲区**（不是这一屏）—— 它是"这一局报了几次"的账本。
    let counts = level_counts(&state.console_messages);
    // 不用 `ui.separator()`：紧贴页签条下方，那条线会和页签的底边叠成双线。
    header(state, indices.len(), counts, ui);

    // 点击后要 `&mut state` 才能开源码窗口 / 在 Assets 面板定位，而这一整段还在
    // 只读地借消息表 —— 所以先记下来、出了滚动区再处理。
    let mut clicked: Option<LogSource> = None;
    let mut clicked_asset: Option<String> = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for index in indices {
                let Some(msg) = state.console_messages.get(index) else {
                    continue;
                };
                let (icon, color) = level_style(msg.level);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    icons::small_icon(ui, icon, 11.0, color);
                    // 单行截断：一条 2KB 的报错不应该把控制台撑成一面墙。
                    // 带位置的报错可点（打开源码查看器），其余保持纯文本。
                    let has_source = msg.source.is_some();
                    // 资产路径只在"没有源码位置"时才去找：脚本报错的正文里也有
                    // 脚本路径，但点它应该是打开源码（更精确），不是去 Assets 面板。
                    let may_hold_asset = !has_source && mentions_asset(&msg.text);
                    // 只有**脚本**报错加下划线：引擎日志几乎每条都有 `file:line`，
                    // 全加下划线会让控制台看起来像一页链接（实测截图确认过），
                    // 而它的主要读者（引擎开发者）看悬停提示就够了。两者都可点。
                    let is_script = msg
                        .source
                        .as_ref()
                        .is_some_and(|s| s.path.ends_with(".lua"));
                    let text = if is_script {
                        egui::RichText::new(&msg.text).size(11.5).color(color).underline()
                    } else {
                        egui::RichText::new(&msg.text).size(11.5).color(color)
                    };
                    let label = egui::Label::new(text).truncate();
                    let label = if has_source || may_hold_asset {
                        label.sense(egui::Sense::click())
                    } else {
                        label
                    };
                    let resp = ui.add(label).on_hover_text(match &msg.source {
                        Some(src) => format!("{}\n\n点击打开源码：{}", msg.text, src.label()),
                        None if may_hold_asset => {
                            format!("{}\n\n点击在 Assets 面板定位（如果这条消息指向一个项目资产）", msg.text)
                        }
                        None => msg.text.clone(),
                    });
                    if resp.clicked() {
                        // 真的命中资产才记（预筛只看后缀，可能只是"提到了 .png"）。
                        if has_source {
                            clicked = msg.source.clone();
                        } else if let Some(asset) = asset_in_text(&msg.text, |path| {
                            state.asset_browser.iter().any(|(known, _)| known == path)
                        }) {
                            clicked_asset = Some(asset);
                        }
                    }
                    if msg.count > 1 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("×{}", msg.count))
                                    .monospace()
                                    .size(10.5)
                                    .color(theme::colors::TEXT_DIM),
                            );
                        });
                    }
                });
            }
        });

    if let Some(source) = clicked {
        state.open_source(&source);
    }
    if let Some(asset) = clicked_asset {
        select_asset(state, &asset);
    }
}

/// 在 Assets 面板里定位一个资产。
///
/// 选中之外还要**清掉搜索框**（当它会把目标藏起来时）：否则"点了报错行却什么都没
/// 发生"—— 资产其实是选中了，只是被过滤掉了。这条判断放在这里而不是 Assets 面板，
/// 是因为只有"定位"这个动作用户预期是"立刻看见"。
fn select_asset(state: &mut EditorState, asset: &str) {
    let filter = state.asset_filter.trim().to_lowercase();
    if !filter.is_empty() && !asset.to_lowercase().contains(&filter) {
        state.asset_filter.clear();
    }
    state.asset_selected = Some(asset.to_owned());
    log::info!("控制台定位资产：{asset}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(level: LogLevel, text: &str, count: u32) -> LogMessage {
        LogMessage {
            level,
            text: text.to_owned(),
            count,
            source: None,
        }
    }

    /// 统计按**出现次数**求和（重复消息的 ×N 计入），与筛选开关无关。
    #[test]
    fn level_counts_sum_occurrences_not_rows() {
        let messages = [
            message(LogLevel::Info, "a", 1),
            message(LogLevel::Info, "a", 7),
            message(LogLevel::Warning, "b", 2),
            message(LogLevel::Error, "c", 1),
        ];
        assert_eq!(level_counts(&messages), [8, 2, 1, 0]);
        // 空表全零（Clear 之后面板照样要画）。
        assert_eq!(level_counts(&[]), [0, 0, 0, 0]);
    }

    /// 从日志正文里认出项目相对的资产路径：引号、括号、逗号、`脚本:行号` 都要能吃。
    #[test]
    fn asset_in_text_recognizes_project_relative_paths() {
        let known = |path: &str| {
            matches!(
                path,
                "Resources/Models/Box.glb" | "Scripts/player.lua" | "Resources/UI/Icon.png"
            )
        };

        assert_eq!(
            asset_in_text("模型加载失败: 'Resources/Models/Box.glb'", known).as_deref(),
            Some("Resources/Models/Box.glb"),
            "引号包裹的路径要认出来"
        );
        assert_eq!(
            asset_in_text("Scripts/player.lua:42: 语法错误", known).as_deref(),
            Some("Scripts/player.lua"),
            "带行号的写法要按冒号切开"
        );
        assert_eq!(
            asset_in_text("缺两张图 Resources/UI/Icon.png, Resources/UI/Missing.png", known)
                .as_deref(),
            Some("Resources/UI/Icon.png"),
            "逗号分隔时命中第一个**存在**的"
        );
        // 不在资产库里 = 不是资产（避免把"提到了 .png"当成可跳转）。
        assert!(asset_in_text("找不到 Resources/UI/Missing.png", known).is_none());
        // 越界与绝对路径一律不认（归一化会拒），也不许把盘符当相对路径。
        assert!(asset_in_text("../escape/Box.glb", known).is_none());
        assert!(asset_in_text("D://proj//Resources//Models//Box.glb", known).is_none());
        // 非资产后缀不当链接。
        assert!(asset_in_text("build.rs 编译失败", known).is_none());
        assert!(asset_in_text("没有路径的一句话", known).is_none());
    }

    /// 预筛只看后缀，但必须覆盖全部资产类型 —— 漏一个类型就等于那一类资产不可点。
    #[test]
    fn mentions_asset_covers_every_suffix() {
        for suffix in ASSET_SUFFIXES {
            let text = format!("加载失败 Resources/x{suffix}");
            assert!(mentions_asset(&text), "{suffix} 应当被预筛命中");
        }
        assert!(!mentions_asset("纯文本消息"));
    }
}
