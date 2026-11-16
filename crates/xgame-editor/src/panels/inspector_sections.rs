//! 版式本身承载信息的组件段落。
//!
//! # 为什么这些不在 `inspector_reflect` 里
//!
//! `inspector_reflect` 那一套是"字段列表 → 行"的通用渲染：范围、步长、条件显示、
//! 同排分组全由元数据驱动。但下面这几段的**版式本身在传递信息**：
//!
//! - `Transform`：用户改的是**欧拉角**、存的是**四元数**，一改要连带规范化
//! - `Mesh`：Morph 目标的名字来自渲染器（模型加载之后才知道），不是编译期常量
//! - `Animation`：状态机是张图（状态 + 转移 + 参数），不是字段树
//! - `Scripts` / `Behaviors`：实例集合，每行的增减是"挂/摘一个组件"
//! - `Material`：通道来自 `.mat` 资产，同样是运行时才知道
//! - `Prefab`：实例 → 资产的关系操作（Apply / Revert），没有字段
//!
//! 把它们硬塞进 `FieldMeta` 的结果是往元数据里加十几个只有一处用得上的字段，
//! 而且"改完了没"仍然要靠手写代码判断 —— 那是把 DSL 造出来又不用它。
//!
//! # 但它们仍然在同一张表里
//!
//! 每一段都通过 `ReflectComponentSpec::render = Render::Custom` 登记，
//! 入口、分类条、右键菜单、脏标记、撤销时机与 `Fields` 那条路完全一致。
//! 换句话说：**统一的是入口，不是画法**（详见 `Docs/历史/反射系统设计.md` §15.6）。

use xgame_core::{BuiltinComponent, Entity};

use super::inspector::{
    collect, component_context_menu, row, show_behavior_fields, ComponentAction, ComponentAddress,
    ComponentMenuTarget, PLAY_STRUCTURE_HINT,
};
use super::inspector_reflect::SectionCx;
use super::{icons, theme};

/// Transform 段。
pub(super) fn show_transform(ui: &mut egui::Ui, entity: &mut Entity, cx: &mut SectionCx<'_>) {
    let mut touched = false;
    let mut drag_began = false;
    let mut drag_ended = false;
    let Some(index) = cx.state.selected_entity else {
        return;
    };
    // 世界坐标（层级解算后的结果）——局部变换不等于它在世界里的位置，
    // 有父节点时两者差着一个父级的变换，单独显示出来省得用户对着局部坐标发懵。
    let world_position = cx.state.scene.world_transform(index).position;
    // 三轴输入框的宽度按面板剩余空间算：面板够宽时等于 `FIELD_WIDTH`
    // （与旧画面逐像素一致），窗口收到 1280 时自动压窄 —— 否则最后的
    // Z 列会被面板右边缘裁掉，数值看不见也拖不到。
    // 必须在放第一个控件之前算：放完一个 `available_width()` 就少一截。
    let axis_w = theme::axis_field_width(ui);
    theme::field_row(ui, "Position", |ui| {
        for (axis, value) in [
            ("X", &mut entity.transform.position.x),
            ("Y", &mut entity.transform.position.y),
            ("Z", &mut entity.transform.position.z),
        ] {
            theme::axis_label(ui, axis);
            let resp = theme::drag_field_w(ui, axis_w, value, 0.1, "");
            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
        }
    });

    // 旋转（欧拉角显示，内部仍是四元数）
    //
    // 换算是 `xgame_core::quat_to_euler_deg` / `euler_deg_to_quat`
    // （顺序固定 **YXZ**）：脚本 API 的 `rot` / `set_rot` 与 MCP 的
    // `rotation_euler_deg` 都走同一个入口，否则"面板上读到 153°、
    // 脚本里读到 −130°"，作者会以为其中一处坏了。
    // 对相机这类物体，YXZ 还给出人类可读的 yaw / pitch（俯角 18°），
    // XYZ 分解在同样位姿下会折成 (−46°, 153°, 160°) 这种读不出来的三元组。
    let mut rot = xgame_core::quat_to_euler_deg(entity.transform.rotation).to_array();
    // 把 −0.000 抹成 0：四元数分解常在零附近给出负零，
    // 面板上显示 "−0.000°" 像是算错了（实际是同一角度）。
    for value in &mut rot {
        if value.abs() < 1e-3 {
            *value = 0.0;
        }
    }
    // 走 `field_row`（而不是手搓 horizontal）：标签列宽必须与 Position /
    // Scale 完全一致，否则 "Rotation" 比 "Position" 窄一点，下面 Scale 的
    // 三个输入框就跟着往左挪一档。
    let rotation_changed = theme::field_row(ui, "Rotation", |ui| {
        let mut any = false;
        // 用 zip(iter_mut) 而不是数组字面量：`[&mut rot[0], &mut rot[1], ...]`
        // 会被借用检查器判成"同一数组的多次可变借用"——数组索引不是字段，
        // 拿不到字段级借用拆分。
        for (axis, value) in ["X", "Y", "Z"].into_iter().zip(rot.iter_mut()) {
            theme::axis_label(ui, axis);
            let resp = theme::drag_field_w(ui, axis_w, value, 0.5, "°");
            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
            any |= resp.changed();
        }
        any
    });
    if rotation_changed {
        let degrees = xgame_math::glam::Vec3::from_array(rot);
        entity.transform.rotation = xgame_core::euler_deg_to_quat(degrees);
    }

    // 缩放（不允许 0 或负数——那会让法线矩阵退化，整个物体变黑）
    theme::field_row(ui, "Scale", |ui| {
        for (axis, value) in [
            ("X", &mut entity.transform.scale.x),
            ("Y", &mut entity.transform.scale.y),
            ("Z", &mut entity.transform.scale.z),
        ] {
            theme::axis_label(ui, axis);
            let resp = ui.add_sized(
                egui::vec2(axis_w, theme::FIELD_HEIGHT),
                egui::DragValue::new(value)
                    .speed(0.05)
                    .range(0.01..=f32::MAX),
            );
            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
        }
    });

    // 世界坐标用小一号的等宽字：它是只读参考，不该和可编辑控件抢注意力。
    // 用空标签的 `field_row` 占位，值才能落在与上面三行同一条竖线上。
    row(ui, "", |ui| {
        ui.add_space(1.0);
        ui.label(
            egui::RichText::new(format!(
                "World  {:.2},  {:.2},  {:.2}",
                world_position.x, world_position.y, world_position.z
            ))
            .monospace()
            .size(10.5)
            .color(theme::colors::TEXT_DIM),
        );
    });

    // 三个布尔是 `show_body` 时代的老写法：控件几十个，逐个改 `cx.sig` 更容易漏。
    // 统一在收尾处折进 `FieldSignals`，与 `Fields` 那条路走同一套脏检查。
    cx.sig.collect_changed_any(touched);
    if drag_began {
        cx.sig.mark_drag_began();
    }
    if drag_ended {
        cx.sig.mark_drag_ended();
    }
}

