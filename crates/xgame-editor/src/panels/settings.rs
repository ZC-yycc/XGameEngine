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

use crate::panels::{icons, theme};
use crate::state::EditorState;

/// 关卡顺序列表里的行内动作（收集完再改 Vec，避免边迭代边改）。
#[derive(Clone, Copy)]
enum RowAction {
    Up,
    Down,
    Remove,
}

/// 设置窗口里的单行文本字段：**撑满网格剩下的宽度**。
///
/// 不这么做的话每个字段只有 egui 的默认宽度（~220px），右边缘离窗口边框
/// 还差一大截：一列输入框右边缘不齐，窗口右边又空出一块。网格单元格的
/// `available_width()` 就是"本行到网格右边界"的剩余量，拿它定宽即可。
fn text_field(ui: &mut egui::Ui, value: &mut String) {
    let width = ui.available_width().max(160.0);
    ui.add_sized(
        egui::vec2(width, theme::FIELD_HEIGHT + 2.0),
        egui::TextEdit::singleline(value),
    );
}

pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    if !state.settings_open {
        return;
    }

    // 第一次显示（或上次被 Cancel/关闭丢弃）时克隆清单开草稿。
    if state.settings_draft.is_none() {
        state.settings_draft = Some(state.project.manifest().clone());
    }

    // 发布清单的"可添加项"= 盘上所有场景。只在窗口开着时扫一次目录
    // （文件数是个位数），换来"刚在 Scenes 菜单里新建的场景立刻可选"。
    let all_scenes = state.project.list_scenes();

    let mut open = state.settings_open;
    let mut apply = false;
    let mut reset = false;
    let mut cancel = false;

    // 内容比一屏高：**必须给滚动区一个上限**，否则窗口会按内容高度长到屏幕外，
    // 顶部的标题栏和底部的 Apply / Cancel 一起被裁掉（用户连确认都点不到）。
    // 上限按窗口高度算，留出标题栏与页脚的位置。
    let body_max = (ctx.screen_rect().height() - 190.0).max(240.0);

    egui::Window::new("Project Settings")
        .open(&mut open)
        .collapsible(false)
        // 固定高度上限之后，拉大窗口只会把滚动区撑在同一高度上，不如不给拖。
        .resizable(false)
        .default_width(470.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let Some(draft) = state.settings_draft.as_mut() else {
                return;
            };

            egui::ScrollArea::vertical()
                .max_height(body_max)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                // ── 项目 ──
                theme::section_bar(ui, "项目");
                egui::Grid::new("set_proj")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(88.0)
                    .show(ui, |ui| {
                        ui.label("项目名");
                        text_field(ui, &mut draft.name);
                        ui.end_row();

                        ui.label("入口场景")
                            .on_hover_text("打开项目时加载的场景（项目相对路径）");
                        text_field(ui, &mut draft.entry_scene);
                        ui.end_row();
                    });

                ui.add_space(10.0);

                // ── 构建 ──
                //
                // 顺序即关卡索引：脚本 `scene_index()` / `load_scene_at(i)` 读的就是
                // 这份清单。用列表而不是一段逗号分隔的文本框，是因为"顺序"本身
                // 是这份数据的语义 —— 让用户手打顺序等于把 `index + 1` 的 bug
                // 挪到配置里。
                theme::section_bar(ui, "构建（版本 / 场景顺序）");
                egui::Grid::new("set_build")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(88.0)
                    .show(ui, |ui| {
                        ui.label("版本号")
                            .on_hover_text("发布版本，只用于展示（独立游戏启动日志 / 构建报告）");
                        text_field(ui, &mut draft.build.version);
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.label("关卡顺序")
                    .on_hover_text("顺序即关卡索引：脚本 scene_index() 取当前关、load_scene_at(i) 切关");
                if draft.build.scenes.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "未配置：运行时不提供关卡顺序，脚本只能按路径 load_scene()。",
                        )
                        .color(theme::colors::TEXT_DIM),
                    );
                }

                // 先收集动作再改列表：`iter()` 期间不能 `&mut` 同一个 Vec。
                let mut action: Option<(usize, RowAction)> = None;
                for (index, path) in draft.build.scenes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let up = ui
                            .add_enabled(index > 0, egui::Button::new("上移"))
                            .on_hover_text("往前一关（索引 -1）");
                        if up.clicked() {
                            action = Some((index, RowAction::Up));
                        }
                        let down = ui
                            .add_enabled(
                                index + 1 < draft.build.scenes.len(),
                                egui::Button::new("下移"),
                            )
                            .on_hover_text("往后一关（索引 +1）");
                        if down.clicked() {
                            action = Some((index, RowAction::Down));
                        }
                        ui.label(format!("{index}. {path}"));
                        if icons::close_button(ui, "从发布清单里移除（不删文件）") {
                            action = Some((index, RowAction::Remove));
                        }
                    });
                }
                match action {
                    Some((index, RowAction::Up)) => draft.build.scenes.swap(index, index - 1),
                    Some((index, RowAction::Down)) => draft.build.scenes.swap(index, index + 1),
                    Some((index, RowAction::Remove)) => {
                        draft.build.scenes.remove(index);
                    }
                    None => {}
                }

                // 添加：只列磁盘上还没进清单的场景（重复项在 Apply 时还会再挡一次）。
                let available: Vec<String> = all_scenes
                    .iter()
                    .filter(|scene| !draft.build.scenes.iter().any(|listed| listed == *scene))
                    .cloned()
                    .collect();
                if available.is_empty() {
                    ui.label(
                        egui::RichText::new("（所有场景都已列入）")
                            .color(theme::colors::TEXT_DIM),
                    );
                } else {
                    egui::ComboBox::from_id_salt("set_build_add")
                        .selected_text("添加场景…")
                        .show_ui(ui, |ui| {
                            for scene in &available {
                                if ui.selectable_label(false, scene).clicked() {
                                    draft.build.scenes.push(scene.clone());
                                }
                            }
                        });
                }

                ui.add_space(10.0);

                // ── 窗口 ──
                theme::section_bar(ui, "窗口");
                egui::Grid::new("set_win")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(88.0)
                    .show(ui, |ui| {
                        ui.label("标题");
                        text_field(ui, &mut draft.window.title);
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

                ui.add_space(10.0);

                // ── 运行时 ──
                theme::section_bar(ui, "运行时（物理 / 时间）");
                egui::Grid::new("set_rt")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(88.0)
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

                ui.add_space(10.0);

                // ── 画面 ──
                theme::section_bar(ui, "画面（曝光 / 色调映射 / 泛光 / FXAA）");
                egui::Grid::new("set_render")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(88.0)
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

                        ui.label("FXAA").on_hover_text(
                            "后处理抗锯齿：平滑 MSAA 覆盖不到的几何 / 着色器走样。\
                             默认关（MSAA 已经处理几何边缘，开着会改变旧画面）；\
                             低采样档位或细长几何多的场景收益最明显。\
                             需要场景先渲进 HDR 中间标，属于后处理链的最后一趟",
                        );
                        ui.checkbox(&mut draft.render.fxaa.enabled, "");
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

                ui.add_space(10.0);
                theme::section_bar(ui, "天空（程序化天空盒 + 环境光照 IBL）");
                egui::Grid::new("set_sky")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(88.0)
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

                ui.add_space(10.0);

                ui.add_space(10.0);

                // ── 内容目录 ──
                theme::section_bar(ui, "内容目录（项目相对路径）");
                egui::Grid::new("set_dirs")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .min_col_width(88.0)
                    .show(ui, |ui| {
                        let c = &mut draft.content;
                        ui.label("场景");
                        text_field(ui, &mut c.scenes_dir);
                        ui.end_row();
                        ui.label("模型");
                        text_field(ui, &mut c.models_dir);
                        ui.end_row();
                        ui.label("贴图");
                        text_field(ui, &mut c.textures_dir);
                        ui.end_row();
                        ui.label("音频");
                        text_field(ui, &mut c.audio_dir);
                        ui.end_row();
                        ui.label("着色器");
                        text_field(ui, &mut c.shaders_dir);
                        ui.end_row();
                        ui.label("预制体");
                        text_field(ui, &mut c.prefabs_dir);
                        ui.end_row();
                        ui.label("脚本");
                        text_field(ui, &mut c.scripts_dir);
                        ui.end_row();
                    });
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Apply").color(egui::Color32::WHITE),
                        )
                        .fill(theme::colors::ACCENT_BLUE)
                        .min_size(egui::vec2(80.0, theme::FIELD_HEIGHT + 6.0)),
                    )
                    .on_hover_text("校验并写入 project.ron")
                    .clicked()
                {
                    apply = true;
                }
                if ui
                    .add(
                        egui::Button::new("Reset")
                            .min_size(egui::vec2(80.0, theme::FIELD_HEIGHT + 6.0)),
                    )
                    .on_hover_text("丢弃本次修改，恢复清单里的原值")
                    .clicked()
                {
                    reset = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new("Cancel")
                                .min_size(egui::vec2(80.0, theme::FIELD_HEIGHT + 6.0)),
                        )
                        .clicked()
                    {
                        cancel = true;
                    }
                });
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
