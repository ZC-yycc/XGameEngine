//! 类 Unity 工具栏——播放/暂停/停止控制和变换工具选择。

use crate::state::{EditorState, PlayMode, ToolMode};

/// 工具栏高度（逻辑像素）。
pub const TOOLBAR_HEIGHT: f32 = 32.0;

/// 渲染顶部工具栏。
pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    egui::TopBottomPanel::top("toolbar")
        .min_height(TOOLBAR_HEIGHT)
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // ── 项目 ──
                if ui
                    .button("⌂  Projects")
                    .on_hover_text("关闭当前项目，返回项目枢纽")
                    .clicked()
                {
                    state.return_to_hub = true;
                }

                ui.separator();

                // ── 保存 ──
                // 脏时给个 "*"，与窗口标题栏一致 —— 在工具栏还是标题栏看到的
                // 都是同一份按帧 memo 的脏检查（不重复比较场景）。
                let dirty = state.scene_dirty_memo(ctx.cumulative_pass_nr());
                let save_label = if dirty { "💾  Save *" } else { "💾  Save" };
                if ui
                    .button(save_label)
                    .on_hover_text("保存入口场景（Ctrl+S）")
                    .clicked()
                {
                    let _ = state.save_scene();
                }
                if ui
                    .button("Save As…")
                    .on_hover_text(
                        "把当前场景另存成新文件并切过去（Ctrl+Shift+S）\n\
                         旧文件保持原样，改坏了随时能重来",
                    )
                    .clicked()
                {
                    state.save_as_open = true;
                    state.save_as_name = state.scene.name.clone();
                }

                ui.separator();

                // ── 播放控制 ──
                let play_btn = egui::Button::new(if state.play_mode == PlayMode::Playing {
                    "⏸  Playing"
                } else {
                    "▶  Play"
                });
                // 这里只切 `play_mode`；运行时的启停由主循环检测状态跃迁后执行
                // （`enter_play_mode` / `exit_play_mode`）。日志也在那边记，
                // 免得同一个动作在 Console 里出现两条。
                if ui.add(play_btn).clicked() {
                    state.play_mode = match state.play_mode {
                        PlayMode::Edit => PlayMode::Playing,
                        PlayMode::Playing => PlayMode::Edit,
                        PlayMode::Paused => PlayMode::Playing,
                    };
                }

                let pause_btn = egui::Button::new("⏸  Pause");
                if ui.add(pause_btn).clicked() && state.play_mode == PlayMode::Playing {
                    state.play_mode = PlayMode::Paused;
                    state.log_info("⏸ Pause — 物理与脚本暂停，场景保持当前状态");
                }

                let stop_btn = egui::Button::new("⏹  Stop");
                if ui.add(stop_btn).clicked() && state.play_mode != PlayMode::Edit {
                    state.play_mode = PlayMode::Edit;
                }

                // ── Rust 插件编译（P0.5）──
                //
                // 只在插件项目里出现。编译跑在后台线程，但**必须看得见**：
                // 按了 Play 却"什么都没发生"会被当成卡死，而这里恰恰是 UE 模式
                // 的体验核心 —— 后台编译 + 自动进 Play。
                if state.is_plugin_project() {
                    if state.plugin_building() {
                        let secs = state.plugin_build_elapsed().unwrap_or_default();
                        ui.add(egui::Spinner::new());
                        ui.label(format!("编译插件… {secs}s"))
                            .on_hover_text("后台 cargo build；完成后自动进入 Play");
                    } else if ui
                        .button("🔨  Build")
                        .on_hover_text("编译 Rust 插件（不进入 Play）\n产物同步进 Plugins/")
                        .clicked()
                    {
                        state.build_plugins();
                    }
                }

                ui.separator();

                // ── 脚本热重载 ──
                // 只在运行中有意义：编辑态没有运行时，脚本本来就没在跑。
                if state.play_mode != PlayMode::Edit {
                    if ui
                        .button("⟳  Scripts")
                        .on_hover_text(
                            "重新编译项目里的全部 .rhai 脚本\n\
                             改完脚本不用退出 Play，编译一次即刻生效",
                        )
                        .clicked()
                    {
                        state.reload_scripts();
                    }
                    ui.separator();
                }

                // ── 变换工具 ──
                let tools = [
                    (ToolMode::Translate, "↔"),
                    (ToolMode::Rotate, "↻"),
                    (ToolMode::Scale, "⇔"),
                    (ToolMode::Rect, "▭"),
                ];
                for (mode, icon) in &tools {
                    let is_active = state.tool_mode == *mode;
                    let btn = egui::SelectableLabel::new(is_active, *icon);
                    let size = egui::Vec2::new(28.0, 22.0);
                    if ui.add_sized(size, btn).clicked() {
                        state.tool_mode = *mode;
                    }
                }

                ui.separator();

                // ── 视图开关 ──
                let mut grid = state.show_grid;
                if ui
                    .add(egui::Checkbox::new(&mut grid, "Grid"))
                    .changed()
                {
                    state.show_grid = grid;
                }

                let mut physics = state.show_physics_debug;
                if ui
                    .add(egui::Checkbox::new(&mut physics, "Physics"))
                    .changed()
                {
                    state.show_physics_debug = physics;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // ── 构建 ──
                    // right_to_left 布局：先加的在最右边，所以先画构建按钮。
                    if ui
                        .button("🔨  Build")
                        .on_hover_text(
                            "把当前项目打包成可独立运行的游戏\n\
                             输出到 <项目目录>/Build/<项目名>/，产物里的 exe 双击即玩",
                        )
                        .clicked()
                    {
                        state.build_project();
                    }

                    // 构建过之后才出现：直接打开产物目录
                    if let Some(dir) = state.last_build_dir.clone() {
                        if ui
                            .button("📂")
                            .on_hover_text(format!("打开输出目录\n{}", dir.display()))
                            .clicked()
                        {
                            open_in_file_manager(&dir);
                        }
                    }

                    // ── 项目设置 ──
                    if ui
                        .button("⚙")
                        .on_hover_text("项目设置\n窗口 / 重力 / 固定步长 / 内容目录")
                        .clicked()
                    {
                        state.settings_open = true;
                    }

                    ui.separator();
                    ui.label(format!("Project: {}", state.project_name()));
                    ui.separator();
                    // 场景名旁的星号与标题栏一致：脏状态来自同一份按帧 memo。
                    let star = if dirty { " *" } else { "" };
                    ui.label(format!(
                        "{}{star} | {} entities",
                        state.scene.name,
                        state.scene.entities.len()
                    ));
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
