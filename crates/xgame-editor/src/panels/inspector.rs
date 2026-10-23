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

use crate::panels::{icons, theme};
use crate::state::EditorState;
use xgame_core::{EntityId, FieldDesc, FieldKind, FieldValue, Inspect};
use xgame_math::glam::{Vec2, Vec3, Vec4};

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

/// 检查器里的**标量行**：定宽标签列 + 轴标占位 + 控件。
///
/// Transform 的三行是 `[标签][X][框]`，输入框从"轴标宽 + 间距"之后开始；
/// 别的行没有轴标，不补这一截的话两类行的输入框左边缘就差一个轴标的宽度
/// （15px），一列控件看着参差不齐。所有**非向量**行都走这个包装。
fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    theme::field_row(ui, label, |ui| {
        ui.add_space(theme::AXIS_TAG_WIDTH + ui.spacing().item_spacing.x);
        add(ui)
    })
}

// ---------------------------------------------------------------------------
// 组件字段（双轨架构 P1）
// ---------------------------------------------------------------------------

/// 数值标量的通用步长 / 灵敏度兜底。
///
/// 单独拎出来是因为 `#[inspect]` 不写 `speed` 时五个数值类型各要一个合理默认：
/// `f32` 拖一下想动 0.1，而 `u32` 拖一下动 0.1 只会让它**一格不动**
/// （向下取整）。分两个值省掉"为什么这个数框拖不动"的排查。
fn drag_speed(kind: FieldKind) -> f64 {
    match kind {
        FieldKind::I32 | FieldKind::I64 | FieldKind::U32 => 1.0,
        _ => 0.1,
    }
}

/// 把编辑器里拖出来的 `f32` 按字段类型写回成对应的 [`FieldValue`]。
///
/// 只在 `desc.kind` 是数值类型时调用（上面的 match 分支保证了这一点），
/// 其余类型走不到 —— 所以不是 `Option`：走到了就是编程错误，让
/// `U32` 那一臂兜底也比静默返回 `None` 让写入被吞掉好。
fn numeric_value(kind: FieldKind, v: f32) -> FieldValue {
    match kind {
        FieldKind::F32 => FieldValue::F32(v),
        FieldKind::F64 => FieldValue::F64(v as f64),
        FieldKind::I32 => FieldValue::I32(v as i32),
        FieldKind::I64 => FieldValue::I64(v as i64),
        FieldKind::U32 => FieldValue::U32(v.max(0.0) as u32),
        _ => FieldValue::F32(v),
    }
}

/// 一个字段控件产生的编辑信号（与 [`collect`] 同语义，聚合成结构体往下传）。
///
/// 单独成型是因为 P1.5 之后组件字段改动**进撤销栈、进场景文件**：需要在
/// "改动发生"与"写回场景"之间拿到 `changed` / 拖拽起点 / 拖拽终点三个时机，
/// 而一个字段一个 `bool` 传三遍的写法在有五种字段类型的地方很快失控。
#[derive(Default, Clone, Copy)]
struct FieldSignals {
    /// 本帧有字段被改动（对应 `Response::changed`）。
    changed: bool,
    /// 本帧有控件开始拖拽（改前快照的拍摄时机）。
    drag_began: bool,
    /// 本帧有控件结束拖拽（入撤销栈的时机）。
    drag_ended: bool,
}

impl FieldSignals {
    fn collect(&mut self, resp: &egui::Response) {
        self.changed |= resp.changed();
        self.drag_began |= resp.drag_started();
        self.drag_ended |= resp.drag_stopped();
    }
}

/// 组件字段一帧的编辑结果（P1.5）——信号交给面板级撤销逻辑，值交给 `working`。
#[derive(Default)]
struct BehaviorEdit {
    /// 三个撤销信号（与 [`FieldSignals`] 同语义）。
    sig: FieldSignals,
    /// `Some(新参数表)` = 本帧有字段被改动，调用方写进 `working.behavior_params`；
    /// `None` = 没动过 —— `working` 克隆自场景，那份本来就是对的，不必回写。
    ///
    /// **不直接写 `state.scene`**：组件参数从此与 Name / Transform 走**同一条**
    /// 通道（`working` → 撤销 → 写回场景），撤销与脏检查的时序不用为它单开
    /// 一条支路 —— 支路就是将来"调完参改个名字，参数被旧克隆盖掉"的出处。
    values: Option<xgame_core::inspect::FieldValues>,
}

/// 画一个组件的可检视字段，并把改动**写回组件本体**。
///
/// # 改的是哪一份
///
/// 由 [`EditorState::inspect_component`] 决定：Play 中是 World 里**跑着的**
/// 那份（拖一下，下一次 `update` 就生效），Edit 中是编辑态副本（按 ▶ Play 时
/// 灌进运行时）。两种模式下控件长得完全一样 —— 这是"一份元数据，两个消费者"
/// 在编辑器这一侧的体现：控件布局只由 `FieldDesc` 决定，跟实例活在哪无关。
///
/// # 参数怎么进场景文件（P1.5）
///
/// 返回的 [`BehaviorEdit`] 里带捕获好的参数表，调用方（Behaviors 区）把它写进
/// `working.behavior_params` 并置 `touched` —— 于是存盘、脏检查、撤销、打包玩家
/// 的手感参数全部走 `show_content` 既有的那套逻辑，没有第二条写回路径：
///
/// - **拖拽**类控件：`drag_began` 挂改前快照、`drag_ended` 入栈（拖 200 帧
///   不记 200 步），与 Transform 的滑杆完全同款；
/// - **离散**编辑（勾选框 / 文本）：由面板级逻辑按 `touched` 当场入栈，
///   那一刻 `state.scene` 里还是旧参数；
/// - **Play 中的热调参不写回**：那是"试试看"，停止 Play 靠场景快照整体还原
///   （与 Unity 默认一致），写回反而会让"Play 里试的值"泄漏进存盘文件。
///
/// # 取不到实例时
///
/// 直接返回什么都不画。Behaviors 列表那一行已经用 ⚠ 标红了未注册的组件名，
/// 再画一行"无法检视"是同一条信息说第二遍。
fn show_behavior_fields(
    state: &mut EditorState,
    ui: &mut egui::Ui,
    entity_id: EntityId,
    slot: usize,
    name: &str,
) -> BehaviorEdit {
    // 先取模式（下面 `comp` 借着 `&mut state`，届时再读就是 E0502）。
    let editing = state.is_editing();
    let Some((_, comp)) = state.inspect_component(entity_id, slot, name) else {
        return BehaviorEdit::default();
    };

    let count = comp.inspect_fields().len();
    if count == 0 {
        ui.label(
            egui::RichText::new("这个组件没有可调参数")
                .weak()
                .size(11.0),
        );
        return BehaviorEdit::default();
    }

    // 标签列宽固定，字段名长短不同也不会让右边的控件参差不齐。
    let label_w = 120.0;
    let mut sig = FieldSignals::default();
    for i in 0..count {
        // `desc` 是 `'static`（表是编译期常量），拿在手里**不占用** `comp` ——
        // 下一行还能接着 &mut 它，不需要先放开借用。
        let Some((desc, value)) = comp.inspect_field(i) else {
            continue;
        };

        ui.horizontal(|ui| {
            let label = egui::RichText::new(desc.display_label()).size(11.0);
            let text = ui.add_sized([label_w, ui.spacing().interact_size.y], egui::Label::new(label));
            if !desc.tooltip.is_empty() {
                text.on_hover_text(desc.tooltip);
            }
            show_field_widget(ui, comp, i, desc, value, &mut sig);
        });
    }

    // 只在 Edit 模式捕获（见文档"Play 中的热调参不写回"）。`comp` 的借用到此
    // 结束 —— `BehaviorEdit` 必须是拥有的值。
    let values = if editing && sig.changed {
        Some(xgame_core::inspect::capture_field_values(comp))
    } else {
        None
    };
    BehaviorEdit { sig, values }
}