/// Mesh 段。
pub(super) fn show_mesh(ui: &mut egui::Ui, entity: &mut Entity, cx: &mut SectionCx<'_>) {
    #[allow(unused_mut)]
    let mut touched = false;
    #[allow(unused_mut)]
    let mut drag_began = false;
    #[allow(unused_mut)]
    let mut drag_ended = false;
    row(ui, "Model", |ui| {
        let mut path = entity.model.clone();
        let resp = ui.add_sized(
            egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
            egui::TextEdit::singleline(&mut path)
                .hint_text("Resources/… 或 builtin://cube"),
        );
        if resp.changed() && path != entity.model {
            entity.model = path;
            touched = true;
        }
    });

    // 内置图元：下拉选择（此前是四个并排 chip，"Cylinder" 会被面板边缘裁掉）
    row(ui, "Primitive", |ui| {
        let current = xgame_render::mesh::BuiltinMesh::ALL
            .iter()
            .find(|kind| entity.model == kind.uri())
            .copied();
        let text = current
            .map(|kind| kind.label().to_owned())
            .unwrap_or_else(|| "Custom".to_owned());
        egui::ComboBox::from_id_salt("inspector_primitive")
            .selected_text(text)
            // 与上面 Model 那行同宽（而不是固定 140）：两行右边缘对齐，
            // 否则 140 的框在满宽输入框下面缩进去一截，像没画完。
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for kind in xgame_render::mesh::BuiltinMesh::ALL {
                    let active = entity.model == kind.uri();
                    if ui.selectable_label(active, kind.label()).clicked() {
                        entity.model = kind.uri();
                        touched = true;
                    }
                }
            });
    });

    let (morph_names, morph_count) = cx.state
        .scene_renderer
        .as_ref()
        .map(|renderer| {
            entity
                .model_at(0)
                .map_or_else(Default::default, |model| renderer.morph_targets(model))
        })
        .unwrap_or_default();
    if morph_count > 0 {
        ui.label(theme::dim_note("Morph Targets"));
        for index in 0..morph_count {
            let label = morph_names
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("Target {index}"));
            let mut value = entity.morph_weights.get(index).copied().unwrap_or(0.0);
            theme::field_row(ui, &label, |ui| {
                let response = ui.add_sized(
                    egui::vec2(ui.available_width(), theme::FIELD_HEIGHT),
                    egui::Slider::new(&mut value, 0.0..=1.0),
                );
                if response.changed() {
                    // 只在真的拖了才扩容；打开面板不能把场景改脏。
                    if entity.morph_weights.len() <= index {
                        entity.morph_weights.resize(index + 1, 0.0);
                    }
                    entity.morph_weights[index] = value.clamp(0.0, 1.0);
                    touched = true;
                }
                response.on_hover_text(
                    "形变目标权重（0 = 基准形状，1 = 完全变成这个目标）",
                );
            });
        }
    }

    // 三个布尔是 `show_body` 时代的老写法：控件几十个，逐个改 `cx.sig` 更容易漏。
    // 统一在收尾处折进 `FieldSignals`，与 `Fields` 那条路走同一套脏检查。
    cx.sig.collect_changed_any(touched);
    if drag_began {
        cx.sig.mark_drag_began();
    }
    if drag_ended {
        cx.sig.mark_drag_ended();
    }
}

