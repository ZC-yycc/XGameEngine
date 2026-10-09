//! 预制体资源面板——浏览项目里的预制体资产，并把它们放进场景。
//!
//! 对应 Unity 的 Project 窗口里那一排 Prefab 资产。面板本身只负责"看"和"点"：
//!
//! - 列出项目 `Prefabs/`（可配置，见 `ContentConfig::prefabs_dir`）下所有
//!   `*.prefab.ron`，支持子目录归类和搜索；
//! - 点一下资产 = 实例化一份到场景里（落在根层）；
//! - 顶部「Create from Selection」= 把 Hierarchy 里选中的实体（含整棵子树）
//!   存成新的预制体资产。
//!
//! # 为什么列表要缓存
//!
//! 资产清单存在 [`EditorState::prefab_list`] 里，由 [`EditorState::refresh_prefabs`]
//! 在打开项目、创建/删除资产时刷新。**不是**每帧去扫目录——目录扫描是一串系统
//! 调用，逐帧做就是"界面没动却在耗 CPU"的典型来源（这个项目已经在这类问题上
//! 栽过几次）。列表因此可能滞后于手工在资源管理器里新建的文件，那个 ⟳ 按钮
//! 就是给这种情况准备的。

use xgame_core::prefab::prefab_display_name;

use crate::state::EditorState;

/// 在 `Ui` 内渲染预制体面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    let is_editing = state.is_editing();

    // ── 标题行 + 刷新 ──
    ui.horizontal(|ui| {
        ui.strong("Prefabs");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("⟳")
                .on_hover_text("重新扫描项目里的预制体资产\n（在资源管理器里手工增删过文件时用）")
                .clicked()
            {
                state.refresh_prefabs();
            }
        });
    });

    // ── 搜索 ──
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.prefab_filter)
                .hint_text("Search prefabs...")
                .desired_width(f32::INFINITY),
        );
    });

    // ── 从选中创建 ──
    let has_selection = state.selected_entity.is_some();
    let create_enabled = is_editing && has_selection;
    let create_hint = if !is_editing {
        "播放模式下不能修改项目"
    } else if !has_selection {
        "先在 Hierarchy 里选中一个实体"
    } else {
        "把选中实体（含整棵子树）存成可复用的预制体资产"
    };
    ui.horizontal(|ui| {
        if ui
            .add_enabled(create_enabled, egui::Button::new("📦  Create from Selection"))
            .on_hover_text(create_hint)
            .clicked()
        {
            state.begin_create_prefab();
        }
    });
    if is_editing && has_selection {
        let name = state.prefab_default_name();
        ui.label(
            egui::RichText::new(format!("Selection: {name}"))
                .weak()
                .size(11.0),
        );
    }

    ui.separator();

    // ── 资产列表 ──
    // 先把要画的行收集成 owned 值：绘制过程中要往 `state` 里写（触发实例化/删除），
    // 若同时持有对 `state.prefab_list` 的借用会撞借用检查器。
    let filter = state.prefab_filter.trim().to_lowercase();
    let entries: Vec<String> = state
        .prefab_list
        .iter()
        .filter(|p| filter.is_empty() || p.to_lowercase().contains(&filter))
        .cloned()
        .collect();
    let total = state.prefab_list.len();

    let mut instantiate: Option<String> = None;
    let mut request_delete: Option<String> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        if total == 0 {
            ui.weak("还没有预制体。选中一个实体后点上方 “Create from Selection”。");
            return;
        }
        if entries.is_empty() {
            ui.weak("没有匹配的预制体。");
            return;
        }

        for path in &entries {
            let name = prefab_display_name(path);
            let width = ui.available_width();
            let resp = ui
                .add_enabled(
                    is_editing,
                    egui::Button::new(format!("📦  {name}")).min_size(egui::vec2(width, 20.0)),
                )
                .on_hover_text(format!("{path}\n\n点击 = 实例化到场景"));
            if resp.clicked() {
                instantiate = Some(path.clone());
            }

            let path_for_menu = path.clone();
            resp.context_menu(|ui| {
                if ui
                    .add_enabled(is_editing, egui::Button::new("Instantiate"))
                    .on_disabled_hover_text("播放模式下不能修改场景")
                    .clicked()
                {
                    instantiate = Some(path_for_menu.clone());
                    ui.close_menu();
                }
                if ui.button("Copy Path").clicked() {
                    ui.ctx().copy_text(path_for_menu.clone());
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Delete Asset…").clicked() {
                    request_delete = Some(path_for_menu.clone());
                    ui.close_menu();
                }
            });
        }
    });

    // ── 应用本帧的点击结果 ──
    if let Some(path) = instantiate {
        state.instantiate_prefab_by_path(&path);
    }
    if let Some(path) = request_delete {
        state.prefab_delete_pending = Some(path);
    }

    // ── 弹窗 ──
    create_prefab_dialog(state, ui);
    delete_prefab_dialog(state, ui);
}

/// 「从选中创建预制体」的命名弹窗。
fn create_prefab_dialog(state: &mut EditorState, ui: &egui::Ui) {
    if !state.prefab_save_dialog {
        return;
    }

    // 路径预览在打开窗口**之前**算好，避免在窗口闭包里同时可变借用
    // `state.prefab_save_name` 和不可变借用 `state.project`。
    // 代价是预览比输入晚一帧，肉眼不可见。
    let preview = state.project.prefab_path_for(state.prefab_save_name.trim());
    let ctx = ui.ctx().clone();

    let mut confirm = false;
    let mut cancel = false;

    egui::Window::new("Create Prefab")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(&ctx, |ui| {
            ui.label("把选中实体（含整棵子树）存成可复用的预制体资产。");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Name:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.prefab_save_name).desired_width(220.0),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("→ {preview}"))
                    .monospace()
                    .weak()
                    .size(11.0),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if cancel {
        state.prefab_save_dialog = false;
    }
    if confirm {
        state.prefab_save_dialog = false;
        let name = state.prefab_save_name.clone();
        state.create_prefab_from_selection(&name);
    }
}

/// 删除预制体资产的确认弹窗。
///
/// 删除是**不可撤销**的文件操作（撤销栈管的是场景，管不了磁盘），所以必须
/// 二次确认，并把这句"不可撤销"写清楚。
fn delete_prefab_dialog(state: &mut EditorState, ui: &egui::Ui) {
    let Some(target) = state.prefab_delete_pending.clone() else {
        return;
    };

    let ctx = ui.ctx().clone();
    let mut confirm = false;
    let mut cancel = false;

    egui::Window::new("Delete Prefab")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(&ctx, |ui| {
            ui.label(format!("确定删除预制体资产 “{}” 吗？", prefab_display_name(&target)));
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("该操作不可撤销，文件会从磁盘上删除。")
                    .color(egui::Color32::from_rgb(230, 180, 90))
                    .size(11.0),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(target.clone()).monospace().weak().size(11.0));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if cancel {
        state.prefab_delete_pending = None;
    }
    if confirm {
        state.prefab_delete_pending = None;
        state.delete_prefab_asset(&target);
    }
}
