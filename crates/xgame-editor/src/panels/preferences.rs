//! 「Preferences」窗口——编辑器级偏好 + 面板布局。
//!
//! # 为什么是**编辑器**偏好而不是项目设置
//!
//! 相机手感、网格开关、面板布局描述的是"这个人怎么用编辑器"，换一个项目
//! 不该重置；而重力 / 步长是项目内容（改了对玩家行为有影响），归项目设置。
//! 两者写在不同文件：`editor.ron` 与 `project.ron`。
//!
//! # 为什么数值走草稿
//!
//! 与项目设置同一条纪律：拖数值框时不写盘、不逐帧改运行时状态，按 **Apply**
//! 才一次性生效。区别只有一个：这里**不提供 Cancel 回滚布局** —— 布局是
//! 直接拖动产生的，回滚它需要额外维护一份快照，收益不值得（恢复默认布局
//! 已覆盖"弄乱了想重来"）。

use egui_tiles::Tree;

use crate::config;
use crate::panels::dock::PanelTab;
use crate::panels::theme;
use crate::prefs::{self, EditorPreferences};
use crate::state::{EditorCamera, EditorState};

/// 窗口要对主循环做的事。
pub enum PreferencesAction {
    /// 用户按了 Apply：数值已写进 `EditorState`，这里带回要落盘的偏好。
    ///
    /// `Box` 是因为它带着整棵布局树（~248 字节），不装箱的话枚举的每个
    /// 返回值（含 `ResetLayout`）都会按最大变体占空间。
    Apply(Box<EditorPreferences>),
    /// 用户按了「恢复默认布局」：主循环重建布局树并保存。
    ResetLayout,
}

/// 偏好窗口的界面状态（打开位 + 编辑草稿）。
pub struct PreferencesWindow {
    open: bool,
    draft: Option<EditorPreferences>,
}

impl PreferencesWindow {
    pub fn new() -> Self {
        Self {
            open: false,
            draft: None,
        }
    }

    /// 打开窗口，并用当前生效的偏好填草稿。
    pub fn open(&mut self, current: &EditorPreferences) {
        self.open = true;
        self.draft = Some(current.clone());
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 画窗口；返回 `Some` 时由主循环执行。
    pub fn show(
        &mut self,
        state: &mut EditorState,
        tree: &mut Tree<PanelTab>,
        ctx: &egui::Context,
    ) -> Option<PreferencesAction> {
        if !self.open {
            return None;
        }
        if self.draft.is_none() {
            self.open = false;
            return None;
        }

        let mut open = self.open;
        let mut apply = false;
        let mut cancel = false;
        let mut reset = false;

        let store = config::file(prefs::FILE_NAME)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "（配置目录不可用）".to_owned());

        egui::Window::new("Preferences")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                let Some(draft) = self.draft.as_mut() else {
                    return;
                };

                // ── 视口 ──
                theme::section_bar(ui, "视口");
                egui::Grid::new("pref_viewport")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(96.0)
                    .show(ui, |ui| {
                        ui.label("相机速度")
                            .on_hover_text("自由相机平移速度（滚轮也能临时调整）");
                        ui.add(
                            egui::DragValue::new(&mut draft.camera_speed)
                                .range(EditorCamera::MIN_PAN_SPEED..=EditorCamera::MAX_PAN_SPEED)
                                .speed(0.1)
                                .suffix(" m/s"),
                        );
                        ui.end_row();

                        ui.label("鼠标灵敏度")
                            .on_hover_text("按住右键拖拽转视角时的灵敏度");
                        ui.add(
                            egui::DragValue::new(&mut draft.camera_sensitivity)
                                .range(
                                    EditorCamera::MIN_SENSITIVITY..=EditorCamera::MAX_SENSITIVITY,
                                )
                                .speed(0.0001)
                                .max_decimals(4),
                        );
                        ui.end_row();

                        ui.label("显示网格");
                        ui.checkbox(&mut draft.show_grid, "");
                        ui.end_row();
                    });

                // ── 自动保存 ──
                theme::section_bar(ui, "自动保存");
                egui::Grid::new("pref_autosave")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(96.0)
                    .show(ui, |ui| {
                        ui.label("间隔").on_hover_text(
                            "每隔多少秒在项目的 .autosave/ 下留一份副本；0 = 关闭",
                        );
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut draft.autosave_interval_secs)
                                    .range(0..=crate::autosave::MAX_INTERVAL_SECS)
                                    .speed(1.0)
                                    .suffix(" 秒"),
                            );
                            if ui
                                .small_button("关闭")
                                .on_hover_text("设为 0，不再自动保存")
                                .clicked()
                            {
                                draft.autosave_interval_secs = crate::autosave::OFF;
                            }
                        });
                        ui.end_row();
                    });
                theme::note(
                    ui,
                    "副本只用于崩溃恢复，不改变「未保存」状态 —— 要落盘仍然要 Ctrl+S。",
                );

                // ── 布局 ──
                theme::section_bar(ui, "面板布局");
                theme::note(
                    ui,
                    "拖动页签、调整分隔条后自动保存，下次启动恢复。",
                );
                theme::note(ui, format!("保存位置：{store}"));
                ui.add_space(4.0);
                if ui
                    .button("恢复默认布局")
                    .on_hover_text("立即重建出厂停靠布局（当前页签位置不再保留）")
                    .clicked()
                {
                    reset = true;
                }

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        if ui.button("Apply").clicked() {
                            apply = true;
                        }
                    });
                });
            });

        if !open || cancel {
            self.open = false;
            self.draft = None;
            return None;
        }

        if reset {
            // 保留窗口与草稿：用户可能还想接着调数值。布局由主循环立刻重建。
            return Some(PreferencesAction::ResetLayout);
        }

        if apply {
            let mut next = self.draft.clone()?;
            // 布局以**当前树**为准：窗口打开期间用户可能又拖了页签。
            next.layout = Some(tree.clone());
            next.apply_to_state(state);
            self.open = false;
            self.draft = None;
            return Some(PreferencesAction::Apply(Box::new(next)));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_copies_current_preferences_into_draft() {
        let mut window = PreferencesWindow::new();
        assert!(!window.is_open());

        let prefs = EditorPreferences {
            camera_speed: 9.5,
            camera_sensitivity: 0.003,
            show_grid: false,
            autosave_interval_secs: 30,
            layout: None,
        };
        window.open(&prefs);
        assert!(window.is_open());
        assert_eq!(window.draft.as_ref(), Some(&prefs));
    }
}