/// Animation 段。
pub(super) fn show_animation(ui: &mut egui::Ui, entity: &mut Entity, cx: &mut SectionCx<'_>) {
    let mut touched = false;
    let mut drag_began = false;
    let mut drag_ended = false;
    let clips = cx.state
        .scene_renderer
        .as_ref()
        .map(|r| entity.model_at(0).map_or_else(Vec::new, |m| r.animation_clip_names(m)))
        .unwrap_or_default();
    // 骨骼名：层栈遮罩的下拉来源（去重、跳过匿名；模型未加载 → 空）。
    let bone_names = cx.state
        .scene_renderer
        .as_ref()
        .map(|r| entity.model_at(0).map_or_else(Vec::new, |m| r.bone_names(m)))
        .unwrap_or_default();
    let model_empty = entity.model.is_empty();

    let mut remove = false;
        let mut remove_machine = false;
        // 事件时间轴的边界要用剪辑时长：模型名先取出来，下面
        // `entity.animator` 被可变借走之后就取不到 `entity` 了。
        let anim_model = entity.model_at(0).map(|model| model.to_owned());
        if let Some(animator) = entity.animator.as_mut() {
            // 根运动属于**基础通道**，放在机器 / 叠加 / 层栈之前。层栈可以
            // 任意增长，若把它放在末尾，窄窗口里即使开着也要滚过所有层才
            // 找得到；同时这也让“只从基础通道提取”在界面层级上一眼可见。
            let mut root_motion_enabled = animator.root_motion.is_some();
            if ui
                .checkbox(&mut root_motion_enabled, "Root Motion")
                .on_hover_text(
                    "从基础动画通道提取指定节点的逐帧平移，\n\
                     Play / 独立游戏 / 无头运行都由 runtime 移动实体",
                )
                .changed()
            {
                animator.root_motion = root_motion_enabled
                    .then(xgame_core::RootMotion::default);
                touched = true;
            }
            if let Some(root_motion) = animator.root_motion.as_mut() {
                row(ui, "Node", |ui| {
                    let selected = if root_motion.node.is_empty() {
                        "（选择节点）"
                    } else {
                        root_motion.node.as_str()
                    };
                    egui::ComboBox::from_id_salt("root_motion_node")
                        .width(ui.available_width())
                        .selected_text(
                            egui::RichText::new(selected).monospace().size(11.0),
                        )
                        .show_ui(ui, |ui| {
                            if bone_names.is_empty() {
                                ui.label(theme::dim_note(if model_empty {
                                    "没有模型"
                                } else {
                                    "模型未加载或没有命名节点"
                                }));
                            }
                            for node in &bone_names {
                                if ui
                                    .selectable_label(
                                        root_motion.node == *node,
                                        node.as_str(),
                                    )
                                    .clicked()
                                {
                                    root_motion.node.clone_from(node);
                                    touched = true;
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "显式选择 glTF 节点；通常是髋骨。\n猜场景根会因导出器不同而漂移",
                        );
                });
                row(ui, "Axes", |ui| {
                    for (index, axis) in ["X", "Y", "Z"].into_iter().enumerate() {
                        if ui
                            .checkbox(&mut root_motion.axes[index], axis)
                            .on_hover_text("勾选后由 runtime 消费该轴位移")
                            .changed()
                        {
                            touched = true;
                        }
                    }
                });
                if entity.controller.is_none()
                    && entity.physics.is_some_and(|body| {
                        body.kind != xgame_core::BodyKind::Kinematic
                    })
                {
                    theme::note(
                        ui,
                        "动态 / 静态刚体的位置归物理所有，根运动不会生效",
                    );
                } else if root_motion.node.is_empty() {
                    theme::note(ui, "请选择一个有平移轨道的节点");
                }
            }

            // 机器模式：clip/speed/looping 由状态表决定，这三行换成
            // 机器信息 + 参数调节；Fade/Autoplay 两行仍生效、照常显示。
            //
            // 坏机器（entry 指不到状态）不算机器模式 —— 走单剪辑 UI，
            // 用户还能改 clip / 转状态机自救，不会被一张死表锁死。
            let machine_usable =
                animator.machine.as_ref().is_some_and(|m| m.is_usable());
            if machine_usable {
                let current = match cx.state.game_runtime.as_ref() {
                    Some(runtime) => runtime.animation().state(entity.id),
                    None => cx.state.scene_renderer.as_ref().and_then(|renderer| {
                        renderer.edit_animation_state(entity.id)
                    }),
                };
                let entry_name = animator
                    .machine
                    .as_ref()
                    .map(|m| m.entry.clone())
                    .unwrap_or_default();
                row(ui, "Machine", |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(entry_name.as_str())
                                .monospace()
                                .size(11.0),
                        )
                        .truncate(),
                    )
                    .on_hover_text(
                        "动画状态机：播哪个剪辑由状态表+参数决定\n\
                         （states / transitions 在场景 RON 里编辑）",
                    );
                    if let Some(cur) = current {
                        ui.label(
                            egui::RichText::new("→")
                                .size(11.0)
                                .color(theme::colors::TEXT_DIM),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(cur)
                                    .monospace()
                                    .size(11.0)
                                    .color(theme::colors::TEXT_SECONDARY),
                            )
                            .truncate(),
                        );
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if icons::close_button(
                                ui,
                                "移除状态机，回到单剪辑模式（clip 字段还在）",
                            ) {
                                remove_machine = true;
                            }
                        },
                    );
                });
                // 参数行：脚本/这里都能改，值随场景落盘。
                if let Some(machine) = animator.machine.as_mut() {
                    if machine.parameters.is_empty() {
                        ui.label(theme::dim_note(
                            "没有参数（转移条件永远不满足，停在 entry）",
                        ));
                    }
                    for (name, value) in machine.parameters.iter_mut() {
                        row(ui, name.as_str(), |ui| {
                            let resp = ui.add_sized(
                                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
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
                ui.label(theme::dim_note("states / transitions 在场景 RON 里编辑"));
            } else {
                // 剪辑下拉：菜单一打开就把当前项勾出来，选中新项才算改动。
                row(ui, "Clip", |ui| {
                    let current = animator.clip.clone();
                    let label = if current.is_empty() {
                        "（未选剪辑）".to_string()
                    } else {
                        current.clone()
                    };
                    egui::ComboBox::from_id_salt("anim_clip")
                        .width(ui.available_width() - 20.0)
                        .selected_text(egui::RichText::new(label).monospace().size(11.0))
                        .show_ui(ui, |ui| {
                            if clips.is_empty() {
                                ui.label(theme::dim_note(if model_empty {
                                    "没有模型"
                                } else {
                                    "模型未加载或没有动画剪辑"
                                }));
                            }
                            for clip in clips.iter() {
                                if ui
                                    .selectable_label(
                                        clip == &current,
                                        clip.as_str(),
                                    )
                                    .clicked()
                                {
                                    animator.clip = clip.clone();
                                    touched = true;
                                }
                            }
                        });
                    let close = ui.add_enabled_ui(cx.state.is_editing(), |ui| {
                        icons::close_button(
                            ui,
                            "摘掉动画组件（模型与剪辑都不动）",
                        )
                    });
                    close.response.on_disabled_hover_text(PLAY_STRUCTURE_HINT);
                    if close.inner {
                        remove = true;
                    }
                });

                row(ui, "Speed", |ui| {
                    let mut speed = animator.speed;
                    let resp = ui
                        .add_sized(
                            egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                            egui::DragValue::new(&mut speed).speed(0.05).suffix("x"),
                        )
                        .on_hover_text("播放速度倍率；0 = 冻结在当前帧");
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    if resp.changed() {
                        animator.speed = speed.clamp(0.0, 10.0);
                    }
                });

                if ui.checkbox(&mut animator.looping, "Loop").changed() {
                    touched = true;
                }

                // 转为状态机：用当前 clip/speed/looping 建一个单状态机器，
                // entry 就是它 —— 之后在 RON 里加状态和转移即可。
                if icons::text_button(
                    ui,
                    "转为状态机（单状态）",
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

            // ── 动画事件 ──
            //
            // 事件表跟**实体**走（剪辑全部来自 glTF，没有能写事件的
            // 剪辑资产），时间按基础通道当前剪辑的本地时间轴解释。
            // 只在单剪辑模式下给"越界"提示：机器模式下播的是状态表
            // 里的剪辑，拿 entry 的时长去判断会误导。
            // 表格顺序不影响派发（运行时按命中时刻排序），所以这里
            // 不排序 —— 拖动时间时也就不会让行在指尖下跳来跳去。
            if icons::add_row(
                ui,
                "Add Event",
                "在基础通道的剪辑时间轴上打一个命名事件：\n\
                 到点时脚本 on_anim_event(ctx, name) 与组件的\n\
                 Lifecycle::on_anim_event 各自收到一次。",
            )
            .clicked()
            {
                animator.events.push(xgame_core::AnimEvent {
                    time: 0.0,
                    name: "event".to_string(),
                });
                touched = true;
            }
            if !animator.events.is_empty() {
                let clip_len = if animator.active_machine().is_some() {
                    None
                } else {
                    anim_model.as_deref().and_then(|model| {
                        cx.state.scene_renderer.as_ref().and_then(|renderer| {
                            renderer.animation_clip_duration(model, &animator.clip)
                        })
                    })
                };
                let mut remove_event = None;
                for idx in 0..animator.events.len() {
                    let beyond =
                        clip_len.is_some_and(|len| animator.events[idx].time > len);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if icons::close_button(ui, "删掉这个事件") {
                            remove_event = Some(idx);
                        }
                        if beyond {
                            // 越界时刻不会触发（运行时不猜"取模"），
                            // 用错误色提示，但不拦编辑。
                            ui.visuals_mut().override_text_color =
                                Some(theme::colors::ERROR);
                        }
                        let event = &mut animator.events[idx];
                        let mut time = event.time;
                        let resp = ui
                            .add_sized(
                                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                                egui::DragValue::new(&mut time)
                                    .speed(0.01)
                                    .range(0.0..=600.0)
                                    .suffix("s"),
                            )
                            .on_hover_text(match clip_len {
                                Some(len) => format!(
                                    "触发时刻（秒，剪辑时长 {len:.2}s）；超过时长的事件永不派发"
                                ),
                                None => "触发时刻（秒，从剪辑 0 起算）".to_owned(),
                            });
                        if resp.changed() {
                            event.time = time;
                            touched = true;
                        }
                        // 名字宽度跟着剩余空间走：✕16 + 时间 54 + 间距
                        // 之后剩下的都给它，窄窗口也不会把名字挤出面板。
                        let mut name = event.name.clone();
                        let resp = ui
                            .add_sized(
                                egui::vec2(
                                    ui.available_width().max(40.0),
                                    theme::FIELD_HEIGHT,
                                ),
                                egui::TextEdit::singleline(&mut name)
                                    .hint_text("name")
                                    .font(egui::TextStyle::Monospace),
                            )
                            .on_hover_text(
                                "事件名：脚本 on_anim_event(ctx, name) 收到的字符串",
                            );
                        if resp.changed() {
                            event.name = name;
                            touched = true;
                        }
                    });
                }
                if let Some(index) = remove_event {
                    animator.events.remove(index);
                    touched = true;
                }
            }

            // 叠加层：与基础层正交 —— 机器 / 单剪辑模式都显示这一行。
            if animator.additive.is_none() {
                if icons::add_row(
                    ui,
                    "Additive Layer",
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
                row(ui, "Additive", |ui| {
                    if let Some(add) = animator.additive.as_mut() {
                        // 剪辑下拉：与基础剪辑同一来源（模型的剪辑列表）。
                        let current = add.clip.clone();
                        egui::ComboBox::from_id_salt("anim_additive_clip")
                            .width(84.0)
                            .selected_text(
                                egui::RichText::new(if current.is_empty() {
                                    "（选剪辑）"
                                } else {
                                    current.as_str()
                                })
                                .monospace()
                                .size(11.0),
                            )
                            .show_ui(ui, |ui| {
                                if clips.is_empty() {
                                    ui.label(theme::dim_note(if model_empty {
                                        "没有模型"
                                    } else {
                                        "模型未加载或没有动画剪辑"
                                    }));
                                }
                                for clip in clips.iter() {
                                    if ui
                                        .selectable_label(
                                            clip == &current,
                                            clip.as_str(),
                                        )
                                        .clicked()
                                    {
                                        add.clip = clip.clone();
                                        touched = true;
                                    }
                                }
                            });
                        // 权重：与脚本 set_anim_additive_weight 同源（都写这个字段）。
                        let resp = ui
                            .add_sized(
                                egui::vec2(48.0, theme::FIELD_HEIGHT),
                                egui::DragValue::new(&mut add.weight)
                                    .speed(0.01)
                                    .range(0.0..=1.0),
                            )
                            .on_hover_text(
                                "叠加层权重：0 = 完全不影响基础姿势，1 = 全量叠加",
                            );
                        collect(
                            &resp,
                            &mut touched,
                            &mut drag_began,
                            &mut drag_ended,
                        );
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if icons::close_button(ui, "摘掉叠加层（基础层不动）") {
                                remove_additive = true;
                            }
                        },
                    );
                });
                if remove_additive {
                    animator.additive = None;
                    touched = true;
                }
            }

            // 混合层栈：与 additive 快捷字段正交 —— additive 先求值，
            // 这里按数组顺序依次叠上（后层盖前层）。增删在闭包外收集、
            // 循环后统一应用（闭包里正借着 animator 的可变引用）。
            let mut remove_layer: Option<usize> = None;
            // 层栈不做列头：一行五个控件里 ComboBox 自带内边距
            // （请求 54 实得 59），列头若按请求值排布会逐列累积偏移，
            // 到 "Weight" 已经错开一个字母 —— 一列对不齐的表头比没有
            // 表头更显乱。改成每个控件一条悬停说明，信息一样给全。
            for idx in 0..animator.layers.len() {
                let layer_current_state = match cx.state.game_runtime.as_ref() {
                    Some(runtime) => runtime
                        .animation()
                        .layer_state(entity.id, &animator.layers[idx].name),
                    None => cx.state.scene_renderer.as_ref().and_then(|renderer| {
                        renderer.edit_animation_layer_state(
                            entity.id,
                            &animator.layers[idx].name,
                        )
                    }),
                };
                // ── 第一行：摘除 / 名字 / 模式 / 剪辑 / 权重 ──
                //
                // 一行要塞五个控件，宽度是硬约束（面板 ~284px）：
                // ✕16 + 名字 52 + 模式 54 + 剪辑 66 + 权重 44 + 4×5 间距
                // = 252 —— 再宽就会把权重挤出面板。
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let layer = &mut animator.layers[idx];
                    if icons::close_button(ui, "摘掉这一层") {
                        remove_layer = Some(idx);
                    }
                    // 层名：脚本 set_anim_layer_weight 按它查找。
                    let mut name = layer.name.clone();
                    let resp = ui
                        .add_sized(
                            egui::vec2(52.0, theme::FIELD_HEIGHT),
                            egui::TextEdit::singleline(&mut name)
                                .hint_text("name")
                                .font(egui::TextStyle::Monospace),
                        )
                        .on_hover_text(
                            "层名：脚本 set_anim_layer_weight(ctx, 名字, 权重) 按它找这一层",
                        );
                    if resp.changed() {
                        layer.name = name;
                        touched = true;
                    }
                    // 模式下拉
                    egui::ComboBox::from_id_salt(("layer_mode", idx))
                        .width(54.0)
                        .selected_text(match layer.mode {
                            xgame_core::AnimLayerMode::Additive => "Add",
                            xgame_core::AnimLayerMode::Override => "Over",
                        })
                        .show_ui(ui, |ui| {
                            let variants = [
                                (
                                    xgame_core::AnimLayerMode::Additive,
                                    "Additive（加增量）",
                                ),
                                (
                                    xgame_core::AnimLayerMode::Override,
                                    "Override（覆盖）",
                                ),
                            ];
                            for (mode, label) in variants {
                                if ui
                                    .selectable_label(layer.mode == mode, label)
                                    .clicked()
                                {
                                    layer.mode = mode;
                                    touched = true;
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "Additive = 把这一层的增量加上去；\nOverride = 用这一层的姿势覆盖（可配骨骼遮罩）",
                        );
                    // 剪辑下拉：与基础剪辑同一来源。
                    let current = layer.clip.clone();
                    egui::ComboBox::from_id_salt(("layer_clip", idx))
                        .width(66.0)
                        .selected_text(
                            egui::RichText::new(if current.is_empty() {
                                "—"
                            } else {
                                current.as_str()
                            })
                            .monospace()
                            .size(10.5),
                        )
                        .show_ui(ui, |ui| {
                            if clips.is_empty() {
                                ui.label(theme::dim_note(if model_empty {
                                    "没有模型"
                                } else {
                                    "模型未加载或没有动画剪辑"
                                }));
                            }
                            for clip in clips.iter() {
                                if ui
                                    .selectable_label(clip == &current, clip.as_str())
                                    .clicked()
                                {
                                    layer.clip = clip.clone();
                                    touched = true;
                                }
                            }
                        })
                        .response
                        .on_hover_text("这一层播的剪辑（与基础层同一来源）");
                    // 权重：与脚本 set_anim_layer_weight 同源（都写这个字段）。
                    let resp = ui
                        .add_sized(
                            egui::vec2(44.0, theme::FIELD_HEIGHT),
                            egui::DragValue::new(&mut layer.weight)
                                .speed(0.01)
                                .range(0.0..=1.0),
                        )
                        .on_hover_text("层权重：0 = 不生效，1 = 全量");
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });
                // ── 第二行：骨骼遮罩 chips + 加骨骼 ──
                ui.horizontal(|ui| {
                    let layer = &mut animator.layers[idx];
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        egui::RichText::new("Mask")
                            .size(10.5)
                            .color(theme::colors::TEXT_DIM),
                    )
                    .on_hover_text(
                        "子树根名单：名单里的骨及其全部子孙被这层作用；\n\
                         留空 = 整副骨架。glTF 匿名节点匹配不到，\n\
                         只能靠有名字的祖先进名单。",
                    );
                    if layer.mask.is_empty() {
                        ui.label(theme::dim_note("整副骨架"));
                    }
                    let mut drop_mask: Option<usize> = None;
                    for (mi, m) in layer.mask.iter().enumerate() {
                        if icons::chip(ui, m, "点一下把这根骨头移出遮罩") {
                            drop_mask = Some(mi);
                        }
                    }
                    if let Some(mi) = drop_mask {
                        layer.mask.remove(mi);
                        touched = true;
                    }
                    let bone_id = egui::Id::new(("layer_bone_menu", idx));
                    let add_bone =
                        icons::text_button(ui, "＋", "往遮罩里加一根骨骼");
                    if add_bone.clicked() {
                        ui.memory_mut(|m| m.toggle_popup(bone_id));
                    }
                    egui::popup_below_widget(
                        ui,
                        bone_id,
                        &add_bone,
                        egui::PopupCloseBehavior::CloseOnClick,
                        |ui| {
                            ui.set_min_width(170.0);
                            ui.set_max_height(260.0);
                            if bone_names.is_empty() {
                                ui.label(theme::dim_note(if model_empty {
                                    "没有模型"
                                } else {
                                    "模型未加载"
                                }));
                            }
                            egui::ScrollArea::vertical()
                                .max_height(240.0)
                                .show(ui, |ui| {
                                    for b in bone_names.iter() {
                                        if layer.mask.contains(b) {
                                            continue;
                                        }
                                        if ui.button(b.as_str()).clicked() {
                                            layer.mask.push(b.clone());
                                            touched = true;
                                        }
                                    }
                                });
                        },
                    );
                });
                // ── 第三行：本层状态机参数 ──
                //
                // 状态图结构仍在 RON 编辑；Inspector 负责把最常用的运行期
                // 观察/调参闭环补上，和基础 machine 的 UI 同一层级。
                if let Some(machine) = animator.layers[idx]
                    .machine
                    .as_mut()
                    .filter(|machine| machine.is_usable())
                {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Machine")
                                .color(theme::colors::TEXT_DIM),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(machine.entry.as_str())
                                    .monospace(),
                            )
                            .truncate(),
                        );
                        if let Some(current) = layer_current_state {
                            ui.label(
                                egui::RichText::new(format!("→ {current}"))
                                    .monospace()
                                    .color(theme::colors::TEXT_SECONDARY),
                            );
                        }
                    });
                    for (name, value) in machine.parameters.iter_mut() {
                        row(ui, name.as_str(), |ui| {
                            let response = theme::drag_field_w(
                                ui,
                                theme::FIELD_WIDTH,
                                value,
                                0.05,
                                "",
                            )
                            .on_hover_text(
                                "本层状态机参数；脚本 set_anim_layer_param 写同一份值",
                            );
                            collect(
                                &response,
                                &mut touched,
                                &mut drag_began,
                                &mut drag_ended,
                            );
                        });
                    }
                }
            }
            if let Some(idx) = remove_layer {
                animator.layers.remove(idx);
                touched = true;
            }
            if icons::add_row(
                ui,
                "Add Layer",
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

            row(ui, "Fade", |ui| {
                let mut fade = animator.fade_time;
                let resp = ui
                    .add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut fade)
                            .speed(0.01)
                            .range(0.0..=2.0)
                            .suffix("s"),
                    )
                    .on_hover_text("换剪辑/换状态的交叉淡入时长；0 = 硬切");
                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                if resp.changed() {
                    animator.fade_time = fade.clamp(0.0, 2.0);
                }
            });

            if ui
                .checkbox(&mut animator.autoplay, "Autoplay")
                .on_hover_text("关掉后停在剪辑第 0 帧（摆好第一帧再播）")
                .changed()
            {
                touched = true;
            }

        }
        if remove_machine {
            if let Some(animator) = entity.animator.as_mut() {
                animator.machine = None;
            }
            touched = true;
        }
    if remove {
        *cx.request = Some(ComponentAction::Remove(
            ComponentAddress::Builtin(BuiltinComponent::Animator),
        ));
    }

    // 三个布尔是 `show_body` 时代的老写法：控件几十个，逐个改 `cx.sig` 更容易漏。
    // 统一在收尾处折进 `FieldSignals`，与 `Fields` 那条路走同一套脏检查。
    cx.sig.collect_changed_any(touched);
    if drag_began {
        cx.sig.mark_drag_began();
    }
    if drag_ended {
        cx.sig.mark_drag_ended();
    }
}