/// 一个字段的控件。改动直接 `inspect_set` 回组件。
///
/// 每种 `FieldKind` 一个分支 —— 控件形状就是类型本身的一部分：
/// 有边界的数值给滑块（拖到哪算哪，不会拖出界），没有的给裸拖拽值
/// （`min/max` 缺省是常态，不强加一个作者没声明过的范围）。
///
/// 三个编辑信号经 `sig` 外传（见 [`FieldSignals`]）—— 控件的 `Response`
/// 是借用，出了这个函数就没了。
fn show_field_widget(
    ui: &mut egui::Ui,
    comp: &mut dyn Inspect,
    index: usize,
    desc: &FieldDesc,
    value: FieldValue,
    sig: &mut FieldSignals,
) {
    let unit = desc.unit;
    match desc.kind {
        FieldKind::F32 | FieldKind::F64 | FieldKind::I32 | FieldKind::I64 | FieldKind::U32 => {
            let mut v = value.as_f32().unwrap_or(0.0);
            let speed = desc.speed.map_or(drag_speed(desc.kind), |s| s as f64);
            let resp = if desc.has_range() {
                let lo = desc.min.unwrap_or(0.0);
                let hi = desc.max.unwrap_or(1.0).max(lo);
                // 滑块不做 `step_by`：边拖边吸附会让细调变成"一格一格跳"，
                // 而 `step` 的语义是"落点对齐"，松手后量化一次更符合直觉。
                ui.add(egui::Slider::new(&mut v, lo..=hi).suffix(unit))
            } else {
                ui.add(egui::DragValue::new(&mut v).speed(speed).suffix(unit))
            };
            sig.collect(&resp);
            if resp.changed() {
                // 离散步长：量化在**写回前**做，两种控件共用这一处。
                if let Some(step) = desc.step {
                    if step > 0.0 {
                        v = (v / step).round() * step;
                    }
                }
                comp.inspect_set(index, numeric_value(desc.kind, v));
            }
        }

        FieldKind::Bool => {
            let mut b = matches!(value, FieldValue::Bool(true));
            let resp = ui.checkbox(&mut b, "");
            sig.collect(&resp);
            if resp.changed() {
                comp.inspect_set(index, FieldValue::Bool(b));
            }
        }

        FieldKind::String => {
            let mut s = match value {
                FieldValue::String(s) => s,
                _ => String::new(),
            };
            let hint = if desc.tooltip.is_empty() {
                desc.display_label().to_owned()
            } else {
                desc.tooltip.to_owned()
            };
            let resp = ui
                .add(egui::TextEdit::singleline(&mut s).hint_text(hint).desired_width(140.0));
            sig.collect(&resp);
            if resp.changed() {
                comp.inspect_set(index, FieldValue::String(s));
            }
        }

        FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 => {
            // 拆成一个定长数组来编辑：三个类型共用同一套控件逻辑，
            // 只在"读出来 / 拼回去"两处分叉 —— 比给每个向量类型抄一遍短得多。
            let (mut arr, dims) = match value {
                FieldValue::Vec2(v) => ([v.x, v.y, 0.0, 0.0], 2usize),
                FieldValue::Vec3(v) => ([v.x, v.y, v.z, 0.0], 3),
                FieldValue::Vec4(v) => ([v.x, v.y, v.z, v.w], 4),
                // 与 `desc.kind` 对不上：不该发生（表与值同源），按 vec3 兜底。
                _ => ([0.0; 4], 3),
            };
            // 本字段自己的改动标记：不能直接用聚合后的 `sig.changed` 判定写回 ——
            // 那会让同一帧里**别的**字段的改动把这里一个没动过的值也写回去。
            let mut local = false;
            ui.horizontal(|ui| {
                for (axis, f) in ["X", "Y", "Z", "W"]
                    .iter()
                    .zip(arr.iter_mut().take(dims))
                {
                    ui.label(
                        egui::RichText::new(*axis)
                            .weak()
                            .size(10.0),
                    );
                    let speed = desc.speed.map_or(0.1, |s| s as f64);
                    let resp = ui.add(egui::DragValue::new(f).speed(speed));
                    sig.collect(&resp);
                    local |= resp.changed();
                }
            });
            if local {
                let out = match desc.kind {
                    FieldKind::Vec2 => FieldValue::Vec2(Vec2::new(arr[0], arr[1])),
                    FieldKind::Vec3 => FieldValue::Vec3(Vec3::new(arr[0], arr[1], arr[2])),
                    _ => FieldValue::Vec4(Vec4::new(arr[0], arr[1], arr[2], arr[3])),
                };
                comp.inspect_set(index, out);
            }
        }
    }
}

