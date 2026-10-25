//! 应用级弹窗：未保存确认、场景切换确认、场景另存为 / 新建。
//!
//! 面板内的小弹窗（删除确认之类）留在各自面板文件里；这几个跟**整个会话**
//! 的去留走（关窗口 / 回枢纽 / 写盘 / 换场景），所以收在主循环旁边。
//!
//! 关窗拦截本身在 `main.rs`：`close_requested()` + `ViewportCommand::CancelClose`，
//! 这里只负责"问用户怎么办"，把决定以 [`ExitTarget`] 还回去。

use crate::panels::theme::colors;
use crate::state::{EditorState, ExitTarget, SceneChange};

/// 「发现自动保存」恢复框：上一次会话没走正常退出流程时出现。
///
/// 三个选择（都不做"自动恢复"：在用户没同意的情况下把场景换成一份旧现场，
/// 比丢改动更让人困惑）：
///
/// - **恢复**：载入副本。载入后仍是**未保存**状态（脏检查基准还是磁盘上那一份），
///   要 Ctrl+S 才真正写进场景文件；
/// - **丢弃**：删掉副本，继续用磁盘上的场景；
/// - **稍后**：这次不处理，副本留在盘上 —— 下次打开还会问。
pub fn show_autosave_recovery_dialog(state: &mut EditorState, ctx: &egui::Context) {
    let Some(notice) = state.pending_autosave.clone() else {
        return;
    };
    // Play / Pause 态下不该弹：进 Play 会换掉场景，恢复出来的内容会被
    // `play_snapshot` 覆盖掉，等于白恢复。留着副本，等回到编辑态。
    if !state.is_editing() {
        return;
    }

    let label = crate::state::scene_label(&notice.scene);
    let age = notice
        .age_label()
        .map(|a| format!("，写入于{a}"))
        .unwrap_or_default();

    let mut recover = false;
    let mut discard = false;
    let mut later = false;

    egui::Window::new("发现自动保存")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!("场景「{label}」有一份自动保存的副本{age}。"));
            ui.label("它来自上一次没有正常退出的会话 —— 可能是崩溃或被强制结束。");
            theme_note(
                ui,
                "恢复之后仍然是未保存状态：要按 Ctrl+S 才会写进场景文件。",
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button("恢复")
                    .on_hover_text("载入这份副本（随后仍需 Ctrl+S 落盘）")
                    .clicked()
                {
                    recover = true;
                }
                if ui
                    .button("丢弃")
                    .on_hover_text("删除副本，继续使用磁盘上的场景文件")
                    .clicked()
                {
                    discard = true;
                }
                if ui
                    .button("稍后")
                    .on_hover_text("这次不处理，副本留在盘上，下次打开再问")
                    .clicked()
                {
                    later = true;
                }
            });
        });

    if recover {
        let _ = state.recover_autosave();
    } else if discard {
        state.discard_autosave();
    } else if later {
        state.pending_autosave = None;
    }
}

/// 与面板里同款的说明文字（这条弹窗不进 dock，拿不到 `theme` 的排版上下文）。
fn theme_note(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(11.0)
            .color(crate::panels::theme::colors::TEXT_DIM),
    );
}

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

/// 切换场景 / 新建场景前的「未保存更改」确认框。
///
/// 与关窗确认同一套三选一语义，只是放行后执行的是 [`SceneChange`]：
/// 保存成功 / 不保存 → 切换；取消 → 清掉挂起请求，留在当前场景。
/// 弹窗开着期间用户自己 Ctrl+S（或用菜单保存）后不再脏，就直接放行。
pub fn show_scene_change_dialog(state: &mut EditorState, ctx: &egui::Context, pass_nr: u64) {
    let Some(change) = state.pending_scene_change.clone() else {
        return;
    };

    if state.play_mode != crate::state::PlayMode::Edit {
        // 弹窗开着时进了 Play：切换本身会被 `open_scene` 拒绝，直接撤销请求。
        state.pending_scene_change = None;
        return;
    }

    if !state.scene_dirty_memo(pass_nr) {
        state.pending_scene_change = None;
        state.finish_scene_change(change);
        return;
    }

    let target = match &change {
        SceneChange::Open(path) => format!("打开「{}」", crate::state::scene_label(path)),
        SceneChange::New => "新建场景".to_owned(),
    };

    let mut save = false;
    let mut discard = false;
    let mut cancel = false;

    egui::Window::new("未保存的更改")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!(
                "场景「{}」有未保存的改动，{}前要处理一下。",
                state.scene.name, target
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .button("保存")
                    .on_hover_text("保存当前场景后继续")
                    .clicked()
                {
                    save = true;
                }
                if ui
                    .button("不保存")
                    .on_hover_text("丢弃当前场景的改动后继续")
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
        state.pending_scene_change = None;
        return;
    }
    if discard {
        state.pending_scene_change = None;
        state.finish_scene_change(change);
        return;
    }
    if save && state.save_scene().is_ok() {
        state.pending_scene_change = None;
        state.finish_scene_change(change);
    }
    // 写盘失败（或没点保存）：确认框留着，错误已经在 Console 里。
}