/// Scripts 段。
pub(super) fn show_scripts(ui: &mut egui::Ui, entity: &mut Entity, cx: &mut SectionCx<'_>) {
    #[allow(unused_mut)]
    let mut touched = false;
    #[allow(unused_mut)]
    let mut drag_began = false;
    #[allow(unused_mut)]
    let mut drag_ended = false;
    let mut remove: Option<usize> = None;
    for (slot, path) in entity.scripts.iter().enumerate() {
        let row_response = ui.horizontal(|ui| {
            icons::small_icon(
                ui,
                icons::Icon::Script,
                12.0,
                theme::colors::BADGE_SCRIPT,
            );
            ui.add(
                egui::Label::new(egui::RichText::new(path).monospace().size(11.0))
                    .truncate(),
            )
            .on_hover_text("项目相对路径；顺序即执行顺序");
            // 行尾右对齐：`with_layout` 把剩余宽度全吃掉，✕ 贴右边对齐，
            // 多个脚本行之间因此不会因为路径长短而左右乱跳。
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = ui.add_enabled_ui(cx.state.is_editing(), |ui| {
                    icons::close_button(
                        ui,
                        "把这个脚本从实体上摘掉（脚本文件本身不动）",
                    )
                });
                close.response.on_disabled_hover_text(PLAY_STRUCTURE_HINT);
                if close.inner {
                    remove = Some(slot);
                }
            });
        }).response.interact(egui::Sense::click());
        component_context_menu(
            &row_response,
            cx.state,
            entity,
            ComponentMenuTarget::Removable {
                address: ComponentAddress::Lua(slot),
                label: "Remove Lua Script",
            },
            cx.request,
            cx.refresh_scripts,
        );
    }
    if let Some(slot) = remove {
        *cx.request = Some(ComponentAction::Remove(ComponentAddress::Lua(slot)));
    }

    theme::note(ui, "钩子: start / update / fixed_update / on_destroy");

    // 三个布尔是 `show_body` 时代的老写法：控件几十个，逐个改 `cx.sig` 更容易漏。
    // 统一在收尾处折进 `FieldSignals`，与 `Fields` 那条路走同一套脏检查。
    cx.sig.collect_changed_any(touched);
    if drag_began {
        cx.sig.mark_drag_began();
    }
    if drag_ended {
        cx.sig.mark_drag_ended();
    }
}

