//! 类 Unity 检视面板——显示并编辑选中实体的属性。
//!
//! # 撤销集成
//!
//! 面板里的每次编辑都会进入撤销栈（Ctrl+Z 可回退）。难点在于"改前快照"从哪来：
//! 控件一旦落到真实数据上，改前状态就已经没了。
//!
//! 这里的做法是**把控件指向实体的副本**（`working`），`state.scene` 在渲染 UI 的
//! 整个过程中保持不动，于是"改前快照"随时都能拿 —— 推迟到确认真的发生编辑时再拍：
//!
//! - 离散编辑（打字、点内置图元）：此刻 `state.scene` 就是改前状态，直接入栈；
//! - 拖拽类控件（`DragValue`）：拖动过程中每一帧 `changed()` 都是 true，逐帧入栈会
//!   让一次拖动塞进几十个撤销点、撤销变成"一像素一步"。所以**开始拖**时先把快照挂
//!   到 `undo_pending`，**松手**时才真正入栈。
//!
//! # 为什么不是"进入面板先克隆整份场景"
//!
//! 那样写法最直白，但**每帧**（哪怕用户什么都没动、只是鼠标划过界面）都要克隆整份
//! `Scene`。这里改成只克隆选中实体：每帧两个小字符串的拷贝，和整场景拷贝差了三个
//! 数量级；真正需要整场景快照的时刻仍然是整场景克隆，但那时是"确实编辑了一次"。

use crate::state::EditorState;

/// 从一个控件的响应里收集「有改动 / 拖拽开始 / 拖拽结束」三个信号。
fn collect(
    resp: &egui::Response,
    touched: &mut bool,
    drag_began: &mut bool,
    drag_ended: &mut bool,
) {
    *touched |= resp.changed();
    *drag_began |= resp.drag_started();
    *drag_ended |= resp.drag_stopped();
}