/// 「新建场景」命名对话框：写盘后切过去，启动场景保持不变。
///
/// 失败（非法名 / 同名文件已存在 / 写盘错误）时对话框保持打开，错误已进
/// Console —— 用户改个名字直接重试。
pub fn show_new_scene_dialog(state: &mut EditorState, ctx: &egui::Context) {
    if !state.new_scene_open {
        return;
    }

    // 预览走与真正创建相同的校验：名字非法时直接显示错误，而不是等到点创建。
    let preview = state.scene_path_for_name(&state.new_scene_name);
    // 窗口开着时用户仍能点 Play；进 Play 后创建按钮要禁用（状态层也有硬保护）。
    let can_edit = state.is_editing();

    let mut open = state.new_scene_open;
    let mut confirm = false;
    let mut cancel = false;

    egui::Window::new("新建场景")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label("创建一个空场景（主相机 + 方向光）并切过去。");
            ui.label(
                "启动场景不变 —— 要让游戏从这里开始，用 Scenes 菜单的 Set Current as Startup。",
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Name:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.new_scene_name).desired_width(220.0),
                );
                if resp.lost_focus() && can_edit && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            });
            match &preview {
                Ok(path) => {
                    ui.label(format!("→ {path}"));
                }
                Err(e) => {
                    ui.colored_label(colors::ERROR, e);
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_edit, egui::Button::new("创建"))
                    .clicked()
                {
                    confirm = true;
                }
                if ui.button("取消").clicked() {
                    cancel = true;
                }
            });
        });

    state.new_scene_open = open && !cancel;
    if confirm && can_edit {
        let name = state.new_scene_name.clone();
        if state.new_scene(&name).is_ok() {
            state.new_scene_open = false;
        }
        // 失败：保持打开，用户改名重试。
    }
}

/// 「场景另存为」对话框：写新文件并切过去（语义见
/// [`EditorState::save_scene_as`](crate::state::EditorState::save_scene_as)）。
///
/// 失败（非法名 / 同名文件已存在 / 写盘错误）时对话框保持打开，错误已进
/// Console —— 用户改个名字直接重试，不用重新点开。
pub fn show_save_as_dialog(state: &mut EditorState, ctx: &egui::Context) {
    if !state.save_as_open {
        return;
    }

    // 路径预览走与真正保存相同的校验：名字非法时直接显示错误，而不是等到点保存。
    let preview = state.scene_path_for_name(&state.save_as_name);
    let can_edit = state.is_editing();

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
            ui.label(
                "启动场景不变；要让游戏从新场景开始，用 Scenes 菜单的 Set Current as Startup。",
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Name:");
                let resp = ui
                    .add(egui::TextEdit::singleline(&mut state.save_as_name).desired_width(220.0));
                if resp.lost_focus() && can_edit && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            });
            match &preview {
                Ok(path) => {
                    ui.label(format!("→ {path}"));
                }
                Err(e) => {
                    ui.colored_label(colors::ERROR, e);
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_edit, egui::Button::new("保存"))
                    .clicked()
                {
                    confirm = true;
                }
                if ui.button("取消").clicked() {
                    cancel = true;
                }
            });
        });

    state.save_as_open = open && !cancel;
    if confirm && can_edit {
        let name = state.save_as_name.clone();
        if state.save_scene_as(&name).is_ok() {
            state.save_as_open = false;
        }
        // 失败：保持打开，用户改名重试。
    }
}