/// Behaviors 段。
pub(super) fn show_behaviors(ui: &mut egui::Ui, entity: &mut Entity, cx: &mut SectionCx<'_>) {
    let mut touched = false;
    let mut drag_began = false;
    let mut drag_ended = false;
    let names: Vec<String> = cx.state.behavior_names().to_vec();
    let mut remove: Option<usize> = None;
    // 本帧被改动的组件参数（P1.5）：先攒着，出了 `entity.behaviors` 的
    // 迭代再写 —— 循环还借着 `entity` 的不可变借用，循环里直接写
    // `entity.behavior_params` 会撞可变借用。按**名**记而不是按下标：
    // 同一帧点 ✕ 删除会让下标错位，名字不会。
    let mut pending_params: Option<(String, xgame_core::inspect::FieldValues)> = None;
    let entity_id = entity.id;
    for (slot, name) in entity.behaviors.iter().enumerate() {
        let registered = names.iter().any(|n| n == name);
        let row_response = ui.horizontal(|ui| {
            icons::small_icon(
                ui,
                icons::Icon::Component,
                12.0,
                if registered {
                    theme::colors::BADGE_PREFAB
                } else {
                    theme::colors::WARN
                },
            );
            let text = egui::RichText::new(name.clone()).monospace().size(11.0);
            if registered {
                // 悬停先把"有哪些参数"说清楚 —— 展开之前就知道值不值得
                // 展开，也省得对着一个没参数的组件找为什么是空的。
                // 查的是常驻缓存的字段表，不构造实例、不重跑插件入口。
                // `is_empty` 的判断放进分支体而不是 match guard：
                // `Some(t) if t.is_empty()` 这种写法 clippy 会报
                // redundant_guard（空切片本可以直接写成 `Some([])`）。
                let tip = match cx.state.behavior_field_table(name) {
                    None => "已注册的 Rust 组件；顺序即执行顺序".to_owned(),
                    Some(table) => {
                        let params: Vec<&str> =
                            table.iter().map(|f| f.display_label()).collect();
                        if params.is_empty() {
                            "已注册的 Rust 组件；顺序即执行顺序；没有可调参数"
                                .to_owned()
                        } else {
                            format!(
                                "已注册的 Rust 组件；顺序即执行顺序\n\
                             可调参数（{} 个）：{}",
                                params.len(),
                                params.join("、")
                            )
                        }
                    }
                };
                ui.label(text).on_hover_text(tip);
            } else {
                // 未注册不是错误（用户可能先在场景里写好、再按 Play 编译），
                // 但它**必须可见** —— 否则就是"挂了个组件却纹丝不动"。
                icons::small_icon(ui, icons::Icon::Warning, 12.0, theme::colors::WARN)
                    .on_hover_text("未注册：插件还没编译，或名字写错了");
                ui.label(text.color(theme::colors::ERROR))
                    .on_hover_text("插件没编译过 / 组件名不在注册表里");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = ui.add_enabled_ui(cx.state.is_editing(), |ui| {
                    icons::close_button(ui, "把这个组件从实体上摘掉")
                });
                close.response.on_disabled_hover_text(PLAY_STRUCTURE_HINT);
                if close.inner {
                    remove = Some(slot);
                }
            });
        }).response.interact(egui::Sense::click());
        component_context_menu(
            &row_response,
            cx.state,
            entity,
            ComponentMenuTarget::Removable {
                address: ComponentAddress::Rust(slot),
                label: "Remove Rust Component",
            },
            cx.request,
            cx.refresh_scripts,
        );

        // ── 可调参数（P1）──
        //
        // 缩进一级挂在组件名下面：字段属于它上面那行的组件，不是实体的。
        // 未注册的不画 —— 注册表里没有它就取不到字段表，而"为什么是空的"
        // 上面那行 ⚠ 已经说清楚了。
        if registered {
            ui.indent(("behavior_fields", slot), |ui| {
                let edit = show_behavior_fields(cx.state, ui, entity_id, slot, name);
                // 撤销信号接到面板级两段式（外层变量，靠闭包捕获）：
                // 拖拽起点挂快照、松手入栈，离散改动按 touched 入栈。
                drag_began |= edit.sig.drag_began;
                drag_ended |= edit.sig.drag_ended;
                if let Some(values) = edit.values {
                    pending_params = Some((name.clone(), values));
                }
            });
        }
    }
    // 参数改动写进 `working`（P1.5）：与 Name / Transform 同一条通道，
    // 写回场景、脏检查、撤销都由 show_content 的既有逻辑统一处理。
    if let Some((name, values)) = pending_params.take() {
        if values.is_empty() {
            // 没有可检视字段的组件不值得在场景里留一张空表（纯噪音）。
            entity.behavior_params.remove(&name);
        } else {
            entity.behavior_params.insert(name, values);
        }
        touched = true;
    }
    if let Some(slot) = remove {
        *cx.request =
            Some(ComponentAction::Remove(ComponentAddress::Rust(slot)));
    }

    // 调参什么时候生效 —— 两种模式的差别就这一句话，说在最前面免得
    // 用户在 Edit 里拖了半天问"为什么没动"。
    if !entity.behaviors.is_empty() {
        let hint = if cx.state.is_editing() {
            "参数随场景存盘（Ctrl+Z 可撤销）；按 ▶ Play 后生效"
        } else {
            "热调参中 —— 改动立即生效，停止 Play 时丢弃"
        };
        theme::note(ui, hint);
    }

    // 三个布尔是 `show_body` 时代的老写法：控件几十个，逐个改 `cx.sig` 更容易漏。
    // 统一在收尾处折进 `FieldSignals`，与 `Fields` 那条路走同一套脏检查。
    cx.sig.collect_changed_any(touched);
    if drag_began {
        cx.sig.mark_drag_began();
    }
    if drag_ended {
        cx.sig.mark_drag_ended();
    }
}

