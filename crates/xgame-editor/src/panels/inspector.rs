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
pub(super) use xgame_core::ComponentAddress;
use xgame_core::{
    BuiltinComponent, Entity, EntityId, FieldDesc, FieldKind, FieldValue, Inspect,
};
use xgame_math::glam::{Vec2, Vec3, Vec4};

/// 从一个控件的响应里收集「有改动 / 拖拽开始 / 拖拽结束」三个信号。
pub(super) fn collect(
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
pub(super) fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
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

// ---------------------------------------------------------------------------
// 组件结构操作（Add Component / 标题右键 / 空白右键共用）
// ---------------------------------------------------------------------------

/// Inspector 里所有组件结构编辑最终都收敛成这一种请求。
///
/// 菜单只负责产生请求，真正修改仍在 `working` 副本上发生；这样右键不会绕过
/// `show_body` 末尾的 Undo、脏检查与文件身份合并。Rust / Lua 继续按当前运行时的
/// 注册名 / 路径去重，同名多实例要等运行时状态按 `ComponentId` 隔离后才能开放。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComponentAction {
    AddBuiltin(BuiltinComponent),
    AddRust(String),
    AddLua(String),
    Remove(ComponentAddress),
}

/// 组件标题右键里的“移除”语义。
#[derive(Clone, Copy)]
pub(super) enum ComponentMenuTarget {
    /// 标题正好对应一个可移除组件实例。
    Removable {
        address: ComponentAddress,
        label: &'static str,
    },
    /// Transform 这类真实存在、但不能删除的固有组件。
    Locked {
        label: &'static str,
        reason: &'static str,
    },
    /// Scripts / Behaviors 是实例集合，不能把分组标题冒充成一个组件。
    Collection {
        label: &'static str,
        reason: &'static str,
    },
}

pub(super) const PLAY_STRUCTURE_HINT: &str = "播放模式下不能修改组件结构";
pub(super) const TRANSFORM_REMOVE_REASON: &str =
    "Transform 是每个实体的固有组件；可以改值，但不能从实体上移除。";

/// 执行一个统一组件请求。`editing` 是最后一道结构门禁，菜单是否置灰不能替代它。
fn apply_component_action(
    entity: &mut Entity,
    action: ComponentAction,
    editing: bool,
    animator_clip: Option<String>,
) -> bool {
    if !editing {
        return false;
    }
    match action {
        ComponentAction::AddBuiltin(kind) => {
            let changed = entity.add_builtin_component(kind);
            if changed && kind == BuiltinComponent::Animator {
                if let (Some(clip), Some(animator)) = (animator_clip, entity.animator.as_mut()) {
                    animator.clip = clip;
                }
            }
            changed
        }
        ComponentAction::AddRust(name) => entity.add_registered_component(&name),
        ComponentAction::AddLua(path) => entity.attach_lua_script(&path),
        ComponentAction::Remove(address) => entity.remove_attached_component(address),
    }
}

/// Animator 新挂上去时沿用旧入口的友好默认值：模型有剪辑就先选第一段。
fn default_animator_clip(state: &EditorState, entity: &Entity) -> Option<String> {
    state.scene_renderer.as_ref().and_then(|renderer| {
        entity
            .model_at(0)
            .and_then(|model| renderer.animation_clip_names(model).into_iter().next())
    })
}

/// 空白右键发生在 `show_body` 返回之后，无法再借用那一帧的 `working`；这里按完全
/// 相同的“先克隆、后入 Undo、再替换”顺序提交选中实体，仍不直接碰文件身份侧车。
fn apply_component_action_to_selected(state: &mut EditorState, action: ComponentAction) -> bool {
    let editing = state.is_editing();
    let Some(index) = state.selected_entity else {
        return false;
    };
    let Some(mut working) = state.scene.entities.get(index).cloned() else {
        return false;
    };
    let animator_clip = default_animator_clip(state, &working);
    if !apply_component_action(&mut working, action, editing, animator_clip) {
        return false;
    }

    // 必须在替换实体之前拍快照；文件模型会在这里同步当前身份与持久高水位。
    state.push_undo();
    state.scene.entities[index] = working;
    true
}

