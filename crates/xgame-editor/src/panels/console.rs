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

use crate::logging;
use crate::panels::{icons, theme};
use crate::state::{ConsoleFilter, EditorState, LogLevel, LogSource, CONSOLE_RENDER_LIMIT};

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

/// 一个等级开关。点亮的用等级本身的颜色，关掉的压成次级灰 —— 一眼看出"哪一类被滤掉了"。
fn level_toggle(ui: &mut egui::Ui, on: &mut bool, level: LogLevel, label: &str, tooltip: &str) {
    let color = if *on {
        level.color()
    } else {
        theme::colors::TEXT_DIM
    };
    let text = egui::RichText::new(label).size(11.0).color(color);
    if ui
        .selectable_label(*on, text)
        .on_hover_text(tooltip)
        .clicked()
    {
        *on = !*on;
    }
}

/// 标题栏：Clear / 四个等级开关 / 关键词 / 条数 / 日志文件入口。
///
/// `shown` 由调用方算好传进来（它就是同一帧要渲染的那份列表的长度）——
/// 这里再算一遍等于每帧白扫两遍消息表。
fn header(state: &mut EditorState, shown: usize, ui: &mut egui::Ui) {
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
        level_toggle(ui, &mut filter.info, LogLevel::Info, "Info", "显示普通信息");
        level_toggle(
            ui,
            &mut filter.warning,
            LogLevel::Warning,
            "Warn",
            "显示警告",
        );
        level_toggle(ui, &mut filter.error, LogLevel::Error, "Error", "显示错误");
        level_toggle(
            ui,
            &mut filter.success,
            LogLevel::Success,
            "OK",
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
    // 不用 `ui.separator()`：紧贴页签条下方，那条线会和页签的底边叠成双线。
    header(state, indices.len(), ui);

    // 点击后要 `&mut state` 才能开源码窗口，而这一整段还在只读地借消息表 ——
    // 所以先记下来、出了滚动区再开。
    let mut clicked: Option<LogSource> = None;

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
                    let label = if has_source {
                        label.sense(egui::Sense::click())
                    } else {
                        label
                    };
                    let resp = ui.add(label).on_hover_text(match &msg.source {
                        Some(src) => format!("{}\n\n点击打开源码：{}", msg.text, src.label()),
                        None => msg.text.clone(),
                    });
                    if has_source && resp.clicked() {
                        clicked = msg.source.clone();
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
}