/// Material 段。
pub(super) fn show_material(ui: &mut egui::Ui, entity: &mut Entity, cx: &mut SectionCx<'_>) {
    let mut touched = false;
    let mut drag_began = false;
    let mut drag_ended = false;
    let materials = cx.state.material_list();
    row(ui, "Asset", |ui| {
        let current = entity.material_asset.clone().unwrap_or_default();
        let text = if current.is_empty() {
            "None（模型自带）".to_string()
        } else {
            current
                .rsplit('/')
                .next()
                .unwrap_or(&current)
                .trim_end_matches(".mat")
                .to_string()
        };
        egui::ComboBox::from_id_salt("inspector_material_asset")
            .selected_text(text)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_empty(), "None（模型自带）")
                    .clicked()
                    && !current.is_empty()
                {
                    entity.material_asset = None;
                    touched = true;
                }
                for path in &materials {
                    if ui.selectable_label(&current == path, path).clicked() {
                        entity.material_asset = Some(path.clone());
                        touched = true;
                    }
                }
            });
    });
    if entity.material_asset.is_none() {
        theme::note(ui, "共享材质资产 · 在 Assets 面板新建 .mat 后可在此指派");
    } else {
        theme::note(ui, entity.material_asset.as_deref().unwrap_or_default());
    }

    let mut enabled = entity.material.is_some();
    if ui
        .checkbox(&mut enabled, "Override material params")
        .on_hover_text(
            "在内置白模 / glTF 材质之上逐字段覆盖 PBR 参数；\n只影响勾选的字段",
        )
        .changed()
    {
        entity.material = if enabled {
            Some(xgame_core::MaterialOverride::default())
        } else {
            None
        };
        touched = true;
    }

    if let Some(m) = entity.material.as_mut() {
        // 基础色乘子（线性 RGBA；a 一并相乘 = "整体透明一点"）
        //
        // 每组都是「标签列 + 勾选框 + 编辑控件」：勾选框贴控件列起始，
        // 颜色块/数值紧随其后。这样三行的勾选框在同一竖线上，扫一眼
        // 就知道哪些字段被覆盖了。
        row(ui, "Tint", |ui| {
            let mut has = m.base_color.is_some();
            if ui
                .checkbox(&mut has, "")
                .on_hover_text("与模型自带基础色**相乘**：白模直接上色，\n贴图模型调整体色调；a 控制整体透明度")
                .changed()
            {
                m.base_color = if has { Some([1.0; 4]) } else { None };
                touched = true;
            }
            if let Some(c) = m.base_color.as_mut() {
                let resp = ui.color_edit_button_rgba_unmultiplied(c);
                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
            }
        });

        row(ui, "Metallic", |ui| {
            let mut has = m.metallic.is_some();
            if ui
                .checkbox(&mut has, "")
                .on_hover_text("0 = 绝缘体，1 = 金属。金属没有漫反射，\n反射环境色（写覆盖值，不做相乘）")
                .changed()
            {
                m.metallic = if has { Some(0.0) } else { None };
                touched = true;
            }
            if let Some(v) = m.metallic.as_mut() {
                let resp = ui.add_sized(
                    egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                    egui::DragValue::new(v).speed(0.01).range(0.0..=1.0),
                );
                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
            }
        });

        row(ui, "Rough", |ui| {
            let mut has = m.roughness.is_some();
            if ui
                .checkbox(&mut has, "")
                .on_hover_text("粗糙度：0 = 镜面，1 = 完全漫反射。\n（写覆盖值，不做相乘）")
                .changed()
            {
                m.roughness = if has { Some(0.5) } else { None };
                touched = true;
            }
            if let Some(v) = m.roughness.as_mut() {
                let resp = ui.add_sized(
                    egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                    egui::DragValue::new(v).speed(0.01).range(0.0..=1.0),
                );
                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
            }
        });
    }

    // 三个布尔是 `show_body` 时代的老写法：控件几十个，逐个改 `cx.sig` 更容易漏。
    // 统一在收尾处折进 `FieldSignals`，与 `Fields` 那条路走同一套脏检查。
    cx.sig.collect_changed_any(touched);
    if drag_began {
        cx.sig.mark_drag_began();
    }
    if drag_ended {
        cx.sig.mark_drag_ended();
    }
}