/// “Add Component” 的唯一菜单内容。三个入口只决定菜单挂在哪里，不再各抄一份增删。
fn show_add_component_entries(
    ui: &mut egui::Ui,
    entity: &Entity,
    rust_names: &[String],
    script_paths: &[String],
    request: &mut Option<ComponentAction>,
) -> bool {
    ui.set_min_width(theme::COMPONENT_MENU_WIDTH);
    let mut any = false;

    let missing_builtin = BuiltinComponent::ADDABLE
        .into_iter()
        .filter(|kind| !kind.present(entity))
        .collect::<Vec<_>>();
    if !missing_builtin.is_empty() {
        ui.label(theme::dim_note("Built-in Components"));
        for kind in missing_builtin {
            any = true;
            if ui.button(kind.name()).clicked() {
                *request = Some(ComponentAction::AddBuiltin(kind));
                ui.close_menu();
            }
        }
    }

    let missing_rust = rust_names
        .iter()
        .filter(|name| !entity.behaviors.contains(*name))
        .collect::<Vec<_>>();
    if !missing_rust.is_empty() {
        if any {
            ui.separator();
        }
        ui.label(theme::dim_note("Rust Components"));
        for name in missing_rust {
            any = true;
            if ui.button(name).clicked() {
                *request = Some(ComponentAction::AddRust(name.clone()));
                ui.close_menu();
            }
        }
    } else if rust_names.is_empty() {
        if any {
            ui.separator();
        }
        theme::note(ui, "Rust 组件：构建项目插件后可选");
        any = true;
    }

    let missing_scripts = script_paths
        .iter()
        .filter(|path| !entity.scripts.contains(*path))
        .collect::<Vec<_>>();
    if !missing_scripts.is_empty() {
        if any {
            ui.separator();
        }
        ui.label(theme::dim_note("Lua Scripts"));
        for path in missing_scripts {
            any = true;
            if ui.button(path).clicked() {
                *request = Some(ComponentAction::AddLua(path.clone()));
                ui.close_menu();
            }
        }
    }

    if !any {
        theme::note(ui, "当前可用组件都已挂载");
    }
    ui.separator();
    theme::note(ui, "Transform 始终存在；Mesh / LOD / Material 可独立增删。");
    ui.separator();
    let refresh = ui.button("重新扫描 Scripts/").clicked();
    if refresh {
        ui.close_menu();
    }
    refresh
}

/// 给组件标题或具体 Rust / Lua 行挂统一右键菜单。
pub(super) fn component_context_menu(
    response: &egui::Response,
    state: &EditorState,
    entity: &Entity,
    target: ComponentMenuTarget,
    request: &mut Option<ComponentAction>,
    refresh_scripts: &mut bool,
) {
    let editing = state.is_editing();
    response.context_menu(|ui| {
        match target {
            ComponentMenuTarget::Removable { address, label } => {
                let remove = ui
                    .add_enabled(editing, egui::Button::new(label))
                    .on_disabled_hover_text(PLAY_STRUCTURE_HINT);
                if remove.clicked() {
                    *request = Some(ComponentAction::Remove(address));
                    ui.close_menu();
                }
            }
            ComponentMenuTarget::Locked { label, reason }
            | ComponentMenuTarget::Collection { label, reason } => {
                ui.add_enabled(false, egui::Button::new(label))
                    .on_disabled_hover_text(reason);
                theme::note(ui, reason);
            }
        }

        ui.separator();
        let add = ui.add_enabled_ui(editing, |ui| {
            ui.menu_button("Add Component", |ui| {
                *refresh_scripts |= show_add_component_entries(
                    ui,
                    entity,
                    state.behavior_names(),
                    &state.script_list,
                    request,
                );
            });
        });
        add.response.on_disabled_hover_text(PLAY_STRUCTURE_HINT);
    });
}

