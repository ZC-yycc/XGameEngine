//! 类 UE 工具栏——文件、播放控制、变换工具、视图开关与构建。
//!
//! # 视觉纪律
//!
//! UE 的工具栏是一条**扁平的分组条**：按钮常态没有边框和底色，悬停才浮出浅灰；
//! 组与组之间用一条 18px 的竖线分开；所有按钮同高（[`theme::FIELD_HEIGHT`] + 2）。
//! 图标全部走 [`icons`] 的自绘几何图形 —— 字形图标（💾 / ⚙ / 🔨）的字宽和基线与
//! 文字不一致，混排时整条工具栏会歪。
//!
//! # 保留的行为
//!
//! 按钮只是换了画法，**语义与快捷键一个都没改**：项目枢纽返回、保存 / 另存为、
//! Play/Pause/Stop 与状态跃迁、插件编译、脚本热重载、四种变换工具、网格 / 物理
//! 调试开关、构建项目、打开产物目录、项目设置。

use crate::panels::icons::{self, Icon};
use crate::panels::theme::{self, colors};
use crate::state::{EditorState, PlayMode, ToolMode};

/// 工具栏高度（逻辑像素）。
pub const TOOLBAR_HEIGHT: f32 = theme::TOOLBAR_HEIGHT;

/// 渲染顶部工具栏。
pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    egui::TopBottomPanel::top("toolbar")
        .exact_height(TOOLBAR_HEIGHT)
        .frame(
            egui::Frame::default()
                .fill(colors::PANEL_HEADER_BG)
                .inner_margin(egui::Margin::symmetric(8.0, 0.0)),
        )
        .show(ctx, |ui| {
            // 底边 1px：把工具栏和下面的面板分割开（UE 的工具栏下沿就这一条线）。
            let full = ui.max_rect();
            ui.painter().hline(
                full.x_range(),
                full.bottom() - 0.5,
                egui::Stroke::new(1.0_f32, colors::SEPARATOR),
            );

            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;

                // ── 文件 ──
                if icons::icon_text_button(
                    ui,
                    Icon::Folder,
                    "Projects",
                    "关闭当前项目，返回项目枢纽",
                )
                .clicked()
                {
                    state.return_to_hub = true;
                }

                icons::v_separator(ui);

                // 脏时标签带 "*"，与窗口标题栏一致（同一份按帧 memo 的脏检查）。
                let dirty = state.scene_dirty_memo(ctx.cumulative_pass_nr());
                let save_label = if dirty { "Save *" } else { "Save" };
                if icons::icon_text_button(ui, Icon::Save, save_label, "保存入口场景（Ctrl+S）")
                    .clicked()
                {
                    let _ = state.save_scene();
                }
                if icons::text_button(ui, "Save As…", "另存为…（Ctrl+Shift+S）").clicked() {
                    state.save_as_open = true;
                    state.save_as_name = state.scene.name.clone();
                }

                icons::v_separator(ui);

                // ── 播放控制 ──
                let playing = state.play_mode == PlayMode::Playing;
                let play_color = if playing {
                    colors::PLAY_GREEN
                } else {
                    colors::PLAY_GREEN.gamma_multiply(0.85)
                };
                let play_label = if playing { "Playing" } else { "Play" };
                if icons::icon_text_button_colored(
                    ui,
                    Icon::Play,
                    play_label,
                    "进入 / 退出播放模式（Play 中再点一次回到编辑态）",
                    Some(play_color),
                )
                .clicked()
                {
                    state.play_mode = match state.play_mode {
                        PlayMode::Edit => PlayMode::Playing,
                        PlayMode::Playing => PlayMode::Edit,
                        PlayMode::Paused => PlayMode::Playing,
                    };
                }

                let paused = state.play_mode == PlayMode::Paused;
                if icons::icon_button(ui, Icon::Pause, 24.0, "暂停（物理与脚本停步）", paused)
                    .clicked()
                    && state.play_mode == PlayMode::Playing
                {
                    state.play_mode = PlayMode::Paused;
                    state.log_info("⏸ Pause — 物理与脚本暂停，场景保持当前状态");
                }

                if icons::icon_button(ui, Icon::Stop, 24.0, "停止并回到编辑态", false).clicked()
                    && state.play_mode != PlayMode::Edit
                {
                    state.play_mode = PlayMode::Edit;
                }

                // ── Rust 插件编译（P0.5）──
                if state.is_plugin_project() {
                    if state.plugin_building() {
                        let secs = state.plugin_build_elapsed().unwrap_or_default();
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(
                            egui::RichText::new(format!("编译插件… {secs}s"))
                                .size(11.0)
                                .color(colors::TEXT_SECONDARY),
                        )
                        .on_hover_text("后台 cargo build；完成后自动进入 Play");
                    } else if icons::icon_text_button(
                        ui,
                        Icon::Build,
                        // 不能也叫 "Build"：右边还有一个「构建游戏」的 Build，
                        // 同一条工具栏上两个一模一样的图标 + 文字，用户没法区分
                        // 哪个是编插件、哪个是打包。
                        "Build Plugins",
                        "编译 Rust 插件（不进入 Play）\n产物同步进 Plugins/",
                    )
                    .clicked()
                    {
                        state.build_plugins();
                    }
                }

                // ── 脚本热重载：只在运行中有意义 ──
                if state.play_mode != PlayMode::Edit
                    && icons::icon_button(
                        ui,
                        Icon::Refresh,
                        24.0,
                        "重新编译项目里的全部脚本（.lua）\n改完脚本不用退出 Play，编译一次即刻生效",
                        false,
                    )
                    .clicked()
                {
                    state.reload_scripts();
                }

                icons::v_separator(ui);

                // ── 变换工具 ──
                let tools = [
                    (ToolMode::Translate, Icon::Translate, "移动（W）"),
                    (ToolMode::Rotate, Icon::Rotate, "旋转（E）"),
                    (ToolMode::Scale, Icon::Scale, "缩放（R）"),
                    (ToolMode::Rect, Icon::Rect, "矩形工具"),
                ];
                for (mode, icon, tip) in tools {
                    if icons::icon_button(ui, icon, 26.0, tip, state.tool_mode == mode).clicked() {
                        state.tool_mode = mode;
                    }
                }

                icons::v_separator(ui);

                // ── 视图开关 ──
                if icons::icon_button(ui, Icon::Grid, 24.0, "显示网格", state.show_grid).clicked() {
                    state.show_grid = !state.show_grid;
                }
                if icons::icon_button(
                    ui,
                    Icon::Cube,
                    24.0,
                    "显示物理调试线框（碰撞体 / 接触点）",
                    state.show_physics_debug,
                )
                .clicked()
                {
                    state.show_physics_debug = !state.show_physics_debug;
                }

                // ── 右侧：场景信息 / 设置 / 构建 ──
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::icon_text_button(
                        ui,
                        Icon::Build,
                        "Build",
                        "把当前项目打包成可独立运行的游戏\n\
                         输出到 <项目目录>/Build/<项目名>/，产物里的 exe 双击即玩",
                    )
                    .clicked()
                    {
                        state.build_project();
                    }

                    if let Some(dir) = state.last_build_dir.clone() {
                        if icons::icon_button(
                            ui,
                            Icon::Folder,
                            24.0,
                            &format!("打开输出目录\n{}", dir.display()),
                            false,
                        )
                        .clicked()
                        {
                            open_in_file_manager(&dir);
                        }
                    }

                    if icons::icon_button(
                        ui,
                        Icon::Gear,
                        24.0,
                        "项目设置\n窗口 / 重力 / 固定步长 / 内容目录",
                        false,
                    )
                    .clicked()
                    {
                        state.settings_open = true;
                    }

                    icons::v_separator(ui);

                    // 场景名 + 实体数：小字、次要色，不跟左边的操作抢注意力。
                    let star = if dirty { " *" } else { "" };
                    ui.label(
                        egui::RichText::new(format!(
                            "{}{star}   ·   {} entities",
                            state.scene.name,
                            state.scene.entities.len()
                        ))
                        .size(11.5)
                        .color(colors::TEXT_SECONDARY),
                    )
                    .on_hover_text(format!("项目：{}", state.project_name()));
                });
            });
        });
}

/// 用系统文件管理器打开目录。
///
/// Windows 上 `explorer` 即使成功也可能返回非 0 退出码，所以这里**不检查**退出码。
fn open_in_file_manager(dir: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}