/// 在 `Ui` 内渲染检视面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    ui.separator();

    // 控件全部作用在 `working`（选中实体的副本）上，`state.scene` 在下面整段 UI 里
    // 都不会被碰到 —— 这正是"改前快照"还能拿到的原因。
    let Some(index) = state.selected_entity else {
        ui.weak("Select an entity from the Hierarchy to inspect its properties.");
        return;
    };
    let Some(mut working) = state.scene.entities.get(index).cloned() else {
        return;
    };

    // 世界坐标（层级解算后的结果）——局部变换不等于它在世界里的位置，
    // 有父节点时两者差着一个父级的变换，单独显示出来省得用户对着局部坐标发懵。
    let world_position = state.scene.world_transform(index).position;

    let mut touched = false;
    let mut drag_began = false;
    let mut drag_ended = false;

    {
        let entity = &mut working;

        // ── 实体名称 ──
        ui.horizontal(|ui| {
            ui.label("Name:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut entity.name)
                    .desired_width(f32::INFINITY),
            );
            touched |= resp.changed();
        });

        ui.separator();

        // ── 变换组件 ──
        ui.heading("Transform");
        ui.indent("transform", |ui| {
            // 位置
            ui.label("Position");
            ui.horizontal(|ui| {
                for (axis, value) in [
                    ("X", &mut entity.transform.position.x),
                    ("Y", &mut entity.transform.position.y),
                    ("Z", &mut entity.transform.position.z),
                ] {
                    ui.label(axis);
                    let resp = ui.add(egui::DragValue::new(value).speed(0.1).prefix("  "));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                }
            });

            // 旋转（欧拉角显示，内部仍是四元数）
            ui.label("Rotation");
            let euler = entity
                .transform
                .rotation
                .to_euler(xgame_math::glam::EulerRot::XYZ);
            let mut rot = [
                euler.0.to_degrees(),
                euler.1.to_degrees(),
                euler.2.to_degrees(),
            ];
            let rotation_changed = ui
                .horizontal(|ui| {
                    let mut any = false;
                    // 用 zip(iter_mut) 而不是数组字面量：`[&mut rot[0], &mut rot[1], ...]`
                    // 会被借用检查器判成"同一数组的多次可变借用"——数组索引不是字段，
                    // 拿不到字段级借用拆分。
                    for (axis, value) in ["X", "Y", "Z"].into_iter().zip(rot.iter_mut()) {
                        ui.label(axis);
                        let resp = ui.add(egui::DragValue::new(value).speed(0.5).suffix("°"));
                        collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                        any |= resp.changed();
                    }
                    any
                })
                .inner;
            if rotation_changed {
                entity.transform.rotation = xgame_math::glam::Quat::from_euler(
                    xgame_math::glam::EulerRot::XYZ,
                    rot[0].to_radians(),
                    rot[1].to_radians(),
                    rot[2].to_radians(),
                );
            }

            // 缩放（不允许 0 或负数——那会让法线矩阵退化，整个物体变黑）
            ui.label("Scale");
            ui.horizontal(|ui| {
                for (axis, value) in [
                    ("X", &mut entity.transform.scale.x),
                    ("Y", &mut entity.transform.scale.y),
                    ("Z", &mut entity.transform.scale.z),
                ] {
                    ui.label(axis);
                    let resp = ui.add(
                        egui::DragValue::new(value)
                            .speed(0.05)
                            .range(0.01..=f32::MAX),
                    );
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                }
            });

            ui.label(
                egui::RichText::new(format!(
                    "World: {:.2}, {:.2}, {:.2}",
                    world_position.x, world_position.y, world_position.z
                ))
                .monospace()
                .weak(),
            );
        });

        ui.separator();

        // ── 网格 ──
        ui.heading("Mesh");
        ui.indent("mesh", |ui| {
            ui.horizontal(|ui| {
                ui.label("Model:").on_hover_text(
                    "项目相对路径（如 Resources/Box.glb），或内置图元（builtin://cube）",
                );
                let mut path = entity.model.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut path)
                        .hint_text("Resources/… 或 builtin://cube")
                        .desired_width(f32::INFINITY),
                );
                if resp.changed() && path != entity.model {
                    entity.model = path;
                    touched = true;
                }
            });

            // 内置图元快捷切换——比手敲 builtin:// 路径友好得多
            ui.horizontal(|ui| {
                ui.label("Primitive:");
                for kind in xgame_render::mesh::BuiltinMesh::ALL {
                    let active = entity.model == kind.uri();
                    if ui.selectable_label(active, kind.label()).clicked() {
                        entity.model = kind.uri();
                        touched = true;
                    }
                }
            });
        });

        // ── 动画 ──
        //
        // 没有模型就没东西可播；但**已经有**动画组件时即使模型被清空也要显示，
        // 否则组件会被藏到永远摘不掉。剪辑列表来自渲染器的资产缓存 —— 模型还没
        // 加载时是空的，弱提示占位，下一帧预加载完成后自然有选项。
        if !entity.model.is_empty() || entity.animator.is_some() {
            ui.separator();
            ui.heading("Animation");
            ui.indent("animation", |ui| {
                let clips = state
                    .scene_renderer
                    .as_ref()
                    .map(|r| r.animation_clip_names(&entity.model))
                    .unwrap_or_default();
                // 骨骼名：层栈遮罩的下拉来源（去重、跳过匿名；模型未加载 → 空）。
                let bone_names = state
                    .scene_renderer
                    .as_ref()
                    .map(|r| r.bone_names(&entity.model))
                    .unwrap_or_default();
                let model_empty = entity.model.is_empty();

                if entity.animator.is_none() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("+ Add Animator")
                            .on_hover_text(
                                "挂一个动画播放器：默认自动播放、原速、循环，\n剪辑默认取该模型的第一段。",
                            )
                            .clicked()
                        {
                            let animator = xgame_core::Animator {
                                clip: clips.first().cloned().unwrap_or_default(),
                                ..xgame_core::Animator::default()
                            };
                            entity.animator = Some(animator);
                            touched = true;
                        }
                        if clips.is_empty() {
                            ui.weak(if model_empty {
                                "没有模型"
                            } else {
                                "模型未加载或没有动画剪辑"
                            });
                        }
                    });
                } else {
                    let mut remove = false;
                    let mut remove_machine = false;
                    if let Some(animator) = entity.animator.as_mut() {
                        // 机器模式：clip/speed/looping 由状态表决定，这三行换成
                        // 机器信息 + 参数调节；Fade/Autoplay 两行仍生效、照常显示。
                        //
                        // 坏机器（entry 指不到状态）不算机器模式 —— 走单剪辑 UI，
                        // 用户还能改 clip / 转状态机自救，不会被一张死表锁死。
                        let machine_usable =
                            animator.machine.as_ref().is_some_and(|m| m.is_usable());
                        if machine_usable {
                            let current = state
                                .scene_renderer
                                .as_ref()
                                .and_then(|r| r.animation_state(entity.id));
                            let entry_name = animator
                                .machine
                                .as_ref()
                                .map(|m| m.entry.clone())
                                .unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.label("Machine:")
                                    .on_hover_text("动画状态机：播哪个剪辑由状态表+参数决定\n（states / transitions 在场景 RON 里编辑）");
                                ui.monospace(entry_name.as_str());
                                if let Some(cur) = current {
                                    ui.label(egui::RichText::new("→").weak());
                                    ui.label(egui::RichText::new(cur).monospace().weak());
                                }
                                if ui
                                    .small_button("✕")
                                    .on_hover_text("移除状态机，回到单剪辑模式（clip 字段还在）")
                                    .clicked()
                                {
                                    remove_machine = true;
                                }
                            });
                            // 参数行：脚本/这里都能改，值随场景落盘。
                            if let Some(machine) = animator.machine.as_mut() {
                                if machine.parameters.is_empty() {
                                    ui.weak("没有参数（转移条件永远不满足，停在 entry）");
                                }
                                for (name, value) in machine.parameters.iter_mut() {
                                    ui.horizontal(|ui| {
                                        ui.label(name.as_str())
                                            .on_hover_text("状态机参数：转移条件拿它和阈值比");
                                        let resp = ui.add(
                                            egui::DragValue::new(value)
                                                .speed(0.05)
                                                .range(-100.0..=100.0),
                                        );
                                        collect(
                                            &resp,
                                            &mut touched,
                                            &mut drag_began,
                                            &mut drag_ended,
                                        );
                                    });
                                }
                            }
                            ui.weak("states / transitions 在场景 RON 里编辑");
                        } else {
                            // 剪辑下拉：菜单一打开就把当前项勾出来，选中新项才算改动。
                            ui.horizontal(|ui| {
                                ui.label("Clip:");
                                let current = animator.clip.clone();
                                let label = if current.is_empty() {
                                    egui::RichText::new("（未选剪辑）").weak()
                                } else {
                                    egui::RichText::new(current.clone()).monospace()
                                };
                                ui.menu_button(label, |ui| {
                                    if clips.is_empty() {
                                        ui.weak(if model_empty {
                                            "没有模型"
                                        } else {
                                            "模型未加载或没有动画剪辑"
                                        });
                                    }
                                    for clip in clips.iter() {
                                        if ui
                                            .selectable_label(clip == &current, clip.as_str())
                                            .clicked()
                                        {
                                            animator.clip = clip.clone();
                                            touched = true;
                                            ui.close_menu();
                                        }
                                    }
                                });
                                if ui
                                    .small_button("✕")
                                    .on_hover_text("摘掉动画组件（模型与剪辑都不动）")
                                    .clicked()
                                {
                                    remove = true;
                                }
                            });

                            ui.horizontal(|ui| {
                                ui.label("Speed:")
                                    .on_hover_text("播放速度倍率；0 = 冻结在当前帧");
                                let mut speed = animator.speed;
                                let resp = ui.add(
                                    egui::DragValue::new(&mut speed).speed(0.05).suffix("x"),
                                );
                                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                                if resp.changed() {
                                    animator.speed = speed.clamp(0.0, 10.0);
                                }
                            });

                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut animator.looping, "Loop").changed() {
                                    touched = true;
                                }
                            });

                            // 转为状态机：用当前 clip/speed/looping 建一个单状态机器，
                            // entry 就是它 —— 之后在 RON 里加状态和转移即可。
                            if ui
                                .small_button("转为状态机（单状态）")
                                .on_hover_text(
                                    "把当前配置包成一台只有 entry 状态的机器：\n\
                                     之后在场景 RON 的 states/transitions 里加新状态与转移，\n\
                                     脚本用 set_anim_param 推参数驱动切换。",
                                )
                                .clicked()
                            {
                                let name = if animator.clip.is_empty() {
                                    "State".to_string()
                                } else {
                                    animator.clip.clone()
                                };
                                animator.machine = Some(xgame_core::AnimMachine {
                                    entry: name.clone(),
                                    parameters: Default::default(),
                                    states: vec![xgame_core::AnimState {
                                        name,
                                        clip: animator.clip.clone(),
                                        speed: animator.speed,
                                        looping: animator.looping,
                                    }],
                                    transitions: vec![],
                                });
                                touched = true;
                            }
                        }

                        // 叠加层：与基础层正交 —— 机器 / 单剪辑模式都显示这一行。
                        ui.horizontal(|ui| {
                            if animator.additive.is_none() {
                                if ui
                                    .small_button("+ Additive")
                                    .on_hover_text(
                                        "加一层叠加动画：在基础姿势上加增量（走路呼吸、\n\
                                         受击晃动都靠它），增量 = 叠加剪辑当前帧 − 第 0 帧。",
                                    )
                                    .clicked()
                                {
                                    animator.additive = Some(xgame_core::AnimAdditive::default());
                                    touched = true;
                                }
                            } else {
                                let mut remove_additive = false;
                                ui.label("Additive:")
                                    .on_hover_text("叠加层：基础姿势之上加一层增量（参考帧 = 叠加剪辑第 0 帧）");
                                if let Some(add) = animator.additive.as_mut() {
                                    // 剪辑下拉：与基础剪辑同一来源（模型的剪辑列表）。
                                    let current = add.clip.clone();
                                    let label = if current.is_empty() {
                                        egui::RichText::new("（选剪辑）").weak()
                                    } else {
                                        egui::RichText::new(current.clone()).monospace()
                                    };
                                    ui.menu_button(label, |ui| {
                                        if clips.is_empty() {
                                            ui.weak(if model_empty {
                                                "没有模型"
                                            } else {
                                                "模型未加载或没有动画剪辑"
                                            });
                                        }
                                        for clip in clips.iter() {
                                            if ui
                                                .selectable_label(clip == &current, clip.as_str())
                                                .clicked()
                                            {
                                                add.clip = clip.clone();
                                                touched = true;
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                    // 权重：与脚本 set_anim_additive_weight 同源（都写这个字段）。
                                    let resp = ui.add(
                                        egui::DragValue::new(&mut add.weight)
                                            .speed(0.01)
                                            .range(0.0..=1.0),
                                    );
                                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                                }
                                if ui
                                    .small_button("✕")
                                    .on_hover_text("摘掉叠加层（基础层不动）")
                                    .clicked()
                                {
                                    remove_additive = true;
                                }
                                if remove_additive {
                                    animator.additive = None;
                                    touched = true;
                                }
                            }
                        });

                        // 混合层栈：与 additive 快捷字段正交 —— additive 先求值，
                        // 这里按数组顺序依次叠上（后层盖前层）。增删在闭包外收集、
                        // 循环后统一应用（闭包里正借着 animator 的可变引用）。
                        let mut remove_layer: Option<usize> = None;
                        if !animator.layers.is_empty() {
                            ui.label("Layers:")
                                .on_hover_text(
                                    "混合层栈：按顺序叠到基础姿势上（后层盖前层）；\n\
                                     Additive = 加增量，Override = 整段覆盖；\n\
                                     遮罩 = 骨骼子树根名单（留空 = 整副骨架）",
                                );
                        }
                        for idx in 0..animator.layers.len() {
                            // ── 第一行：摘除 / 名字 / 模式 / 剪辑 / 权重 ──
                            ui.horizontal(|ui| {
                                let layer = &mut animator.layers[idx];
                                if ui
                                    .small_button("✕")
                                    .on_hover_text("摘掉这一层")
                                    .clicked()
                                {
                                    remove_layer = Some(idx);
                                }
                                // 层名：脚本 set_anim_layer_weight 按它查找。
                                let mut name = layer.name.clone();
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut name)
                                        .desired_width(64.0)
                                        .hint_text("name"),
                                );
                                if resp.changed() {
                                    layer.name = name;
                                    touched = true;
                                }
                                // 模式下拉（与剪辑同一套 menu_button 惯例）。
                                let mode_label =
                                    egui::RichText::new(format!("{:?}", layer.mode)).monospace();
                                ui.menu_button(mode_label, |ui| {
                                    let variants = [
                                        (
                                            xgame_core::AnimLayerMode::Additive,
                                            "Additive（加增量）",
                                        ),
                                        (xgame_core::AnimLayerMode::Override, "Override（覆盖）"),
                                    ];
                                    for (mode, label) in variants {
                                        if ui
                                            .selectable_label(layer.mode == mode, label)
                                            .clicked()
                                        {
                                            layer.mode = mode;
                                            touched = true;
                                            ui.close_menu();
                                        }
                                    }
                                });
                                // 剪辑下拉：与基础剪辑同一来源。
                                let current = layer.clip.clone();
                                let label = if current.is_empty() {
                                    egui::RichText::new("（选剪辑）").weak()
                                } else {
                                    egui::RichText::new(current.clone()).monospace()
                                };
                                ui.menu_button(label, |ui| {
                                    if clips.is_empty() {
                                        ui.weak(if model_empty {
                                            "没有模型"
                                        } else {
                                            "模型未加载或没有动画剪辑"
                                        });
                                    }
                                    for clip in clips.iter() {
                                        if ui
                                            .selectable_label(clip == &current, clip.as_str())
                                            .clicked()
                                        {
                                            layer.clip = clip.clone();
                                            touched = true;
                                            ui.close_menu();
                                        }
                                    }
                                });
                                // 权重：与脚本 set_anim_layer_weight 同源（都写这个字段）。
                                let resp = ui.add(
                                    egui::DragValue::new(&mut layer.weight)
                                        .speed(0.01)
                                        .range(0.0..=1.0),
                                );
                                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                            });
                            // ── 第二行：骨骼遮罩 chips + 加骨骼 ──
                            ui.horizontal(|ui| {
                                let layer = &mut animator.layers[idx];
                                ui.label("Mask:")
                                    .on_hover_text(
                                        "子树根名单：名单里的骨及其全部子孙被这层作用；\n\
                                         留空 = 整副骨架。glTF 匿名节点匹配不到，\n\
                                         只能靠有名字的祖先进名单。",
                                    );
                                if layer.mask.is_empty() {
                                    ui.weak("整副骨架");
                                }
                                let mut drop_mask: Option<usize> = None;
                                for (mi, m) in layer.mask.iter().enumerate() {
                                    if ui
                                        .small_button(format!("{m} ✕"))
                                        .on_hover_text("点一下把这根骨头移出遮罩")
                                        .clicked()
                                    {
                                        drop_mask = Some(mi);
                                    }
                                }
                                if let Some(mi) = drop_mask {
                                    layer.mask.remove(mi);
                                    touched = true;
                                }
                                ui.menu_button("+ 骨骼", |ui| {
                                    if bone_names.is_empty() {
                                        ui.weak(if model_empty {
                                            "没有模型"
                                        } else {
                                            "模型未加载"
                                        });
                                    }
                                    for b in bone_names.iter() {
                                        if layer.mask.contains(b) {
                                            continue;
                                        }
                                        if ui.small_button(b.as_str()).clicked() {
                                            layer.mask.push(b.clone());
                                            touched = true;
                                            ui.close_menu();
                                        }
                                    }
                                });
                            });
                        }
                        if let Some(idx) = remove_layer {
                            animator.layers.remove(idx);
                            touched = true;
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("+ Add Layer")
                                .on_hover_text(
                                    "往栈里加一层（默认：Additive、整副骨架、权重 1）",
                                )
                                .clicked()
                            {
                                let name = format!("layer{}", animator.layers.len());
                                animator.layers.push(xgame_core::AnimLayer {
                                    name,
                                    ..Default::default()
                                });
                                touched = true;
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label("Fade:")
                                .on_hover_text("换剪辑/换状态的交叉淡入时长；0 = 硬切");
                            let mut fade = animator.fade_time;
                            let resp = ui.add(
                                egui::DragValue::new(&mut fade)
                                    .speed(0.01)
                                    .range(0.0..=2.0)
                                    .suffix("s"),
                            );
                            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                            if resp.changed() {
                                animator.fade_time = fade.clamp(0.0, 2.0);
                            }
                        });

                        ui.horizontal(|ui| {
                            if ui
                                .checkbox(&mut animator.autoplay, "Autoplay")
                                .on_hover_text("关掉后停在剪辑第 0 帧（摆好第一帧再播）")
                                .changed()
                            {
                                touched = true;
                            }
                        });
                    }
                    if remove_machine {
                        if let Some(animator) = entity.animator.as_mut() {
                            animator.machine = None;
                        }
                        touched = true;
                    }
                    if remove {
                        entity.animator = None;
                        touched = true;
                    }
                }
            });
        }

        ui.separator();

        // ── 脚本 ──
        //
        // 这里存的是**项目相对路径**，不是脚本内容：同一份脚本可以挂在多个实体上，
        // 改一次逻辑全都生效。列表顺序就是执行顺序。
        ui.heading("Scripts");
        ui.indent("scripts", |ui| {
            let mut remove: Option<usize> = None;
            for (slot, path) in entity.scripts.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label("📜");
                    ui.label(egui::RichText::new(path).monospace().size(11.0))
                        .on_hover_text("项目相对路径；顺序即执行顺序");
                    if ui
                        .small_button("✕")
                        .on_hover_text("把这个脚本从实体上摘掉（脚本文件本身不动）")
                        .clicked()
                    {
                        remove = Some(slot);
                    }
                });
            }
            if let Some(slot) = remove {
                entity.scripts.remove(slot);
                touched = true;
            }

            ui.horizontal(|ui| {
                ui.menu_button("+ Add Script", |ui| {
                    if state.script_list.is_empty() {
                        ui.weak("项目里还没有 .rhai 脚本");
                        if ui.button("⟳ 重新扫描").clicked() {
                            state.refresh_scripts();
                            ui.close_menu();
                        }
                    } else {
                        for path in state.script_list.clone() {
                            if entity.scripts.contains(&path) {
                                continue;
                            }
                            if ui.button(path.clone()).clicked() {
                                entity.scripts.push(path);
                                touched = true;
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui.button("⟳ 重新扫描").clicked() {
                            state.refresh_scripts();
                            ui.close_menu();
                        }
                    }
                });
                if ui
                    .small_button("⟳")
                    .on_hover_text("重新扫描项目里的 Scripts/ 目录")
                    .clicked()
                {
                    state.refresh_scripts();
                }
            });

            ui.label(
                egui::RichText::new("钩子: start / update / fixed_update / on_destroy")
                    .weak()
                    .size(11.0),
            );
        });

        ui.separator();

        // ── 物理 ──
        //
        // 取消勾选 = 这个实体不参与物理（纯视觉）。旧场景文件没有 `physics` 字段，
        // 加载出来就是未勾选 —— 与当时"没有任何东西会掉下去"的行为一致。
        ui.heading("Physics");
        ui.indent("physics", |ui| {
            let mut enabled = entity.physics.is_some();
            if ui
                .checkbox(&mut enabled, "Participate in physics")
                .on_hover_text("不勾选 = 纯视觉实体，不会被重力拽走")
                .changed()
            {
                entity.physics = if enabled {
                    Some(xgame_core::PhysicsBody::default())
                } else {
                    None
                };
                touched = true;
            }

            if let Some(body) = entity.physics.as_mut() {
                use xgame_core::physics::{BodyKind, CollisionShape};

                ui.horizontal(|ui| {
                    ui.label("Type:");
                    for (kind, label) in [
                        (BodyKind::Static, "Static"),
                        (BodyKind::Dynamic, "Dynamic"),
                        (BodyKind::Kinematic, "Kinematic"),
                    ] {
                        if ui.selectable_label(body.kind == kind, label).clicked() {
                            body.kind = kind;
                            touched = true;
                        }
                    }
                });
                ui.label(
                    egui::RichText::new(match body.kind {
                        BodyKind::Static => "静态：地面 / 墙，不受力",
                        BodyKind::Dynamic => "动态：受重力与碰撞（物理驱动）",
                        BodyKind::Kinematic => "运动学：由脚本驱动位置（脚本驱动）",
                    })
                    .weak()
                    .size(11.0),
                );

                // -- 形状 --
                ui.horizontal(|ui| {
                    ui.label("Shape:");
                    let shape_names = ["Auto", "Box", "Ball", "Capsule", "Cylinder"];
                    let current = match body.shape {
                        CollisionShape::Auto => 0,
                        CollisionShape::Cuboid { .. } => 1,
                        CollisionShape::Ball { .. } => 2,
                        CollisionShape::Capsule { .. } => 3,
                        CollisionShape::Cylinder { .. } => 4,
                    };
                    for (i, name) in shape_names.iter().enumerate() {
                        if ui.selectable_label(current == i, *name).clicked() && current != i {
                            body.shape = match i {
                                0 => CollisionShape::Auto,
                                1 => CollisionShape::Cuboid {
                                    // 从 Auto 切到 Box 时，用当前缩放推一个合理初值，
                                    // 免得用户一上来就看到一个 0.5 的小方块。
                                    half_extents: CollisionShape::auto_half_extents(
                                        entity.transform.scale.to_array(),
                                    ),
                                },
                                2 => CollisionShape::Ball { radius: 0.5 },
                                3 => CollisionShape::Capsule {
                                    half_height: 0.5,
                                    radius: 0.3,
                                },
                                _ => CollisionShape::Cylinder {
                                    half_height: 0.5,
                                    radius: 0.5,
                                },
                            };
                            touched = true;
                        }
                    }
                });

                // -- 形状参数 --
                //
                // 必须 `match &mut body.shape` 而不是 `match body.shape`：
                // `CollisionShape` 是 `Copy`，写成后者会匹配到一个**临时副本**，
                // `ref mut` 绑在副本上，用户拖动参数时改动会被静默丢弃。
                match &mut body.shape {
                    CollisionShape::Auto => {
                        ui.label(
                            egui::RichText::new("Auto：按实体缩放的绝对值生成盒子")
                                .weak()
                                .size(11.0),
                        );
                    }
                    CollisionShape::Cuboid { half_extents } => {
                        ui.horizontal(|ui| {
                            ui.label("Half:");
                            for (axis, value) in ["x", "y", "z"].into_iter().zip(half_extents.iter_mut())
                            {
                                ui.label(axis);
                                let resp = ui.add(
                                    egui::DragValue::new(value)
                                        .speed(0.02)
                                        .range(0.001..=f32::MAX),
                                );
                                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                            }
                        });
                    }
                    CollisionShape::Ball { radius } => {
                        let resp = ui.add(
                            egui::DragValue::new(radius)
                                .speed(0.02)
                                .prefix("radius ")
                                .range(0.001..=f32::MAX),
                        );
                        collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    }
                    CollisionShape::Capsule { half_height, radius }
                    | CollisionShape::Cylinder { half_height, radius } => {
                        ui.horizontal(|ui| {
                            let a = ui.add(
                                egui::DragValue::new(half_height)
                                    .speed(0.02)
                                    .prefix("half-h ")
                                    .range(0.0..=f32::MAX),
                            );
                            collect(&a, &mut touched, &mut drag_began, &mut drag_ended);
                            let b = ui.add(
                                egui::DragValue::new(radius)
                                    .speed(0.02)
                                    .prefix("radius ")
                                    .range(0.001..=f32::MAX),
                            );
                            collect(&b, &mut touched, &mut drag_began, &mut drag_ended);
                        });
                    }
                }

                ui.horizontal(|ui| {
                    let f = ui.add(
                        egui::DragValue::new(&mut body.friction)
                            .speed(0.01)
                            .prefix("friction ")
                            .range(0.0..=2.0),
                    );
                    collect(&f, &mut touched, &mut drag_began, &mut drag_ended);
                    let r = ui.add(
                        egui::DragValue::new(&mut body.restitution)
                            .speed(0.01)
                            .prefix("bounce ")
                            .range(0.0..=1.0),
                    );
                    collect(&r, &mut touched, &mut drag_began, &mut drag_ended);
                });
                ui.horizontal(|ui| {
                    let m = ui.add(
                        egui::DragValue::new(&mut body.mass)
                            .speed(0.1)
                            .prefix("mass ")
                            .range(0.0..=10_000.0),
                    );
                    collect(&m, &mut touched, &mut drag_began, &mut drag_ended);
                    let g = ui.add(
                        egui::DragValue::new(&mut body.gravity_scale)
                            .speed(0.01)
                            .prefix("gravity ")
                            .range(-5.0..=5.0),
                    );
                    collect(&g, &mut touched, &mut drag_began, &mut drag_ended);
                });
                let sensor = ui
                    .checkbox(&mut body.is_sensor, "Sensor (trigger)")
                    .on_hover_text("只检测重叠、不产生碰撞力；射线默认会忽略它")
                    .changed();
                touched |= sensor;

                ui.label(
                    egui::RichText::new("mass 0 = 按碰撞体密度自动推算")
                        .weak()
                        .size(11.0),
                );
            }
        });

        ui.separator();

        // ── 添加组件（占位）──
        ui.add_enabled(false, egui::Button::new("+  Add Component"))
            .on_disabled_hover_text("自由组件系统尚未开放序列化；脚本 / 物理已可直接编辑");
    }

    // ── 记录撤销点 ──
    // 次序很重要：这些调用必须发生在把 `working` 写回场景**之前** ——
    // 那时候 `state.scene` 还是"改前"的样子。
    // Play 模式下场景被锁定，不参与撤销（也避免污染编辑态的历史）。
    if state.is_editing() {
        if drag_began {
            // 拖拽起始：`state.scene` 此刻仍是拖拽前的状态，整份克隆就是"改前快照"。
            state.undo_pending = Some(state.scene.clone());
        } else if drag_ended {
            if let Some(snapshot) = state.undo_pending.take() {
                state.push_undo_snapshot(snapshot);
            }
        } else if touched && state.undo_pending.is_none() {
            // 非拖拽的离散编辑。`undo_pending` 为 `Some` 说明此刻正处在一次拖拽
            // 中途，那个快照已经代表整次拖拽的起点，不能再补。
            state.push_undo();
        }
    }

    // ── 写回场景 ──
    // 拖拽期间逐帧写回是必须的：场景视图读的是 `state.scene`，只在松手时写回的话，
    // 拖滑块的过程中 3D 视图不会跟着动。
    if touched {
        if let Some(slot) = state.scene.entities.get_mut(index) {
            *slot = working;
        }
    }

    // ── 父节点（层级）──
    // 刻意放在写回**之后**：`reparent_entity` 按"保留世界变换"会改写实体自己的
    // `transform`，若先 reparent 再被 `working` 写回，那次换算就被覆盖掉了。
    ui.separator();
    ui.heading("Parent");
    let current_parent = state.scene.parent_index(index);
    let parent_label = match current_parent {
        Some(p) => state.scene.entities[p].name.clone(),
        None => "(none — scene root)".to_string(),
    };
    let mut chosen: Option<Option<usize>> = None;
    ui.add_enabled_ui(state.is_editing(), |ui| {
        egui::ComboBox::from_label("")
            .selected_text(parent_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current_parent.is_none(), "(none — scene root)")
                    .clicked()
                {
                    chosen = Some(None);
                }
                for i in 0..state.scene.entities.len() {
                    // 自己、以及自己的子孙都不能当父节点（会成环）
                    if i == index || state.scene.is_ancestor(index, i) {
                        continue;
                    }
                    let selected = current_parent == Some(i);
                    let name = state.scene.entities[i].name.clone();
                    if ui.selectable_label(selected, name).clicked() {
                        chosen = Some(Some(i));
                    }
                }
            });
    });
    if let Some(parent) = chosen {
        state.reparent_entity(index, parent);
    }

    // ── 预制体（实例来源 / Apply / Revert）──
    // 放在最后：这几个按钮会直接改场景（Revert 会重建整棵子树、让索引位移），
    // 绝不能和上面的 `working` 写回抢位置。
    let prefab_link = state
        .scene
        .entities
        .get(index)
        .and_then(|e| e.prefab.clone());
    if let Some(link) = prefab_link {
        ui.separator();
        ui.heading("Prefab");
        ui.label(
            egui::RichText::new(format!("Source: {}", link.source))
                .monospace()
                .weak()
                .size(11.0),
        );

        if state.is_prefab_root(index) {
            let editing = state.is_editing();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(editing, egui::Button::new("Apply"))
                    .on_hover_text("把当前实例的改动写回预制体资产文件")
                    .clicked()
                {
                    state.apply_prefab(index);
                }
                if ui
                    .add_enabled(editing, egui::Button::new("Revert"))
                    .on_hover_text("用资产覆盖回这个实例（保留它当前的位置）")
                    .clicked()
                {
                    state.revert_prefab(index);
                }
            });
        } else {
            let root = state.prefab_instance_root(index);
            let root_name = root.map(|r| state.scene.entities[r].name.clone());
            ui.label(
                egui::RichText::new(match &root_name {
                    Some(n) => format!("实例内部节点（根: {n}）"),
                    None => "实例内部节点".to_string(),
                })
                .weak()
                .size(11.0),
            );
            if let Some(r) = root {
                if ui.button("Select Instance Root").clicked() {
                    state.selected_entity = Some(r);
                }
            }
        }
    }
}