/// 一个字段控件产生的编辑信号（与 [`collect`] 同语义，聚合成结构体往下传）。
///
/// 单独成型是因为 P1.5 之后组件字段改动**进撤销栈、进场景文件**：需要在
/// "改动发生"与"写回场景"之间拿到 `changed` / 拖拽起点 / 拖拽终点三个时机，
/// 而一个字段一个 `bool` 传三遍的写法在有五种字段类型的地方很快失控。
#[derive(Default, Clone, Copy)]
pub(crate) struct FieldSignals {
    /// 本帧有字段被改动（对应 `Response::changed`）。
    pub(super) changed: bool,
    /// 本帧有控件开始拖拽（改前快照的拍摄时机）。
    pub(super) drag_began: bool,
    /// 本帧有控件结束拖拽（入撤销栈的时机）。
    pub(super) drag_ended: bool,
}

impl FieldSignals {
    pub(super) fn collect(&mut self, resp: &egui::Response) {
        self.changed |= resp.changed();
        self.drag_began |= resp.drag_started();
        self.drag_ended |= resp.drag_stopped();
    }

    /// 手工标一次"本帧有改动"。
    ///
    /// 有些控件的改动不体现在 `Response::changed` 上 —— 下拉框（`ComboBox`）
    /// 的结果来自 `show_ui` 闭包里被点中的那一项，外层响应永远是"没变"。
    /// 不补这一下，选完总线场景不会被标脏，Ctrl+S 不会保存它。
    pub(super) fn mark_changed(&mut self) {
        self.changed = true;
    }

    /// 专用渲染器汇总用：它自己算好了"这一帧有没有改动"，这里只做并集。
    ///
    /// 存在的理由是 `Render::Custom` 那一类段落的控件是手排的，
    /// 没有逐控件的 `Response` 可收 —— 但它们仍然要参与同一套脏检查与撤销。
    pub(super) fn collect_changed_any(&mut self, changed: bool) {
        self.changed |= changed;
    }

    /// 专用渲染器手工报"开始拖拽"（它没有逐控件的 `Response` 可收）。
    pub(super) fn mark_drag_began(&mut self) {
        self.drag_began = true;
    }

    /// 专用渲染器手工报"结束拖拽"。
    pub(super) fn mark_drag_ended(&mut self) {
        self.drag_ended = true;
    }
}