/// 在 `Ui` 内渲染检视面板内容。
/// Inspector 面板入口：内容一律包在一条竖向滚动区里。
///
/// 为什么必须有：一个挂满组件的实体（模型 + 动画 + 脚本 + 组件 + 物理 + 音频 +
/// 材质 + 父级 + 预制体）在 660px 高的面板里放不下，没有滚动区时**下面几段点不到**，
/// 而且看不出来是被截断还是根本没有 —— UE 的 Details 面板同样是整条滚动的。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    ui.scope(|ui| {
        // **分组内容不额外缩进**。面板里的 `ui.indent(...)` 只用来给代码分块，
        // 视觉上必须是零缩进：UE 的 Details 里分类条下的行与 Name 行共用同一条
        // 左边界，否则（indent = 10px）Name 的值在 x=64、Transform 的值在 x=74，
        // 整块看下来所有控件左右各错开一档 —— 这是"排版不整齐"最主要的一条。
        ui.spacing_mut().indent = 0.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| show_body(state, ui));
    });
}

fn show_body(state: &mut EditorState, ui: &mut egui::Ui) {
    // 控件全部作用在 `working`（选中实体的副本）上，`state.scene` 在下面整段 UI 里
    // 都不会被碰到 —— 这正是"改前快照"还能拿到的原因。
    let Some(index) = state.selected_entity else {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Select an entity in the Hierarchy")
                    .color(theme::colors::TEXT_SECONDARY),
            );
        });
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
        row(ui, "Name", |ui| {
            let resp = ui.add_sized(
                egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
                egui::TextEdit::singleline(&mut entity.name),
            );
            touched |= resp.changed();
        });

        // ── 层 ──
        //
        // 放在 Name 下面而不是塞进 Physics 区块：层是**实体属性**（Unity 的
        // Layer 也在 Inspector 顶部），纯逻辑实体（触发区、生成点）没有刚体
        // 也照样有层。名字来自 `project.ron` 的 `layers` 节，没起名的层显示
        // `N  Layer N` —— 只有层号可见才找得到自己想选的那层。
        let layer_names = state.project.manifest().layers.clone();
        row(ui, "Layer", |ui| {
            let current = xgame_core::sanitize_layer(entity.layer);
            egui::ComboBox::from_id_salt("inspector_layer")
                .selected_text(layer_names.label(current))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for layer in 0..xgame_core::LAYER_COUNT as u8 {
                        let active = layer == current;
                        if ui
                            .selectable_label(active, layer_names.label(layer))
                            .clicked()
                            && !active
                        {
                            entity.layer = layer;
                            touched = true;
                        }
                    }
                });
        });

        // ── 变换组件 ──
        theme::section_bar(ui, "Transform");
        ui.indent("transform", |ui| {
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
                // 顺序用 **YXZ**（不是 XYZ）：脚本 API 的 `rot` / `set_rot` 就是 YXZ，
                // 面板必须与它一致，否则"面板上读到 153°、脚本里读到 −130°"，
                // 作者会以为其中一处坏了。
                // 对相机这类物体，YXZ 还给出人类可读的 yaw / pitch（俯角 18°），
                // XYZ 分解在同样位姿下会折成 (−46°, 153°, 160°) 这种读不出来的三元组。
                let euler = entity
                    .transform
                    .rotation
                    .to_euler(xgame_math::glam::EulerRot::YXZ);
                // `to_euler` 按 (y, x, z) 序返回；面板按 X / Y / Z 排列显示。
                let mut rot = [
                    euler.1.to_degrees(),
                    euler.0.to_degrees(),
                    euler.2.to_degrees(),
                ];
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
                    entity.transform.rotation = xgame_math::glam::Quat::from_euler(
                        xgame_math::glam::EulerRot::YXZ,
                        rot[1].to_radians(),
                        rot[0].to_radians(),
                        rot[2].to_radians(),
                    );
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
        });

        // ── UI 元素（屏幕空间）──
        //
        // 紧挨着 Transform：UI 元素的"矩形"就是锚点 + 偏移，语义上正是
        // 这个实体的变换；放在面板末尾（Material 之后）时，属性多的实体
        // 要滚很久才能看到它 —— 实测截图里它整块在折叠线以下。
        crate::panels::inspector_ui::show(ui, entity, &mut touched);

        // ── 网格 ──
        theme::section_bar(ui, "Mesh");
        ui.indent("mesh", |ui| {
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
        });

        // ── 动画 ──
        //
        // 没有模型就没东西可播；但**已经有**动画组件时即使模型被清空也要显示，
        // 否则组件会被藏到永远摘不掉。剪辑列表来自渲染器的资产缓存 —— 模型还没
        // 加载时是空的，弱提示占位，下一帧预加载完成后自然有选项。
        if !entity.model.is_empty() || entity.animator.is_some() {
            theme::section_bar(ui, "Animation");
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
                    if icons::add_row(
                        ui,
                        "Add Animator",
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
                    // 提示必须**另起一行**：同一行里 `ui.label` 不会换行，
                    // 中文一旦比剩余宽度长就会顶出面板右边缘被裁掉。
                    if clips.is_empty() {
                        ui.label(theme::dim_note(if model_empty {
                            "没有模型"
                        } else {
                            "模型未加载或没有动画剪辑"
                        }));
                    }
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
                                if icons::close_button(ui, "摘掉动画组件（模型与剪辑都不动）") {
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
                        entity.animator = None;
                        touched = true;
                    }
                }
            });
        }


        // ── 脚本 ──
        //
        // 这里存的是**项目相对路径**，不是脚本内容：同一份脚本可以挂在多个实体上，
        // 改一次逻辑全都生效。列表顺序就是执行顺序。
        theme::section_bar(ui, "Scripts");
        ui.indent("scripts", |ui| {
            let mut remove: Option<usize> = None;
            for (slot, path) in entity.scripts.iter().enumerate() {
                ui.horizontal(|ui| {
                    icons::small_icon(ui, icons::Icon::Script, 12.0, theme::colors::BADGE_SCRIPT);
                    ui.add(
                        egui::Label::new(egui::RichText::new(path).monospace().size(11.0))
                            .truncate(),
                    )
                    .on_hover_text("项目相对路径；顺序即执行顺序");
                    // 行尾右对齐：`with_layout` 把剩余宽度全吃掉，✕ 贴右边对齐，
                    // 多个脚本行之间因此不会因为路径长短而左右乱跳。
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::close_button(ui, "把这个脚本从实体上摘掉（脚本文件本身不动）")
                        {
                            remove = Some(slot);
                        }
                    });
                });
            }
            if let Some(slot) = remove {
                entity.scripts.remove(slot);
                touched = true;
            }

            let add_id = egui::Id::new("inspector_add_script");
            let add = icons::add_row(ui, "Add Script", "从项目 Scripts/ 目录里挑一个 .lua 挂上");
            if add.clicked() {
                ui.memory_mut(|m| m.toggle_popup(add_id));
            }
            // 用 `popup_below_widget` 而不是 `menu_button`：后者自带边框和菜单
            // 分隔线的观感，跟这边的扁平行不搭，也没法做到 "整宽点击区"。
            egui::popup_below_widget(
                ui,
                add_id,
                &add,
                egui::PopupCloseBehavior::CloseOnClick,
                |ui| {
                    ui.set_min_width(220.0);
                    if state.script_list.is_empty() {
                        ui.label(theme::dim_note("项目里还没有脚本（.lua）"));
                    } else {
                        for path in state.script_list.clone() {
                            if entity.scripts.contains(&path) {
                                continue;
                            }
                            if ui.button(path.clone()).clicked() {
                                entity.scripts.push(path);
                                touched = true;
                            }
                        }
                        ui.separator();
                    }
                    if ui.button("重新扫描 Scripts/").clicked() {
                        state.refresh_scripts();
                    }
                },
            );

            theme::note(ui, "钩子: start / update / fixed_update / on_destroy");
        });


        // ── Behaviors（Rust 组件，双轨架构 P0.5）──
        //
        // 与上面的 Scripts 同构：存的是**组件名**而不是代码，顺序即执行顺序。
        // 区别在于这些名字来自插件 DLL 的注册表 —— 没构建过插件时是空的，
        // 场景里已挂的名字就会显示成红色 ⚠（名字写错 / 插件没编译，都该看见）。
        theme::section_bar(ui, "Behaviors");
        ui.indent("behaviors", |ui| {
            let names: Vec<String> = state.behavior_names().to_vec();
            let mut remove: Option<usize> = None;
            // 本帧被改动的组件参数（P1.5）：先攒着，出了 `entity.behaviors` 的
            // 迭代再写 —— 循环还借着 `entity` 的不可变借用，循环里直接写
            // `entity.behavior_params` 会撞可变借用。按**名**记而不是按下标：
            // 同一帧点 ✕ 删除会让下标错位，名字不会。
            let mut pending_params: Option<(String, xgame_core::inspect::FieldValues)> = None;
            let entity_id = entity.id;
            for (slot, name) in entity.behaviors.iter().enumerate() {
                let registered = names.iter().any(|n| n == name);
                ui.horizontal(|ui| {
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
                        let tip = match state.behavior_field_table(name) {
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
                        if icons::close_button(ui, "把这个组件从实体上摘掉") {
                            remove = Some(slot);
                        }
                    });
                });

                // ── 可调参数（P1）──
                //
                // 缩进一级挂在组件名下面：字段属于它上面那行的组件，不是实体的。
                // 未注册的不画 —— 注册表里没有它就取不到字段表，而"为什么是空的"
                // 上面那行 ⚠ 已经说清楚了。
                if registered {
                    ui.indent(("behavior_fields", slot), |ui| {
                        let edit = show_behavior_fields(state, ui, entity_id, slot, name);
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
                entity.behaviors.remove(slot);
                touched = true;
            }

            // 调参什么时候生效 —— 两种模式的差别就这一句话，说在最前面免得
            // 用户在 Edit 里拖了半天问"为什么没动"。
            if !entity.behaviors.is_empty() {
                let hint = if state.is_editing() {
                    "参数随场景存盘（Ctrl+Z 可撤销）；按 ▶ Play 后生效"
                } else {
                    "热调参中 —— 改动立即生效，停止 Play 时丢弃"
                };
                theme::note(ui, hint);
            }

            let add_id = egui::Id::new("inspector_add_behavior");
            let add = icons::add_row(
                ui,
                "Add Behavior",
                "从插件注册表里挑一个 Rust 组件挂上 —— 直接编译进宿主，没有解释器开销",
            );
            if add.clicked() {
                ui.memory_mut(|m| m.toggle_popup(add_id));
            }
            egui::popup_below_widget(
                ui,
                add_id,
                &add,
                egui::PopupCloseBehavior::CloseOnClick,
                |ui| {
                    ui.set_min_width(220.0);
                    if names.is_empty() {
                        ui.label(theme::dim_note("还没有已注册的 Rust 组件"));
                        theme::note(ui, "项目根放 Cargo.toml 后按 ▶ Play 会自动编译插件");
                    } else {
                        for name in names.iter() {
                            if entity.behaviors.contains(name) {
                                continue;
                            }
                            if ui.button(name.clone()).clicked() {
                                entity.behaviors.push(name.clone());
                                touched = true;
                            }
                        }
                    }
                },
            );

            if entity.behaviors.is_empty() {
                theme::note(ui, "还没有挂组件");
            }
        });


        // ── 物理 ──
        //
        // 取消勾选 = 这个实体不参与物理（纯视觉）。旧场景文件没有 `physics` 字段，
        // 加载出来就是未勾选 —— 与当时"没有任何东西会掉下去"的行为一致。
        theme::section_bar(ui, "Physics");
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

                // 枚举属性一律用**下拉**，不用并排 chip。
                //
                // 两个理由：① UE 的 Details 面板对 enum 就是 ComboBox；② 三个
                // `selectable_label` 加前缀标签在 300px 宽的面板里必然挤成一片，
                // 五个形状 chip 更是把 "Cylinder" 直接裁掉 —— 溢出不是"有点丑"，
                // 是**信息丢失**。
                row(ui, "Type", |ui| {
                    let current = match body.kind {
                        BodyKind::Static => "Static",
                        BodyKind::Dynamic => "Dynamic",
                        BodyKind::Kinematic => "Kinematic",
                    };
                    egui::ComboBox::from_id_salt("physics_kind")
                        .width(ui.available_width())
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (kind, label) in [
                                (BodyKind::Static, "Static"),
                                (BodyKind::Dynamic, "Dynamic"),
                                (BodyKind::Kinematic, "Kinematic"),
                            ] {
                                if ui.selectable_label(body.kind == kind, label).clicked()
                                    && body.kind != kind
                                {
                                    body.kind = kind;
                                    touched = true;
                                }
                            }
                        })
                        .response
                        .on_hover_text(match body.kind {
                            BodyKind::Static => "静态：地面 / 墙，不受力",
                            BodyKind::Dynamic => "动态：受重力与碰撞（物理驱动）",
                            BodyKind::Kinematic => "运动学：由脚本驱动位置（脚本驱动）",
                        });
                });

                // -- 形状 --
                row(ui, "Shape", |ui| {
                    let shape_names = ["Auto", "Box", "Ball", "Capsule", "Cylinder"];
                    let current = match body.shape {
                        CollisionShape::Auto => 0,
                        CollisionShape::Cuboid { .. } => 1,
                        CollisionShape::Ball { .. } => 2,
                        CollisionShape::Capsule { .. } => 3,
                        CollisionShape::Cylinder { .. } => 4,
                    };
                    egui::ComboBox::from_id_salt("physics_shape")
                        .width(ui.available_width())
                        .selected_text(shape_names[current])
                        .show_ui(ui, |ui| {
                            for (i, name) in shape_names.iter().enumerate() {
                                if ui.selectable_label(current == i, *name).clicked()
                                    && current != i
                                {
                                    body.shape = match i {
                                        0 => CollisionShape::Auto,
                                        1 => CollisionShape::Cuboid {
                                            // 从 Auto 切到 Box 时，用当前缩放推一个
                                            // 合理初值，免得一上来就是个 0.5 的小方块。
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
                });

                // -- 形状参数 --
                //
                // 必须 `match &mut body.shape` 而不是 `match body.shape`：
                // `CollisionShape` 是 `Copy`，写成后者会匹配到一个**临时副本**，
                // `ref mut` 绑在副本上，用户拖动参数时改动会被静默丢弃。
                match &mut body.shape {
                    CollisionShape::Auto => {
                        ui.label(theme::dim_note("按实体缩放的绝对值生成盒子"));
                    }
                    CollisionShape::Cuboid { half_extents } => {
                        row(ui, "Extents", |ui| {
                            for (axis, value) in
                                ["X", "Y", "Z"].into_iter().zip(half_extents.iter_mut())
                            {
                                theme::axis_label(ui, axis);
                                let resp = ui.add_sized(
                                    egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                                    egui::DragValue::new(value)
                                        .speed(0.02)
                                        .range(0.001..=f32::MAX),
                                );
                                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                            }
                        });
                    }
                    CollisionShape::Ball { radius } => {
                        row(ui, "Radius", |ui| {
                            let resp = ui.add_sized(
                                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                                egui::DragValue::new(radius)
                                    .speed(0.02)
                                    .range(0.001..=f32::MAX),
                            );
                            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                        });
                    }
                    CollisionShape::Capsule { half_height, radius }
                    | CollisionShape::Cylinder { half_height, radius } => {
                        row(ui, "Height", |ui| {
                            let resp = ui.add_sized(
                                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                                egui::DragValue::new(half_height)
                                    .speed(0.02)
                                    .range(0.0..=f32::MAX),
                            );
                            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                        });
                        row(ui, "Radius", |ui| {
                            let resp = ui.add_sized(
                                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                                egui::DragValue::new(radius)
                                    .speed(0.02)
                                    .range(0.001..=f32::MAX),
                            );
                            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                        });
                    }
                }

                row(ui, "Friction", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut body.friction)
                            .speed(0.01)
                            .range(0.0..=2.0),
                    );
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });
                row(ui, "Bounce", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut body.restitution)
                            .speed(0.01)
                            .range(0.0..=1.0),
                    );
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });
                row(ui, "Mass", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut body.mass)
                            .speed(0.1)
                            .range(0.0..=10_000.0),
                    )
                    .on_hover_text("0 = 按碰撞体密度自动推算");
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });
                row(ui, "Gravity", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut body.gravity_scale)
                            .speed(0.01)
                            .range(-5.0..=5.0),
                    )
                    .on_hover_text("重力倍率：1 = 正常，0 = 悬浮，负数 = 反重力");
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });
                let sensor = ui
                    .checkbox(&mut body.is_sensor, "Sensor (trigger)")
                    .on_hover_text("只检测重叠、不产生碰撞力；射线默认会忽略它")
                    .changed();
                touched |= sensor;
            }
        });


        // ── 角色控制器 ──
        //
        // 与 Physics 正交：勾上之后这个实体的刚体由**运行时托管**
        // （运动学位置 + 胶囊），`physics` 里的类型/形状会被忽略 ——
        // 见 `xgame_core::character` 的模块文档。
        theme::section_bar(ui, "Character");
        ui.indent("character", |ui| {
            let mut enabled = entity.controller.is_some();
            if ui
                .checkbox(&mut enabled, "Character controller")
                .on_hover_text("由角色控制器驱动：撞墙、自动上台阶、贴地、陡坡滑落")
                .changed()
            {
                entity.controller = if enabled {
                    Some(xgame_core::CharacterController::default())
                } else {
                    None
                };
                touched = true;
            }

            if let Some(c) = entity.controller.as_mut() {
                let mut field = |ui: &mut egui::Ui,
                                 label: &str,
                                 value: &mut f32,
                                 speed: f64,
                                 range: std::ops::RangeInclusive<f32>,
                                 hint: &str| {
                    row(ui, label, |ui| {
                        let resp = ui
                            .add_sized(
                                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                                egui::DragValue::new(value).speed(speed).range(range),
                            )
                            .on_hover_text(hint);
                        collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    });
                };
                field(
                    ui,
                    "Height",
                    &mut c.height,
                    0.02,
                    0.5..=5.0,
                    "总高（含两端半球）。脚底 = 实体原点",
                );
                field(
                    ui,
                    "Radius",
                    &mut c.radius,
                    0.01,
                    0.05..=2.0,
                    "胶囊半径。总高必须大于直径，否则会被钳成球",
                );
                field(
                    ui,
                    "Step",
                    &mut c.step_offset,
                    0.01,
                    0.0..=1.0,
                    "自动上台阶的最大高度；0 = 不许上台阶",
                );
                field(
                    ui,
                    "Slope",
                    &mut c.slope_limit,
                    0.5,
                    0.0..=89.0,
                    "可站立的最大坡度（度）；超过就滑下去",
                );
                field(
                    ui,
                    "Skin",
                    &mut c.skin_width,
                    0.005,
                    0.001..=0.5,
                    "与障碍物保持的微小间隙（米），太小会抖、太大会看起来悬空",
                );
                field(
                    ui,
                    "Snap",
                    &mut c.snap_to_ground,
                    0.01,
                    0.0..=2.0,
                    "贴地吸附距离：走下矮台阶时被吸住的最大落差",
                );
                theme::note(
                    ui,
                    format!("胶囊中心 = 脚底 + {:.2} m", c.center_height()),
                );
            }
        });

        // ── 音频 ──
        //
        // 取消勾选 = 静默实体。旧场景文件没有 `audio` 字段，加载出来就是
        // 未勾选 —— 与当时"没有任何声音"的行为一致。
        theme::section_bar(ui, "Audio");
        ui.indent("audio", |ui| {
            let mut enabled = entity.audio.is_some();
            if ui
                .checkbox(&mut enabled, "Has audio source")
                .on_hover_text("挂一个音频源：autoplay 进场景就响；\n非 autoplay 的剪辑由脚本 play_sound 触发")
                .changed()
            {
                entity.audio = if enabled {
                    Some(xgame_core::AudioSource::default())
                } else {
                    None
                };
                touched = true;
            }

            if let Some(audio) = entity.audio.as_mut() {
                row(ui, "Clip", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
                        egui::TextEdit::singleline(&mut audio.clip)
                            .hint_text("Resources/Audio/Beep.wav"),
                    );
                    if resp.changed() {
                        touched = true;
                    }
                });

                row(ui, "Volume", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut audio.volume)
                            .speed(0.01)
                            .range(0.0..=1.0),
                    );
                    if resp.changed() {
                        touched = true;
                    }
                });
                row(ui, "Pitch", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut audio.pitch)
                            .speed(0.01)
                            .range(0.05..=4.0),
                    );
                    if resp.changed() {
                        touched = true;
                    }
                    ui.label(theme::dim_note("1.0 = 原速"));
                });

                if ui
                    .checkbox(&mut audio.looping, "Loop")
                    .on_hover_text("循环声只能被 stop_* 命令停掉")
                    .changed()
                {
                    touched = true;
                }
                if ui
                    .checkbox(&mut audio.autoplay, "Autoplay")
                    .on_hover_text("进场景自动播一次（每实体只播一次，不会每帧重播）")
                    .changed()
                {
                    touched = true;
                }
                if ui
                    .checkbox(&mut audio.spatial, "Spatial (3D)")
                    .on_hover_text("随实体移动更新发声位置；监听者 = 当前相机")
                    .changed()
                {
                    touched = true;
                }
                if audio.spatial {
                    row(ui, "Distance", |ui| {
                        let min = ui.add_sized(
                            egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                            egui::DragValue::new(&mut audio.min_distance)
                                .speed(0.05)
                                .range(0.001..=f32::MAX),
                        );
                        ui.label(theme::dim_note("→"));
                        let max = ui.add_sized(
                            egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                            egui::DragValue::new(&mut audio.max_distance)
                                .speed(0.1)
                                .range(0.002..=f32::MAX),
                        );
                        if min.changed() || max.changed() {
                            // 引擎侧要求 max > min，编辑器先把住门。
                            if audio.max_distance <= audio.min_distance {
                                audio.max_distance = audio.min_distance + 0.001;
                            }
                            touched = true;
                        }
                    });
                    // 只加一次：这一行同时标出两列的语义（min / max），
                    // 挤在控件后面会把工具栏式的紧凑感打散。
                    theme::note(ui, "min → max：衰减区间（米）");
                }
            }
        });

        // ── 相机 ──
        //
        // 与 Audio / Particle 同一套"勾选才有"的写法。相机是**普通实体组件**
        // （不是场景级数据），所以它在这里，而光源在项目设置里。
        //
        // 没有"预览画面"按钮：Play 态视口直接用这台相机的机位
        // （见 `scene_view::build_view_projection`），那本身就是预览。
        theme::section_bar(ui, "Camera");
        ui.indent("camera", |ui| {
            let mut enabled = entity.camera.is_some();
            if ui
                .checkbox(&mut enabled, "Has camera")
                .on_hover_text(
                    "勾上之后这个实体就是一台相机：\n\
                     Play 态视口 / 独立游戏 / 无头截图都从它取机位",
                )
                .changed()
            {
                entity.camera = if enabled {
                    Some(xgame_core::Camera::default())
                } else {
                    None
                };
                touched = true;
            }

            if let Some(cam) = entity.camera.as_mut() {
                // `sanitized()` 的边界在这里**手写两遍**是刻意的：Inspector 要能在
                // 拖拽过程中就拦住坏值，而不是等 `CameraView` 解析时再钳 ——
                // 否则用户会看到"我拖到 0 了但画面没变化"，比直接拖不动更费解。
                row(ui, "FOV", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut cam.fov)
                            .speed(0.5)
                            .range(1.0..=179.0)
                            .fixed_decimals(1),
                    );
                    ui.label(theme::dim_note("度（垂直）"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Near", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut cam.near)
                            .speed(0.01)
                            .range(0.001..=1.0e4)
                            .fixed_decimals(3),
                    );
                    ui.label(theme::dim_note("米"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    // far 必须 > near：改完 near 顺手把 far 顶上去，
                    // 免得留下一个"深度范围为空"的相机（整屏没有东西）。
                    if cam.far <= cam.near {
                        cam.far = cam.near + 0.001;
                    }
                });

                row(ui, "Far", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut cam.far)
                            .speed(1.0)
                            .range(0.002..=1.0e5)
                            .fixed_decimals(1),
                    );
                    ui.label(theme::dim_note("米"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    if cam.far <= cam.near {
                        cam.far = cam.near + 0.001;
                    }
                });

                if ui
                    .checkbox(&mut cam.active, "Active")
                    .on_hover_text(
                        "多机位时用哪一台：解析规则是「第一个 active 的相机」，\n\
                         全部取消勾选时退回场景里第一台（画面不会突然没有相机）",
                    )
                    .changed()
                {
                    touched = true;
                }
            }
        });

        // ── 粒子发射器 ──
        //
        // 与 Audio 同一套"勾选才有"的写法。组件是纯声明（`core::ParticleEmitter`），
        // 模拟住在运行时的 `ParticleSystem` 里：**编辑态看不见粒子，Play 才喷** ——
        // 所以这里没有任何"预览播放"按钮，那需要另造一条编辑器内模拟回路，
        // 而 Play 就是那条回路。
        theme::section_bar(ui, "Particle");
        ui.indent("particle", |ui| {
            let mut enabled = entity.particle.is_some();
            if ui
                .checkbox(&mut enabled, "Has particle emitter")
                .on_hover_text("CPU 粒子（billboard）。进 Play 开始喷，\n场景存盘只存参数、不存粒子状态")
                .changed()
            {
                entity.particle = if enabled {
                    Some(xgame_core::ParticleEmitter::default())
                } else {
                    None
                };
                touched = true;
            }

            if let Some(p) = entity.particle.as_mut() {
                // 预设：只是"把一组字段填成某个值"，之后照常可改。
                // 不放 Burst（一次性爆发）—— burst 是运行态动作（脚本 / 按键
                // 触发），编辑态按了也没有模拟层去消费它。
                ui.horizontal(|ui| {
                    ui.label(theme::dim_note("Preset:"));
                    if icons::chip(ui, "Fire", "火焰：快、短命、加色、往上飘") {
                        *p = xgame_core::ParticleEmitter::fire();
                        touched = true;
                    }
                    if icons::chip(ui, "Smoke", "烟尘：慢、长大、Alpha、继续上浮") {
                        *p = xgame_core::ParticleEmitter::smoke();
                        touched = true;
                    }
                    if icons::chip(ui, "Sparks", "火花：小、快、强重力、加色") {
                        *p = xgame_core::ParticleEmitter::sparks();
                        touched = true;
                    }
                });

                if ui
                    .checkbox(&mut p.emitting, "Emitting")
                    .on_hover_text("关掉后已出生的粒子走完寿命再消失（不是清屏）")
                    .changed()
                {
                    touched = true;
                }

                row(ui, "Rate", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.rate)
                            .speed(1.0)
                            .range(0.0..=1.0e5)
                            .fixed_decimals(0),
                    );
                    ui.label(theme::dim_note("/s"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Max", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.max_particles)
                            .speed(1.0)
                            .range(0..=xgame_core::particle::MAX_PARTICLES_LIMIT),
                    );
                    ui.label(theme::dim_note("同时存在上限"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Duration", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.duration)
                            .speed(0.1)
                            .range(0.0..=1.0e5)
                            .fixed_decimals(1),
                    );
                    ui.label(theme::dim_note("s（0 = 一直喷）"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    // Loop 是离散勾选（不是拖拽），changed 直接进 touched。
                    if ui
                        .checkbox(&mut p.looping, "Loop")
                        .on_hover_text("到时重来（只在 Duration > 0 时有意义）")
                        .changed()
                    {
                        touched = true;
                    }
                });
                row(ui, "Lifetime", |ui| {
                    let a = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.lifetime[0])
                            .speed(0.05)
                            .range(0.001..=1.0e4),
                    );
                    ui.label(theme::dim_note("→"));
                    let b = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.lifetime[1])
                            .speed(0.05)
                            .range(0.001..=1.0e4),
                    );
                    ui.label(theme::dim_note("s"));
                    collect(&a, &mut touched, &mut drag_began, &mut drag_ended);
                    collect(&b, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Speed", |ui| {
                    let a = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.speed[0])
                            .speed(0.1)
                            .range(0.0..=1.0e4),
                    );
                    ui.label(theme::dim_note("→"));
                    let b = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.speed[1])
                            .speed(0.1)
                            .range(0.0..=1.0e4),
                    );
                    ui.label(theme::dim_note("m/s"));
                    collect(&a, &mut touched, &mut drag_began, &mut drag_ended);
                    collect(&b, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Spread", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.spread)
                            .speed(1.0)
                            .range(0.0..=180.0)
                            .fixed_decimals(0),
                    );
                    ui.label(theme::dim_note("°（0 = 直线，180 = 全向）"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });

                // 方向是实体局部向量：三轴宽度与 Transform 同行算法一致。
                let axis_w = theme::axis_field_width(ui);
                theme::field_row(ui, "Direction", |ui| {
                    // zip(iter_mut) 而不是 `[&mut a[0], ...]` 数组字面量：数组索引
                    // 拿不到字段级借用拆分，会被判成同一数组的多次可变借用。
                    for (axis, value) in ["X", "Y", "Z"].into_iter().zip(p.direction.iter_mut()) {
                        theme::axis_label(ui, axis);
                        let resp = theme::drag_field_w(ui, axis_w, value, 0.1, "");
                        collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    }
                });

                row(ui, "Shape", |ui| {
                    let mut shape = p.shape;
                    egui::ComboBox::from_id_salt("inspector_particle_shape")
                        .width(ui.available_width())
                        .selected_text(match shape {
                            xgame_core::particle::EmissionShape::Point => "Point",
                            xgame_core::particle::EmissionShape::Sphere { .. } => "Sphere",
                            xgame_core::particle::EmissionShape::Box { .. } => "Box",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut shape,
                                xgame_core::particle::EmissionShape::Point,
                                "Point",
                            );
                            ui.selectable_value(
                                &mut shape,
                                xgame_core::particle::EmissionShape::Sphere { radius: 0.15 },
                                "Sphere",
                            );
                            ui.selectable_value(
                                &mut shape,
                                xgame_core::particle::EmissionShape::Box {
                                    half_extents: [0.15, 0.15, 0.15],
                                },
                                "Box",
                            );
                        });
                    if shape != p.shape {
                        p.shape = shape;
                        touched = true;
                    }
                });
                match p.shape {
                    xgame_core::particle::EmissionShape::Sphere { radius } => {
                        row(ui, "Radius", |ui| {
                            let mut r = radius;
                            let resp = ui.add_sized(
                                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                                egui::DragValue::new(&mut r)
                                    .speed(0.05)
                                    .range(0.0..=1.0e4),
                            );
                            ui.label(theme::dim_note("m"));
                            collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                            if r != radius {
                                p.shape = xgame_core::particle::EmissionShape::Sphere { radius: r };
                            }
                        });
                    }
                    xgame_core::particle::EmissionShape::Box { mut half_extents } => {
                        theme::field_row(ui, "Half", |ui| {
                            for (axis, value) in
                                ["X", "Y", "Z"].into_iter().zip(half_extents.iter_mut())
                            {
                                theme::axis_label(ui, axis);
                                let resp = theme::drag_field_w(ui, axis_w, value, 0.05, "");
                                collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                            }
                        });
                        p.shape = xgame_core::particle::EmissionShape::Box { half_extents };
                    }
                    xgame_core::particle::EmissionShape::Point => {}
                }

                theme::field_row(ui, "Gravity", |ui| {
                    for (axis, value) in ["X", "Y", "Z"].into_iter().zip(p.gravity.iter_mut()) {
                        theme::axis_label(ui, axis);
                        let resp = theme::drag_field_w(ui, axis_w, value, 0.1, "");
                        collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                    }
                });

                row(ui, "Drag", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.drag)
                            .speed(0.05)
                            .range(0.0..=1.0e3),
                    );
                    ui.label(theme::dim_note("每秒衰减比例"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Size", |ui| {
                    let a = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.start_size)
                            .speed(0.01)
                            .range(0.0..=1.0e4),
                    );
                    ui.label(theme::dim_note("→"));
                    let b = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.end_size)
                            .speed(0.01)
                            .range(0.0..=1.0e4),
                    );
                    ui.label(theme::dim_note("出生 → 死亡"));
                    collect(&a, &mut touched, &mut drag_began, &mut drag_ended);
                    collect(&b, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Color", |ui| {
                    // 与材质 Tint 同一套写法：先写临时变量，只有真的交互过才回写
                    // （取色器会把 0.38 量化成 0.38039216，白开一次面板就"未保存"）。
                    let mut start = p.start_color;
                    let ra = ui.color_edit_button_rgba_unmultiplied(&mut start);
                    if ra.changed() {
                        p.start_color = start;
                    }
                    ui.label(theme::dim_note("→"));
                    let mut end = p.end_color;
                    let rb = ui.color_edit_button_rgba_unmultiplied(&mut end);
                    if rb.changed() {
                        p.end_color = end;
                    }
                    ui.label(theme::dim_note("出生 → 死亡"));
                    collect(&ra, &mut touched, &mut drag_began, &mut drag_ended);
                    collect(&rb, &mut touched, &mut drag_began, &mut drag_ended);
                });

                row(ui, "Blend", |ui| {
                    let mut blend = p.blend;
                    egui::ComboBox::from_id_salt("inspector_particle_blend")
                        .width(ui.available_width())
                        .selected_text(match blend {
                            xgame_core::particle::ParticleBlend::Alpha => "Alpha",
                            xgame_core::particle::ParticleBlend::Additive => "Additive",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut blend,
                                xgame_core::particle::ParticleBlend::Alpha,
                                "Alpha（烟雾 / 尘土）",
                            );
                            ui.selectable_value(
                                &mut blend,
                                xgame_core::particle::ParticleBlend::Additive,
                                "Additive（火焰 / 火花）",
                            );
                        });
                    if blend != p.blend {
                        p.blend = blend;
                        touched = true;
                    }
                });

                row(ui, "Seed", |ui| {
                    let resp = ui.add_sized(
                        egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                        egui::DragValue::new(&mut p.seed).speed(1.0),
                    );
                    ui.label(theme::dim_note("同种子 ⇒ 逐位可复现"));
                    collect(&resp, &mut touched, &mut drag_began, &mut drag_ended);
                });
            }
        });


        // ── 材质资产 + 覆盖 ──
        //
        // 资产负责"这是什么材质"（跨实体共享，`.mat` 可复用）；下面的 Override
        // 负责"这个实例想怎么微调"。两层正交：资产定金属度 / 粗糙度，覆盖在
        // 其结果之上逐字段打补丁。内置图元（builtin://cube 等）没有模型材质，
        // 要么挂资产、要么用覆盖，否则永远是白模。
        theme::section_bar(ui, "Material");
        ui.indent("material", |ui| {
            let materials = state.material_list();
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
                .on_hover_text("在内置白模 / glTF 材质之上逐字段覆盖 PBR 参数；\n只影响勾选的字段")
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
        });

        // ── 添加组件（占位）──
        ui.add_space(4.0);
        ui.add_enabled_ui(false, |ui| {
            icons::add_row(
                ui,
                "Add Component",
                "自由组件系统尚未开放序列化；脚本 / 物理已可直接编辑",
            );
        });
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
            // 组件参数（P1.5）与其它字段一样活在 `working` 里、随这次写回一起
            // 落盘 —— 没有第二条写回路径，也就没有"旧克隆盖掉刚调的参数"的问题。
            *slot = working;
            // 摘掉组件时顺手清掉它的孤儿参数（键是注册名，删列表项不会自动清）。
            // 留着不出错（装配按 behaviors 查表，孤儿根本读不到），但会污染场景
            // 文件，还会在"删了又加回来"时让旧参数神秘复活。
            let behaviors = &slot.behaviors;
            slot.behavior_params.retain(|name, _| behaviors.contains(name));
        }
    }

    // ── 父节点（层级）──
    // 刻意放在写回**之后**：`reparent_entity` 按"保留世界变换"会改写实体自己的
    // `transform`，若先 reparent 再被 `working` 写回，那次换算就被覆盖掉了。
    theme::section_bar(ui, "Parent");
    ui.indent("parent", |ui| {
        let current_parent = state.scene.parent_index(index);
        let parent_label = match current_parent {
            Some(p) => state.scene.entities[p].name.clone(),
            None => "(none — scene root)".to_string(),
        };
        let mut chosen: Option<Option<usize>> = None;
        ui.add_enabled_ui(state.is_editing(), |ui| {
            row(ui, "Parent", |ui| {
                egui::ComboBox::from_id_salt("inspector_parent")
                    .width(ui.available_width())
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
        });
        if let Some(parent) = chosen {
            state.reparent_entity(index, parent);
        }
    });

    // ── 预制体（实例来源 / Apply / Revert）──
    // 放在最后：这几个按钮会直接改场景（Revert 会重建整棵子树、让索引位移），
    // 绝不能和上面的 `working` 写回抢位置。
    let prefab_link = state
        .scene
        .entities
        .get(index)
        .and_then(|e| e.prefab.clone());
    if let Some(link) = prefab_link {
        theme::section_bar(ui, "Prefab");
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
                ui.label(theme::dim_note(match &root_name {
                    Some(n) => format!("实例内部节点（根: {n}）"),
                    None => "实例内部节点".to_string(),
                }));
                if let Some(r) = root {
                    if ui.button("Select Instance Root").clicked() {
                        state.selected_entity = Some(r);
                    }
                }
            }
        });
    }
}
