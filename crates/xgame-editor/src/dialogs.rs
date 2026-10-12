//! 应用级弹窗：未保存确认、场景另存为。
//!
//! 面板内的小弹窗（删除确认之类）留在各自面板文件里；这两个跟**整个会话**
//! 的去留走（关窗口 / 回枢纽 / 写盘），所以收在主循环旁边。
//!
//! 关窗拦截本身在 `main.rs`：`close_requested()` + `ViewportCommand::CancelClose`，
//! 这里只负责"问用户怎么办"，把决定以 [`ExitTarget`] 还回去。

use crate::state::{EditorState, ExitTarget};

/// 「未保存更改」确认框。
///
/// 返回 `Some(target)` 表示用户已放行（保存成功，或明确选了不保存）——
/// 主循环拿到后直接执行目标动作。三种可能：
///
/// - **保存**：存盘成功才放行；失败时弹窗留着（错误已进 Console），
///   用户可以改选「不保存」或「取消」；
/// - **不保存**：直接放行。场景在内存里仍是脏的，但目标动作（关窗 / 回枢纽）
///   本来就会丢弃它 —— 用 [`EditorState::exit_confirmed`] 记账，不会复弹；
/// - **取消**：关掉弹窗，什么都不发生。
pub fn show_exit_dialog(
    state: &mut EditorState,
    ctx: &egui::Context,
    pass_nr: u64,
) -> Option<ExitTarget> {
    let target = state.exit_dialog?;

    // 弹窗开着的期间用户可能自己 Ctrl+S 了 —— 已经不脏就直接放行。
    if !state.scene_dirty_memo(pass_nr) {
        state.exit_dialog = None;
        return Some(target);
    }

    let what = match target {
        ExitTarget::Hub => "返回项目枢纽",
        ExitTarget::Quit => "关闭编辑器",
    };

    let mut accept = false;
    let mut discard = false;
    let mut cancel = false;

    egui::Window::new("未保存的更改")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!(
                "场景「{}」有未保存的改动，{}前要处理一下。",
                state.scene.name, what
            ));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .button("保存")
                    .on_hover_text("写盘后继续（失败不会继续）")
                    .clicked()
                {
                    accept = true;
                }
                if ui
                    .button("不保存")
                    .on_hover_text("丢弃改动直接继续")
                    .clicked()
                {
                    discard = true;
                }
                if ui.button("取消").clicked() {
                    cancel = true;
                }
            });
        });

    if cancel {
        state.exit_dialog = None;
        return None;
    }
    if discard {
        state.exit_dialog = None;
        return Some(target);
    }
    if accept {
        match state.save_scene() {
            Ok(()) => {
                state.exit_dialog = None;
                return Some(target);
            }
            // 写盘失败：弹窗留着（错误已经在 Console 里）。
            Err(_) => return None,
        }
    }
    None
}

/// 「场景另存为」对话框：写新文件 + 切换入口场景（语义见
/// [`EditorState::save_scene_as`]）。
///
/// 失败（非法名 / 写盘错误）时对话框保持打开，错误已进 Console ——
/// 用户改个名字直接重试，不用重新点开。
pub fn show_save_as_dialog(state: &mut EditorState, ctx: &egui::Context) {
    if !state.save_as_open {
        return;
    }

    // 路径预览在开窗前算好，避免窗口闭包里同时借 `save_as_name` 和 `project`
    // （与 prefabs.rs 的命名对话框同一套路）。
    let scenes_dir = state.project.manifest().content.scenes_dir.clone();
    let preview = if scenes_dir.trim().is_empty() {
        format!("{}.ron", state.save_as_name.trim())
    } else {
        format!("{}/{}.ron", scenes_dir.trim_end_matches(['/', '\\']), state.save_as_name.trim())
    };

    let mut open = state.save_as_open;
    let mut confirm = false;
    let mut cancel = false;

    egui::Window::new("场景另存为")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label("写成一个新的场景文件，并把编辑器切过去 —— 旧文件保持原样。");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Name:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.save_as_name).desired_width(220.0),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            });
            ui.label(format!("→ {preview}"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    confirm = true;
                }
                if ui.button("取消").clicked() {
                    cancel = true;
                }
            });
        });

    state.save_as_open = open && !cancel;
    if confirm {
        let name = state.save_as_name.clone();
        if state.save_scene_as(&name).is_ok() {
            state.save_as_open = false;
        }
        // 失败：保持打开，用户改名重试。
    }
}