/// 组件字段一帧的编辑结果（P1.5）——信号交给面板级撤销逻辑，值交给 `working`。
#[derive(Default)]
pub(super) struct BehaviorEdit {
    /// 三个撤销信号（与 [`FieldSignals`] 同语义）。
    pub(super) sig: FieldSignals,
    /// `Some(新参数表)` = 本帧有字段被改动，调用方写进 `working.behavior_params`；
    /// `None` = 没动过 —— `working` 克隆自场景，那份本来就是对的，不必回写。
    ///
    /// **不直接写 `state.scene`**：组件参数从此与 Name / Transform 走**同一条**
    /// 通道（`working` → 撤销 → 写回场景），撤销与脏检查的时序不用为它单开
    /// 一条支路 —— 支路就是将来"调完参改个名字，参数被旧克隆盖掉"的出处。
    pub(super) values: Option<xgame_core::inspect::FieldValues>,
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
pub(super) fn show_behavior_fields(
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
            let text = ui.add_sized(
                [label_w, ui.spacing().interact_size.y],
                egui::Label::new(label),
            );
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
            let resp = ui.add(
                egui::TextEdit::singleline(&mut s)
                    .hint_text(hint)
                    .desired_width(140.0),
            );
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
                for (axis, f) in ["X", "Y", "Z", "W"].iter().zip(arr.iter_mut().take(dims)) {
                    ui.label(egui::RichText::new(*axis).weak().size(10.0));
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
    let body = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
        // **分组内容不额外缩进**。面板里的 `ui.indent(...)` 只用来给代码分块，
        // 视觉上必须是零缩进：UE 的 Details 里分类条下的行与 Name 行共用同一条
        // 左边界，否则（indent = 10px）Name 的值在 x=64、Transform 的值在 x=74，
        // 整块看下来所有控件左右各错开一档 —— 这是"排版不整齐"最主要的一条。
        ui.spacing_mut().indent = 0.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| show_body(state, ui));
    });

    // 子控件的感知层级高于这个 scope 背景：右键标题 / 具体组件行时由它们自己的
    // 菜单接管，只有真正的 Inspector 空白才会落到这里。
    let Some(index) = state.selected_entity else {
        return;
    };
    let Some(entity) = state.scene.entities.get(index).cloned() else {
        return;
    };
    let editing = state.is_editing();
    let rust_names = state.behavior_names().to_vec();
    let script_paths = state.script_list.clone();
    let mut request = None;
    let mut refresh_scripts = false;
    body.response.context_menu(|ui| {
        ui.label(theme::bold("Add Component"));
        ui.separator();
        let contents = ui.add_enabled_ui(editing, |ui| {
            refresh_scripts |= show_add_component_entries(
                ui,
                &entity,
                &rust_names,
                &script_paths,
                &mut request,
            );
        });
        contents.response.on_disabled_hover_text(PLAY_STRUCTURE_HINT);
        if !editing {
            theme::note(ui, PLAY_STRUCTURE_HINT);
        }
    });
    if refresh_scripts {
        state.refresh_scripts();
    }
    if let Some(action) = request {
        apply_component_action_to_selected(state, action);
    }
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

    let mut touched = false;
    let mut drag_began = false;
    let mut drag_ended = false;
    let mut component_request = None;
    let mut refresh_scripts = false;

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

        // ── 统一添加入口 ──
        // 放在属性列表前面：挂载物理、动画的实体可能要滚很久才能看到列表尾部。
        // 数据组件来自插件注册表，内建组件仍写回原来的 Entity 字段。
        ui.add_space(ui.spacing().item_spacing.y);
        ui.add_enabled_ui(state.is_editing(), |ui| {
            let popup_id = egui::Id::new("inspector_add_component");
            let add = icons::add_row(ui, "Add Component", "给选中对象添加内建、Rust 或 Lua 组件");
            if add.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup_id));
            }
            egui::popup_below_widget(
                ui,
                popup_id,
                &add,
                egui::PopupCloseBehavior::CloseOnClick,
                |ui| {
                    refresh_scripts |= show_add_component_entries(
                        ui,
                        entity,
                        state.behavior_names(),
                        &state.script_list,
                        &mut component_request,
                    );
                },
            );
        });

        // ── 变换组件 ──
        let mut reflected = FieldSignals::default();
        super::inspector_reflect::show_section(
            state,
            ui,
            entity,
            &super::inspector_reflect::TRANSFORM,
            &mut reflected,
            &mut component_request,
            &mut refresh_scripts,
        );
        touched |= reflected.changed;
        drag_began |= reflected.drag_began;
        drag_ended |= reflected.drag_ended;

        // ── UI 元素（屏幕空间）──
        //
        // 紧挨着 Transform：UI 元素的"矩形"就是锚点 + 偏移，语义上正是
        // 这个实体的变换；放在面板末尾（Material 之后）时，属性多的实体
        // 要滚很久才能看到它 —— 实测截图里它整块在折叠线以下。
        if entity.ui.is_some() {
            // UI 段走**专用渲染器**（版式本身承载信息，见 `Render::Custom` 的说明），
            // 但入口与其余组件一致：同一张 spec 表、同一个 `show_section`。
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::UI,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }

        // ── 网格 ──
        // Morph 权重跟模型组件同生共灭：它按当前模型的目标下标解释，单独留下只会
        // 变成一张没有目标资产可对应的孤儿数组。因此 Morph 控件收进 Mesh 区块，
        // 不再画一个看起来像第四份独立组件的标题。
        if BuiltinComponent::Mesh.present(entity) {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::MESH,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }

        // ── LOD（距离分级 + 绘制距离）──
        //
        // 语义（与 `xgame_core::lod` 同一份）：`levels` 非空时 `model` 被忽略，
        // 第 0 级就是最近一级；超出最后一级的 `max_distance` 直接不画
        // （写 0 = 无限远）。距离从相机量到**实体轴心**。
        if BuiltinComponent::Lod.present(entity) {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::LOD,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }

        // ── 动画 ──
        //
        // 只显示已经挂载的动画组件；模型被清空时仍要能移除它。
        // 剪辑列表来自渲染器的资产缓存，模型未加载时为空。
        if entity.animator.is_some() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::ANIMATION,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }

        // ── 脚本 ──
        //
        // 这里存的是**项目相对路径**，不是脚本内容：同一份脚本可以挂在多个实体上，
        // 改一次逻辑全都生效。列表顺序就是执行顺序。
        if !entity.scripts.is_empty() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::SCRIPTS,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
        // ── Behaviors（Rust 组件，双轨架构 P0.5）──
        // 场景存注册名而非代码；未注册的名字仍显示警告，方便查插件装载问题。
        if !entity.behaviors.is_empty() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::BEHAVIORS,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
        // ── 物理 ──
        //
        // 取消勾选 = 这个实体不参与物理（纯视觉）。旧场景文件没有 `physics` 字段，
        // 加载出来就是未勾选 —— 与当时"没有任何东西会掉下去"的行为一致。
        if entity.physics.is_some() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::PHYSICS,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
        // ── 角色控制器 ──
        //
        // 与 Physics 正交：勾上之后这个实体的刚体由**运行时托管**
        // （运动学位置 + 胶囊），`physics` 里的类型/形状会被忽略 ——
        // 见 `xgame_core::character` 的模块文档。
        if entity.controller.is_some() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::CHARACTER,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
        // ── 音频 ──
        if entity.audio.is_some() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::AUDIO,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
        // ── 相机（反射通道）──
        if entity.camera.is_some() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::CAMERA,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
        // ── 粒子发射器 ──
        //
        // 与 Audio 同一套"勾选才有"的写法。组件是纯声明（`core::ParticleEmitter`），
        // 模拟住在运行时的 `ParticleSystem` 里：**编辑态看不见粒子，Play 才喷** ——
        // 所以这里没有任何"预览播放"按钮，那需要另造一条编辑器内模拟回路，
        // 而 Play 就是那条回路。
        if entity.particle.is_some() {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::PARTICLE,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
        // ── 材质资产 + 覆盖 ──
        //
        // 资产负责"这是什么材质"（跨实体共享，`.mat` 可复用）；下面的 Override
        // 负责"这个实例想怎么微调"。两层正交：资产定金属度 / 粗糙度，覆盖在
        // 其结果之上逐字段打补丁。内置图元（builtin://cube 等）没有模型材质，
        // 要么挂资产、要么用覆盖，否则永远是白模。
        if BuiltinComponent::Material.present(entity) {
            let mut reflected = FieldSignals::default();
            super::inspector_reflect::show_section(
                state,
                ui,
                entity,
                &super::inspector_reflect::MATERIAL,
                &mut reflected,
                &mut component_request,
                &mut refresh_scripts,
            );
            touched |= reflected.changed;
            drag_began |= reflected.drag_began;
            drag_ended |= reflected.drag_ended;
        }
    }

    // ── 预制体（实例 → 资产的关系操作）──
    //
    // 放在撤销点之前：Apply / Revert 是**动作**，它们自己会处理撤销，
    // 而这段 UI 本身不改 `working`（它只改 `state.scene` 与资产文件）。
    //
    // 存在性判断必须在这里：`show_section` 先画分类条再进渲染器，
    // 不判断的话没有预制体的实体上会多出一个空的 "Prefab" 标题。
    if working.prefab.is_some() {
        let mut reflected = FieldSignals::default();
        super::inspector_reflect::show_section(
            state,
            ui,
            &mut working,
            &super::inspector_reflect::PREFAB,
            &mut reflected,
            &mut component_request,
            &mut refresh_scripts,
        );
        touched |= reflected.changed;
        drag_began |= reflected.drag_began;
        drag_ended |= reflected.drag_ended;
    }

    if refresh_scripts {
        state.refresh_scripts();
    }
    if let Some(action) = component_request {
        let animator_clip = default_animator_clip(state, &working);
        touched |= apply_component_action(
            &mut working,
            action,
            state.is_editing(),
            animator_clip,
        );
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
            slot.behavior_params
                .retain(|name, _| behaviors.contains(name));
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_actions_share_addresses_defaults_and_edit_gate() {
        let mut entity = Entity::new("Actor");

        assert!(!apply_component_action(
            &mut entity,
            ComponentAction::AddBuiltin(BuiltinComponent::Audio),
            false,
            None,
        ));
        assert!(entity.audio.is_none(), "Play 门禁不能只靠按钮置灰");

        assert!(apply_component_action(
            &mut entity,
            ComponentAction::AddBuiltin(BuiltinComponent::Animator),
            true,
            Some("Idle".into()),
        ));
        assert_eq!(entity.animator.as_ref().unwrap().clip, "Idle");
        assert!(!apply_component_action(
            &mut entity,
            ComponentAction::AddBuiltin(BuiltinComponent::Animator),
            true,
            Some("Run".into()),
        ));
        assert_eq!(
            entity.animator.as_ref().unwrap().clip,
            "Idle",
            "重复添加不能顺手改掉已有 Animator"
        );

        assert!(apply_component_action(
            &mut entity,
            ComponentAction::AddRust("Health".into()),
            true,
            None,
        ));
        assert!(apply_component_action(
            &mut entity,
            ComponentAction::AddLua("Scripts/player.lua".into()),
            true,
            None,
        ));
        assert_eq!(entity.behaviors, ["Health"]);
        assert_eq!(entity.scripts, ["Scripts/player.lua"]);

        assert!(apply_component_action(
            &mut entity,
            ComponentAction::Remove(ComponentAddress::Rust(0)),
            true,
            None,
        ));
        assert!(apply_component_action(
            &mut entity,
            ComponentAction::Remove(ComponentAddress::Lua(0)),
            true,
            None,
        ));
        assert!(entity.behaviors.is_empty() && entity.scripts.is_empty());
        assert!(!apply_component_action(
            &mut entity,
            ComponentAction::Remove(ComponentAddress::Builtin(BuiltinComponent::Transform)),
            true,
            None,
        ));
        assert!(apply_component_action(
            &mut entity,
            ComponentAction::AddBuiltin(BuiltinComponent::Mesh),
            true,
            None,
        ));
        entity.morph_weights = vec![0.25];
        assert!(apply_component_action(
            &mut entity,
            ComponentAction::AddBuiltin(BuiltinComponent::Lod),
            true,
            None,
        ));
        assert!(apply_component_action(
            &mut entity,
            ComponentAction::AddBuiltin(BuiltinComponent::Material),
            true,
            None,
        ));
        entity.material_asset = Some("Resources/Materials/Wood.mat".into());

        assert!(apply_component_action(
            &mut entity,
            ComponentAction::Remove(ComponentAddress::Builtin(BuiltinComponent::Mesh)),
            true,
            None,
        ));
        assert!(entity.model.is_empty() && entity.morph_weights.is_empty());
        assert!(!entity.lod.is_empty());
        assert!(entity.material_asset.is_some() && entity.material.is_some());
        assert!(apply_component_action(
            &mut entity,
            ComponentAction::Remove(ComponentAddress::Builtin(BuiltinComponent::Lod)),
            true,
            None,
        ));
        assert!(entity.lod.is_empty());
        assert!(entity.material_asset.is_some() && entity.material.is_some());
        assert!(apply_component_action(
            &mut entity,
            ComponentAction::Remove(ComponentAddress::Builtin(BuiltinComponent::Material)),
            true,
            None,
        ));
        assert!(entity.material_asset.is_none() && entity.material.is_none());
    }

    fn temp_editor(tag: &str) -> (EditorState, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "xgame_inspector_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let project = xgame_core::ProjectHandle::create(&base, "InspectorTest").unwrap();
        let path = project.manifest().entry_scene.clone();
        let v2 = xgame_core::scene_v2::SceneV2::from_runtime(
            &project.load_entry_scene().unwrap(),
        )
        .unwrap();
        std::fs::write(project.path(&path), v2.to_ron().unwrap()).unwrap();
        let mut state = EditorState::new(project);
        state.selected_entity = Some(0);
        (state, base)
    }

    fn saved_audio_id(state: &EditorState) -> xgame_core::component_id::ComponentId {
        let scene = state
            .project
            .load_scene_v2(&state.current_scene)
            .unwrap();
        scene.entities[0]
            .components
            .entries
            .iter()
            .find_map(|entry| {
                matches!(
                    entry.data,
                    xgame_core::scene_v2::ComponentData::Audio(_)
                )
                .then_some(entry.identity.id)
            })
            .expect("应当保存出 Audio 组件")
    }

    fn saved_visual_id(
        state: &EditorState,
        kind: BuiltinComponent,
    ) -> xgame_core::component_id::ComponentId {
        let scene = state
            .project
            .load_scene_v2(&state.current_scene)
            .unwrap();
        scene.entities[0]
            .components
            .entries
            .iter()
            .find_map(|entry| {
                let matches = matches!(
                    (&entry.data, kind),
                    (
                        xgame_core::scene_v2::ComponentData::Mesh(_),
                        BuiltinComponent::Mesh
                    ) | (
                        xgame_core::scene_v2::ComponentData::Lod(_),
                        BuiltinComponent::Lod
                    ) | (
                        xgame_core::scene_v2::ComponentData::Material(_),
                        BuiltinComponent::Material
                    )
                );
                matches.then_some(entry.identity.id)
            })
            .unwrap_or_else(|| panic!("应当保存出 {} 组件", kind.name()))
    }

    #[test]
    fn selected_component_action_is_undoable_and_never_reuses_v2_ids() {
        let (mut state, base) = temp_editor("v2_identity");

        assert!(apply_component_action_to_selected(
            &mut state,
            ComponentAction::AddBuiltin(BuiltinComponent::Audio),
        ));
        assert!(state.scene.entities[0].audio.is_some());
        state.save_scene().unwrap();
        let first_id = saved_audio_id(&state);

        state.undo();
        assert!(state.scene.entities[0].audio.is_none());
        state.save_scene().unwrap();
        state.redo();
        assert!(state.scene.entities[0].audio.is_some());
        state.save_scene().unwrap();
        assert_eq!(saved_audio_id(&state), first_id, "Redo 必须找回原组件身份");

        state.undo();
        state.save_scene().unwrap();
        assert!(apply_component_action_to_selected(
            &mut state,
            ComponentAction::AddBuiltin(BuiltinComponent::Audio),
        ));
        state.save_scene().unwrap();
        let second_id = saved_audio_id(&state);
        assert!(
            second_id.raw() > first_id.raw(),
            "删除后重新添加必须从持久高水位继续分配"
        );

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn selected_visual_components_keep_independent_ids_through_undo_and_readd() {
        let (mut state, base) = temp_editor("visual_v2_identity");
        let visual = [
            BuiltinComponent::Mesh,
            BuiltinComponent::Lod,
            BuiltinComponent::Material,
        ];
        for kind in visual {
            assert!(apply_component_action_to_selected(
                &mut state,
                ComponentAction::AddBuiltin(kind),
            ));
        }
        state.save_scene().unwrap();

        for kind in visual {
            let first_id = saved_visual_id(&state, kind);
            assert!(apply_component_action_to_selected(
                &mut state,
                ComponentAction::Remove(ComponentAddress::Builtin(kind)),
            ));
            assert!(!kind.present(&state.scene.entities[0]));
            for survivor in visual.into_iter().filter(|candidate| *candidate != kind) {
                assert!(
                    survivor.present(&state.scene.entities[0]),
                    "移除 {} 不能连带删除 {}",
                    kind.name(),
                    survivor.name()
                );
            }
            state.save_scene().unwrap();

            state.undo();
            assert!(kind.present(&state.scene.entities[0]));
            state.save_scene().unwrap();
            assert_eq!(
                saved_visual_id(&state, kind),
                first_id,
                "Undo 必须找回 {} 的原身份",
                kind.name()
            );

            state.redo();
            assert!(!kind.present(&state.scene.entities[0]));
            state.save_scene().unwrap();
            assert!(apply_component_action_to_selected(
                &mut state,
                ComponentAction::AddBuiltin(kind),
            ));
            state.save_scene().unwrap();
            assert!(
                saved_visual_id(&state, kind).raw() > first_id.raw(),
                "删除后重新添加 {} 必须从持久高水位继续分配",
                kind.name()
            );
        }

        std::fs::remove_dir_all(base).unwrap();
    }
}
