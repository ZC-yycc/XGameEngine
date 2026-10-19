//! 「⚙ 项目设置」窗口——编辑 `project.ron` 里玩家可调的字段。
//!
//! # 编辑走草稿
//!
//! 打开窗口时把清单克隆成 `settings_draft`，界面上改的全是草稿；按
//! **Apply** 才校验 + 写回内存清单 + 落盘（[`EditorState::apply_settings_draft`]），
//! 关窗 / Cancel 直接丢弃。与 Undo 同理：半成品设置不该边敲键盘边生效——
//! 比如把重力改成 0 的过程中手一抖输成 0.0001，还没敲完就应用毫无意义。
//!
//! # 这个面板为什么值钱
//!
//! 重力 / 固定步长此前硬编码在 `WorldConfig::default()` 里，想调只能改引擎
//! 源码重编译。现在它们是**数据**：改完 Apply，编辑器 Play 与独立游戏二进制
//! 下次启动就用新值（两端走同一个 `WorldConfig::from_project_runtime`）。
//! 数据化的另一层意义是给 **AI 智能体**用：能被程序读写校验的配置，
//! 才能被智能体批量调参与自动验证，而不是"打开 GUI 拖滑块"。

use crate::state::EditorState;

pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    if !state.settings_open {
        return;
    }

    // 第一次显示（或上次被 Cancel/关闭丢弃）时克隆清单开草稿。
    if state.settings_draft.is_none() {
        state.settings_draft = Some(state.project.manifest().clone());
    }

    let mut open = state.settings_open;
    let mut apply = false;
    let mut reset = false;
    let mut cancel = false;

    egui::Window::new("⚙  Project Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let Some(draft) = state.settings_draft.as_mut() else {
                return;
            };

            egui::ScrollArea::vertical().show(ui, |ui| {
                // ── 项目 ──
                ui.label(egui::RichText::new("项目").strong());
                egui::Grid::new("set_proj")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("项目名");
                        ui.text_edit_singleline(&mut draft.name);
                        ui.end_row();

                        ui.label("入口场景")
                            .on_hover_text("打开项目时加载的场景（项目相对路径）");
                        ui.text_edit_singleline(&mut draft.entry_scene);
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.separator();

                // ── 窗口 ──
                ui.label(egui::RichText::new("窗口").strong());
                egui::Grid::new("set_win")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("标题");
                        ui.text_edit_singleline(&mut draft.window.title);
                        ui.end_row();

                        ui.label("尺寸");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut draft.window.width)
                                    .range(320..=16_384)
                                    .suffix(" px"),
                            );
                            ui.label("×");
                            ui.add(
                                egui::DragValue::new(&mut draft.window.height)
                                    .range(240..=16_384)
                                    .suffix(" px"),
                            );
                        });
                        ui.end_row();

                        ui.label("可调整大小");
                        ui.checkbox(&mut draft.window.resizable, "");
                        ui.end_row();

                        ui.label("垂直同步")
                            .on_hover_text("关闭后帧率不封顶、输入延迟更低（编辑器默认关）");
                        ui.checkbox(&mut draft.window.vsync, "");
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.separator();

                // ── 运行时 ──
                ui.label(egui::RichText::new("运行时（物理 / 时间）").strong());
                egui::Grid::new("set_rt")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("重力")
                            .on_hover_text("世界重力加速度（m/s²），Y 轴向下为负");
                        ui.horizontal(|ui| {
                            ui.label("X");
                            ui.add(egui::DragValue::new(&mut draft.runtime.gravity[0]).speed(0.1));
                            ui.label("Y");
                            ui.add(egui::DragValue::new(&mut draft.runtime.gravity[1]).speed(0.1));
                            ui.label("Z");
                            ui.add(egui::DragValue::new(&mut draft.runtime.gravity[2]).speed(0.1));
                        });
                        ui.end_row();

                        ui.label("固定步长")
                            .on_hover_text(
                                "物理 / 脚本的固定步长。越小越精确越吃 CPU；\
                                 保存时会夹进 [1/240s, 1/10s]",
                            );
                        // 界面用毫秒编辑（60Hz ≈ 16.7ms，比 0.016666 秒好读），
                        // 落盘仍是秒 —— 运行时只认秒。
                        let mut ms = draft.runtime.fixed_dt * 1000.0;
                        if ui
                            .add(egui::DragValue::new(&mut ms).range(4.0..=100.0).suffix(" ms"))
                            .changed()
                        {
                            draft.runtime.fixed_dt = ms / 1000.0;
                        }
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.separator();

                // ── 画面 ──
                ui.label(egui::RichText::new("画面（曝光 / 色调映射 / 泛光）").strong());
                egui::Grid::new("set_render")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("色调映射").on_hover_text(
                            "ACES：高光滚降、过曝仍保留层次（默认）；\
                             Off：只曝光 + 钳制（旧管线语义，高光裁成纯白）",
                        );
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut draft.render.tone_map,
                                xgame_core::ToneMapMode::Aces,
                                "ACES",
                            );
                            ui.selectable_value(
                                &mut draft.render.tone_map,
                                xgame_core::ToneMapMode::Off,
                                "Off",
                            );
                        });
                        ui.end_row();

                        ui.label("曝光")
                            .on_hover_text("线性光乘子：1.0 不增不减；保存时夹进 [0.05, 20]");
                        ui.add(
                            egui::DragValue::new(&mut draft.render.exposure)
                                .range(
                                    xgame_core::RenderConfig::MIN_EXPOSURE
                                        ..=xgame_core::RenderConfig::MAX_EXPOSURE,
                                )
                                .speed(0.02)
                                .fixed_decimals(2),
                        );
                        ui.end_row();

                        ui.label("泛光强度").on_hover_text(
                            "Bloom 光晕的叠加比例：0 = 关闭（渲染器整段跳过，零成本）；\
                             常用 0.4~0.8。保存时夹进 [0, 10]",
                        );
                        ui.add(
                            egui::DragValue::new(&mut draft.render.bloom.intensity)
                                .range(
                                    0.0..=xgame_core::BloomConfig::MAX_INTENSITY,
                                )
                                .speed(0.02)
                                .fixed_decimals(2),
                        );
                        ui.end_row();

                        ui.label("泛光阈值").on_hover_text(
                            "多亮算亮（曝光后的线性亮度，1.0 ≈ 显示白点）。\
                             保存时夹进 [0, 100]",
                        );
                        ui.add(
                            egui::DragValue::new(&mut draft.render.bloom.threshold)
                                .range(
                                    0.0..=xgame_core::BloomConfig::MAX_THRESHOLD,
                                )
                                .speed(0.05)
                                .fixed_decimals(2),
                        );
                        ui.end_row();

                        ui.label("泛光软膝").on_hover_text(
                            "阈值附近多宽的一段『淡入』（单位同阈值）：0 = 硬阈值，\
                             亮斑边缘会有硬边。保存时夹进 [0, 4]",
                        );
                        ui.add(
                            egui::DragValue::new(&mut draft.render.bloom.knee)
                                .range(0.0..=xgame_core::BloomConfig::MAX_KNEE)
                                .speed(0.02)
                                .fixed_decimals(2),
                        );
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.label(egui::RichText::new("天空（程序化天空盒 + 环境光照 IBL）").strong());
                egui::Grid::new("set_sky")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("启用").on_hover_text(
                            "开着时：背景是程序化天空，环境光 = ambient + IBL（两者相加）。\
                             关着时整段跳过，画面与天空上线前逐位一致。\
                             太阳的位置/颜色取场景里第一盏 active 的方向光",
                        );
                        ui.checkbox(&mut draft.render.sky.enabled, "");
                        ui.end_row();

                        ui.label("强度")
                            .on_hover_text("天空辐射的整体乘子；保存时夹进 [0, 10]");
                        ui.add(
                            egui::DragValue::new(&mut draft.render.sky.intensity)
                                .range(0.0..=xgame_core::SkyConfig::MAX_INTENSITY)
                                .speed(0.02)
                                .fixed_decimals(2),
                        );
                        ui.end_row();

                        ui.label("天顶色");
                        ui.horizontal(|ui| {
                            ui.color_edit_button_rgb(&mut draft.render.sky.zenith);
                        });
                        ui.end_row();

                        ui.label("地平线色");
                        ui.horizontal(|ui| {
                            ui.color_edit_button_rgb(&mut draft.render.sky.horizon);
                        });
                        ui.end_row();

                        ui.label("地面色");
                        ui.horizontal(|ui| {
                            ui.color_edit_button_rgb(&mut draft.render.sky.ground);
                        });
                        ui.end_row();

                        ui.label("太阳盘大小")
                            .on_hover_text("半径（度）。默认 1.5°；保存时夹进 [0.1, 20]");
                        ui.add(
                            egui::DragValue::new(&mut draft.render.sky.sun_size)
                                .range(
                                    xgame_core::SkyConfig::MIN_SUN_SIZE
                                        ..=xgame_core::SkyConfig::MAX_SUN_SIZE,
                                )
                                .speed(0.05)
                                .fixed_decimals(2),
                        );
                        ui.end_row();

                        ui.label("太阳强度").on_hover_text(
                            "太阳盘与光晕的乘子；没有方向光时整段不出现。保存时夹进 [0, 10]",
                        );
                        ui.add(
                            egui::DragValue::new(&mut draft.render.sky.sun_intensity)
                                .range(0.0..=xgame_core::SkyConfig::MAX_SUN_INTENSITY)
                                .speed(0.02)
                                .fixed_decimals(2),
                        );
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.separator();

                // ── 内容目录 ──
                ui.label(egui::RichText::new("内容目录（项目相对路径）").strong());
                egui::Grid::new("set_dirs")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        let c = &mut draft.content;
                        ui.label("场景");
                        ui.text_edit_singleline(&mut c.scenes_dir);
                        ui.end_row();
                        ui.label("模型");
                        ui.text_edit_singleline(&mut c.models_dir);
                        ui.end_row();
                        ui.label("贴图");
                        ui.text_edit_singleline(&mut c.textures_dir);
                        ui.end_row();
                        ui.label("音频");
                        ui.text_edit_singleline(&mut c.audio_dir);
                        ui.end_row();
                        ui.label("着色器");
                        ui.text_edit_singleline(&mut c.shaders_dir);
                        ui.end_row();
                        ui.label("预制体");
                        ui.text_edit_singleline(&mut c.prefabs_dir);
                        ui.end_row();
                        ui.label("脚本");
                        ui.text_edit_singleline(&mut c.scripts_dir);
                        ui.end_row();
                    });
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Apply")
                    .on_hover_text("校验并写入 project.ron")
                    .clicked()
                {
                    apply = true;
                }
                if ui
                    .button("Reset")
                    .on_hover_text("丢弃本次修改，恢复清单里的原值")
                    .clicked()
                {
                    reset = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    state.settings_open = open && !cancel;

    if apply {
        // 校验失败时草稿被 `apply_settings_draft` 留在原处 —— 窗口继续
        // 显示，用户直接改错的那个字段；成功时草稿已被 take，下一帧
        // 重新克隆清单，显示的正好是刚写回的值。
        let _ = state.apply_settings_draft();
    }
    if reset {
        state.settings_draft = Some(state.project.manifest().clone());
    }
    if !open {
        // 关窗（含右上角 X）：丢草稿。
        state.settings_draft = None;
    }
}