/// Prefab 段（实例 → 资产的关系操作）。
///
/// 没有字段可编辑：这里全是 **Apply / Revert / Unpack** 这类动作。
/// 放在专用渲染器而不是元数据里，是因为元数据描述的是"值怎么改"，
/// 而这一段描述的是"这个实体与另一个文件的关系"。
pub(super) fn show_prefab(ui: &mut egui::Ui, _entity: &mut Entity, cx: &mut SectionCx<'_>) {
    let Some(index) = cx.state.selected_entity else {
        return;
    };
    let Some(link) = cx
        .state
        .scene
        .entities
        .get(index)
        .and_then(|e| e.prefab.clone())
    else {
        return;
    };
ui.indent("prefab", |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(&link.source)
                    .monospace()
                    .size(10.5)
                    .color(theme::colors::TEXT_DIM),
            )
            .truncate(),
        )
        .on_hover_text(format!("预制体资产：{}", link.source));

        if cx.state.is_prefab_root(index) {
            let editing = cx.state.is_editing();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(editing, egui::Button::new("Apply"))
                    .on_hover_text("把当前实例的改动写回预制体资产文件")
                    .clicked()
                {
                    cx.state.apply_prefab(index);
                }
                if ui
                    .add_enabled(editing, egui::Button::new("Revert"))
                    .on_hover_text("用资产覆盖回这个实例（保留它当前的位置）")
                    .clicked()
                {
                    cx.state.revert_prefab(index);
                }
            });
        } else {
            let root = cx.state.prefab_instance_root(index);
            let root_name = root.map(|r| cx.state.scene.entities[r].name.clone());
            ui.label(theme::dim_note(match &root_name {
                Some(n) => format!("实例内部节点（根: {n}）"),
                None => "实例内部节点".to_string(),
            }));
            if let Some(r) = root {
                if ui.button("Select Instance Root").clicked() {
                    cx.state.selected_entity = Some(r);
                }
            }
        }
    });
}
