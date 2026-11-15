//! Inspector 的**反射通道**：按 `TypeMeta` 画字段行，不写死任何具体组件。
//!
//! # 它替代的是什么
//!
//! `inspector.rs` 里每个内置组件都有一段手写 section（Camera 约 90 行、Audio 约
//! 140 行），里面重复的是同一件事：读一个字段 → 按类型挑控件 → 改了就写回。
//! 这里把这件事变成一次性的：控件形态由 [`ScalarKind`] 决定，范围 / 步长 /
//! 小数位 / 说明文字由 [`FieldMeta`] 决定。
//!
//! # 为什么排版规则与手写段一致而不是另立一套
//!
//! 手写段的排版参数（`theme::FIELD_WIDTH`、`FIELD_HEIGHT`、`axis_field_width`、
//! 标签列宽、`row()` 前面那截轴标占位）是**同一套视觉语法**被写了十遍。
//! 反射通道直接复用 `inspector::row`，因此两边的行高、控件宽度、左边缘对齐
//! 天然一致 —— `--ui-reflect-inspector` 的逐像素对比就是验证这一点。
//!
//! # 与 `Inspect` 通道的关系
//!
//! [`show_behavior_fields`](super::inspector) 走的是 `Inspect` + `FieldValue`
//! （插件组件，值要落盘到 `behavior_params`）。这里走 `Reflect` + `ValueRef`
//! （内置组件，值就是 `Entity` 的字段本身）。两条通道**排版不同**是刻意的：
//! 插件组件的字段表是扁平的，画法也就只能是扁平的。
//!
//! # 写入路径
//!
//! 控件作用在**局部副本**上，改动了才写回组件。原因不是性能，是借用：
//! `ValueRefMut` 同时只能存在一个，而"同排的两个数值框"（`min` / `max` 距离）
//! 天然要同时拿两个 `&mut`。先读后写还有第二个好处 —— 写回只发生在真的有改动
//! 的字段上，不会把没动过的字段重写一遍（那样会让脏检查误报）。

use xgame_core::reflect::{
    FieldMeta, MetaKind, Reflect, ScalarKind, ScalarValue, TypeMeta, TypeRef, ValueRef, ValueRefMut,
};
use xgame_core::{BuiltinComponent, ComponentAddress, Entity};

use super::inspector::{
    component_context_menu, row, ComponentAction, ComponentMenuTarget, FieldSignals,
    PLAY_STRUCTURE_HINT,
};
use super::theme;

/// 给响应挂上字段说明（`doc`），空的就不挂。
///
/// 单独一个函数是因为 `Response::on_hover_text` **消费** `self`：写成
/// `response.on_hover_text(...)` 而不重新绑定，后面就用不了 `response` 了。
fn with_doc(response: egui::Response, doc: &str) -> egui::Response {
    if doc.is_empty() {
        response
    } else {
        response.on_hover_text(doc)
    }
}

/// 一个内置组件在反射通道里需要的**全部**非通用信息。
///
/// 只有四类：标题文案、怎么拿到 `&mut dyn Reflect`、编辑后要维护的引擎不变式、
/// 移除菜单的文字。字段怎么画、画哪些，一概不在这里 —— 那是元数据的事。
///
/// 这就是"统一组件"的边界所在：每个新组件仍然要写一条 spec，但它只有不到
/// 十行，而且**不含任何控件代码**。手写 section 是一百多行，且每一行都要跟着
/// 视觉语法走。
pub struct ReflectComponentSpec {
    pub kind: BuiltinComponent,
    /// 分类条标题（`"Audio"` 与 `BuiltinComponent::name()` 的 `"Audio Source"`
    /// 历史上就不一致，这里以 Inspector 上的文案为准）。
    pub title: &'static str,
    /// "勾上才有"那个复选框的文字。
    pub toggle: &'static str,
    pub toggle_hint: &'static str,
    /// 标题右键里"移除"那一项的文字。
    pub remove_label: &'static str,
    /// 取组件的可写反射视图；组件不存在时返回 `None`。
    pub access: fn(&mut Entity) -> Option<&mut dyn Reflect>,
    /// 编辑后修复**跨字段**不变式（`far > near`）。
    ///
    /// 为什么放在这里而不是元数据里：这些规则是引擎语义（一个 `far <= near`
    /// 的相机整屏全黑），不是显示偏好。用元数据表达只能造一门 DSL
    /// （`max_from = "near" + 0.001`），而规则本身一共只有两条。
    ///
    /// 注意它**在任意字段改动后**都会跑，不只在那两个相关字段上 —— 于是
    /// "改 Volume 顺手把坏的 max ≤ min 修好"这种情况也会发生。这是有意的：
    /// 不变式成立比"只动我碰的那一格"更重要。
    pub normalize: Option<fn(&mut Entity)>,
}

/// Camera 的 spec。
pub static CAMERA: ReflectComponentSpec = ReflectComponentSpec {
    kind: BuiltinComponent::Camera,
    title: "Camera",
    toggle: "Has camera",
    toggle_hint: "勾上之后这个实体就是一台相机：\nPlay 态视口 / 独立游戏 / 无头截图都从它取机位",
    remove_label: "Remove Camera",
    access: |entity| match entity.camera.as_mut() {
        Some(camera) => Some(camera),
        None => None,
    },
    normalize: Some(|entity| {
        // 复用引擎自己的钳制规则，不在这里手抄第二份边界 ——
        // 手写段抄了两份（一处挡 UI、一处给 `CameraView`），那正是漂移的来源。
        if let Some(camera) = entity.camera.as_mut() {
            *camera = camera.sanitized();
        }
    }),
};

/// AudioSource 的 spec。
pub static AUDIO: ReflectComponentSpec = ReflectComponentSpec {
    kind: BuiltinComponent::Audio,
    title: "Audio",
    toggle: "Has audio source",
    toggle_hint: "挂一个音频源：autoplay 进场景就响；\n非 autoplay 的剪辑由脚本 play_sound 触发",
    remove_label: "Remove Audio Source",
    access: |entity| match entity.audio.as_mut() {
        Some(audio) => Some(audio),
        None => None,
    },
    normalize: Some(|entity| {
        if let Some(audio) = entity.audio.as_mut() {
            // 引擎侧要求 max > min（否则衰减曲线退化），编辑器先把住门。
            if audio.max_distance <= audio.min_distance {
                audio.max_distance = audio.min_distance + 0.001;
            }
        }
    }),
};

/// 画一个内置组件的整段（分类条 + "勾上才有" + 全部字段）。
///
/// 调用方负责先判断组件是否存在（`BuiltinComponent::present`）—— 这一步与
/// `show_body` 里其余手写段保持同一种结构，读起来不会有"这里为什么不一样"。
pub(super) fn show_section(
    state: &crate::state::EditorState,
    ui: &mut egui::Ui,
    entity: &mut Entity,
    spec: &ReflectComponentSpec,
    sig: &mut FieldSignals,
    request: &mut Option<ComponentAction>,
    refresh_scripts: &mut bool,
) {
    let editing = state.is_editing();

    let header = theme::section_bar(ui, spec.title);
    component_context_menu(
        &header,
        state,
        entity,
        ComponentMenuTarget::Removable {
            address: ComponentAddress::Builtin(spec.kind),
            label: spec.remove_label,
        },
        request,
        refresh_scripts,
    );

    ui.indent(spec.kind.code(), |ui| {
        // 取消勾选 = 移除组件。勾选框的初值恒为 `true`：能走到这里就说明组件在。
        let mut enabled = true;
        if ui
            .add_enabled(editing, egui::Checkbox::new(&mut enabled, spec.toggle))
            .on_hover_text(spec.toggle_hint)
            .on_disabled_hover_text(PLAY_STRUCTURE_HINT)
            .changed()
        {
            *request = Some(ComponentAction::Remove(ComponentAddress::Builtin(spec.kind)));
        }

        let mut changed = false;
        if let Some(component) = (spec.access)(entity) {
            changed = show_fields(ui, component, sig);
        }
        // `component` 的借用必须在这里结束 —— `normalize` 要独占 `entity`。
        if changed {
            if let Some(normalize) = spec.normalize {
                normalize(entity);
            }
        }
    });
}

/// 按元数据顺序画全部可见字段，返回"本帧是否有字段被改"。
fn show_fields(ui: &mut egui::Ui, component: &mut dyn Reflect, sig: &mut FieldSignals) -> bool {
    let meta: &'static TypeMeta = component.type_meta();
    let mut changed = false;
    let mut i = 0usize;
    while i < meta.fields.len() {
        let field = &meta.fields[i];
        i += 1;
        if field.flags.hidden || !is_visible(&*component, field) {
            continue;
        }
        match field.row_group {
            Some(_group) => {
                // 只合并**连续**的同组字段：不连续还合并的话，渲染顺序会与声明
                // 顺序脱节，读代码的人得在元数据里来回跳才能预测界面。
                let mut members: Vec<&'static FieldMeta> = vec![field];
                while i < meta.fields.len() {
                    let next = &meta.fields[i];
                    if next.row_group != field.row_group {
                        break;
                    }
                    i += 1;
                    if next.flags.hidden || !is_visible(&*component, next) {
                        continue;
                    }
                    members.push(next);
                }
                changed |= show_group_row(ui, component, &members, sig);
            }
            None => changed |= show_field_row(ui, component, field, sig),
        }
    }
    changed
}

/// `visible_when` 的判定：同级 `bool` 字段为 `true` 才显示。
fn is_visible(component: &dyn Reflect, field: &FieldMeta) -> bool {
    let sibling = field
        .visible_when
        .and_then(|name| component.get(name))
        .and_then(|value| value.as_bool());
    field.visible_given(sibling)
}

/// 一个字段按类型分派到具体控件。
fn show_field_row(
    ui: &mut egui::Ui,
    component: &mut dyn Reflect,
    field: &'static FieldMeta,
    sig: &mut FieldSignals,
) -> bool {
    match field.ty {
        TypeRef::Scalar(kind) => show_scalar_row(ui, component, field, kind, sig),
        TypeRef::Named(meta) if meta().kind == MetaKind::Enum => {
            show_enum_row(ui, component, field, meta(), sig)
        }
        other => {
            // 没有通用编辑器的字段（嵌套结构 / 列表）：把已有的东西画成一行说明，
            // 而不是留一个空白 —— 空白会被误读成"这个字段不存在"。
            row(ui, field.display_name(), |ui| {
                ui.label(theme::dim_note(format!(
                    "{}（还没有通用编辑器）",
                    other.type_name()
                )));
            });
            false
        }
    }
}

/// 标量控件。
fn show_scalar_row(
    ui: &mut egui::Ui,
    component: &mut dyn Reflect,
    field: &'static FieldMeta,
    kind: ScalarKind,
    sig: &mut FieldSignals,
) -> bool {
    match kind {
        // 布尔走整行勾选框（不带标签列）：与手写段的 `ui.checkbox(&mut x, "Loop")` 同款。
        ScalarKind::Bool => {
            let mut value = component
                .get(field.name)
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let response = with_doc(ui.checkbox(&mut value, field.display_name()), field.doc);
            sig.collect(&response);
            if response.changed() {
                write(component, field.name, ScalarValue::Bool(value));
                return true;
            }
            false
        }
        ScalarKind::String => {
            let mut text = component
                .get(field.name)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let response = row(ui, field.display_name(), |ui| {
                let mut edit = egui::TextEdit::singleline(&mut text);
                if let Some(hint) = field.hint {
                    edit = edit.hint_text(hint);
                }
                // `+ 2.0` 与 Name 那行一致：单行输入框比数值框矮 2px 会显得塌。
                ui.add_sized(
                    egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
                    edit,
                )
            });
            let response = with_doc(response, field.doc);
            sig.collect(&response);
            if response.changed() {
                write(component, field.name, ScalarValue::Str(text));
                return true;
            }
            false
        }
        ScalarKind::Vec2 | ScalarKind::Vec3 | ScalarKind::Vec4 => {
            show_vector_row(ui, component, field, kind, sig)
        }
        _ => show_number_row(ui, component, field, sig),
    }
}

/// 单分量数值框。
///
/// 宏而不是六段复制：`DragValue` 是泛型的，六个数值类型各自要一个 `&mut T`，
/// 类型一换其余三行（speed / range / decimals）完全一样。
macro_rules! number_arm {
    ($ui:expr, $component:expr, $field:expr, $sig:expr, $ref_variant:ident, $ty:ty, $scalar:ident, $kind:expr) => {{
        let field = $field;
        let mut value: $ty = match $component.get(field.name) {
            Some(ValueRef::$ref_variant(v)) => *v,
            _ => <$ty>::default(),
        };
        let response = row($ui, field.display_name(), |ui| {
            let mut drag = egui::DragValue::new(&mut value);
            drag = drag.speed(field.speed.map_or(default_speed($kind), |s| s as f64));
            if let Some((lo, hi)) = field.range {
                // 浮点 → 整数用 `as`：Rust 的浮点转整型是**饱和**的，
                // 负数变 0、超界变 MAX，正好是"钳住"而不是回绕成天文数字。
                drag = drag.range((lo as $ty)..=(hi as $ty));
            }
            if let Some(decimals) = field.decimals {
                drag = drag.fixed_decimals(decimals as usize);
            }
            let response = ui.add_sized(
                egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
                drag,
            );
            if let Some(note) = field.note {
                ui.label(theme::dim_note(note));
            }
            response
        });
        let response = with_doc(response, field.doc);
        $sig.collect(&response);
        if response.changed() {
            write($component, field.name, ScalarValue::$scalar(value));
            return true;
        }
        return false;
    }};
}

fn show_number_row(
    ui: &mut egui::Ui,
    component: &mut dyn Reflect,
    field: &'static FieldMeta,
    sig: &mut FieldSignals,
) -> bool {
    // 分派依据是**字段声明的类型**（`cur` 的变体只用来取值）：
    // 元数据说这是 `u32`，控件就按 `u32` 编辑，避免"显示是浮点、写回是整数"。
    match field.ty {
        TypeRef::Scalar(ScalarKind::F32) => {
            number_arm!(ui, component, field, sig, F32, f32, F32, ScalarKind::F32)
        }
        TypeRef::Scalar(ScalarKind::F64) => {
            number_arm!(ui, component, field, sig, F64, f64, F64, ScalarKind::F64)
        }
        TypeRef::Scalar(ScalarKind::I32) => {
            number_arm!(ui, component, field, sig, I32, i32, I32, ScalarKind::I32)
        }
        TypeRef::Scalar(ScalarKind::I64) => {
            number_arm!(ui, component, field, sig, I64, i64, I64, ScalarKind::I64)
        }
        TypeRef::Scalar(ScalarKind::U32) => {
            number_arm!(ui, component, field, sig, U32, u32, U32, ScalarKind::U32)
        }
        TypeRef::Scalar(ScalarKind::U64) => {
            number_arm!(ui, component, field, sig, U64, u64, U64, ScalarKind::U64)
        }
        _ => false,
    }
}

/// 数值框在元数据没写 `speed` 时的兜底灵敏度。
///
/// 分两类而不是一律 0.1：`f32` 拖一下想动 0.1，而整数拖一下动 0.1 只会
/// **一格不动**（向下取整），看起来像控件坏了。
fn default_speed(kind: ScalarKind) -> f64 {
    match kind {
        ScalarKind::I32 | ScalarKind::I64 | ScalarKind::U32 | ScalarKind::U64 => 1.0,
        _ => 0.1,
    }
}

/// 多分量向量：轴标 + 每轴一个数值框。
///
/// 与 Transform 的三行同款（轴标彩色、框宽 `FIELD_WIDTH`）。S1 的两个组件里
/// 没有向量字段，所以这段还没被逐像素验证过 —— 它的第一个真实用例是 S2 的
/// `CollisionShape::Cuboid`。
fn show_vector_row(
    ui: &mut egui::Ui,
    component: &mut dyn Reflect,
    field: &'static FieldMeta,
    kind: ScalarKind,
    sig: &mut FieldSignals,
) -> bool {
    let (mut axes, dims) = match component.get(field.name) {
        Some(ValueRef::Vec2(v)) => ([v.x, v.y, 0.0, 0.0], 2usize),
        Some(ValueRef::Vec3(v)) => ([v.x, v.y, v.z, 0.0], 3),
        Some(ValueRef::Vec4(v)) => ([v.x, v.y, v.z, v.w], 4),
        _ => ([0.0; 4], kind.components()),
    };
    let speed = field.speed.map_or(0.1, |s| s as f64);
    let mut local_changed = false;
    row(ui, field.display_name(), |ui| {
        for (index, axis) in ["X", "Y", "Z", "W"].iter().take(dims).enumerate() {
            ui.label(
                egui::RichText::new(*axis)
                    .color(theme::axis_color(axis))
                    .size(10.0),
            );
            let mut drag = egui::DragValue::new(&mut axes[index]).speed(speed);
            if let Some(decimals) = field.decimals {
                drag = drag.fixed_decimals(decimals as usize);
            }
            let response = ui.add_sized(egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT), drag);
            sig.collect(&response);
            local_changed |= response.changed();
        }
        if let Some(note) = field.note {
            ui.label(theme::dim_note(note));
        }
    });
    if !local_changed {
        return false;
    }
    let value = match kind {
        ScalarKind::Vec2 => ScalarValue::Vec2(xgame_math::glam::Vec2::new(axes[0], axes[1])),
        ScalarKind::Vec4 => ScalarValue::Vec4(xgame_math::glam::Vec4::new(
            axes[0], axes[1], axes[2], axes[3],
        )),
        _ => ScalarValue::Vec3(xgame_math::glam::Vec3::new(axes[0], axes[1], axes[2])),
    };
    write(component, field.name, value);
    true
}

/// 无载荷枚举：下拉框，显示名取 `VariantMeta::display_name`。
fn show_enum_row(
    ui: &mut egui::Ui,
    component: &mut dyn Reflect,
    field: &'static FieldMeta,
    meta: &'static TypeMeta,
    sig: &mut FieldSignals,
) -> bool {
    let current = match component.get(field.name) {
        Some(ValueRef::Enum(v)) => v.variant(),
        _ => return false,
    };
    let mut picked: Option<&'static str> = None;
    let response = row(ui, field.display_name(), |ui| {
        egui::ComboBox::from_id_salt(format!("reflect_{}_{}", meta.name, field.name))
            .selected_text(variant_label(meta, current))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for variant in meta.variants {
                    if ui
                        .selectable_label(variant.name == current, variant.display_name())
                        .clicked()
                    {
                        picked = Some(variant.name);
                    }
                }
            })
            .response
    });
    let _ = with_doc(response, field.doc);
    let Some(picked) = picked else {
        return false;
    };
    if let Some(ValueRefMut::Enum(value)) = component.get_mut(field.name) {
        value.set_variant(picked);
        // 下拉框的改动不进 `Response::changed`，见 `FieldSignals::mark_changed`。
        sig.mark_changed();
        return true;
    }
    false
}

/// 变体的显示名。找不到时退回寻址名 —— 那说明 `ReflectEnum` 与元数据脱节，
/// 界面上显示 `Bgm` 也比显示空白强。
fn variant_label(meta: &'static TypeMeta, name: &'static str) -> &'static str {
    // 元数据里查不到这个变体 = `ReflectEnum` 与元数据脱节。这里不 panic
    // （编辑器不该因为一个下拉框崩），退回寻址名显示 —— 比显示空白强。
    meta.variant(name)
        .map_or(name, |variant| variant.display_name())
}

/// 同排的一组字段（`min` → `max` 那种成对区间）。
///
/// 组内字段先读到局部副本、画完再逐个写回：同时持有两个 `ValueRefMut` 是
/// `ValueRefMut` 的借用模型不允许的（见 `reflect::value` 的模块文档）。
fn show_group_row(
    ui: &mut egui::Ui,
    component: &mut dyn Reflect,
    members: &[&'static FieldMeta],
    sig: &mut FieldSignals,
) -> bool {
    // 只有"单分量浮点"能同排：整型塞进 `f32` 会丢精度（2^24 以上），
    // 而成对区间在引擎里全是浮点。遇到别的形状就退回逐行，宁可不合并也不画错。
    let pairable = members.iter().all(|field| {
        matches!(
            field.ty,
            TypeRef::Scalar(ScalarKind::F32 | ScalarKind::F64)
        )
    });
    if members.len() < 2 || !pairable {
        let mut changed = false;
        for field in members.iter().copied() {
            changed |= show_field_row(ui, component, field, sig);
        }
        return changed;
    }

    let label = members[0].display_name();
    let mut values: Vec<f32> = members
        .iter()
        .map(|field| {
            component
                .get(field.name)
                .and_then(|value| value.as_f32())
                .unwrap_or_default()
        })
        .collect();
    let mut touched = vec![false; members.len()];

    row(ui, label, |ui| {
        for (index, field) in members.iter().enumerate() {
            if index > 0 {
                ui.label(theme::dim_note("→"));
            }
            let mut drag = egui::DragValue::new(&mut values[index])
                .speed(field.speed.map_or(default_speed(ScalarKind::F32), |s| s as f64));
            if let Some((lo, hi)) = field.range {
                drag = drag.range(lo..=hi);
            }
            if let Some(decimals) = field.decimals {
                drag = drag.fixed_decimals(decimals as usize);
            }
            let response =
                ui.add_sized(egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT), drag);
            sig.collect(&response);
            touched[index] = response.changed();
        }
    });

    // 组尾的 `note` 整行画在下面而不是塞进本行：一行两个数值框再加一截中文
    // 说明会顶出面板宽度（320px 面板下 Min/Max 加说明已经放不下）。
    if let Some(note) = members.last().and_then(|field| field.note) {
        theme::note(ui, note);
    }

    let mut changed = false;
    for (index, field) in members.iter().enumerate() {
        if !touched[index] {
            continue;
        }
        let value = match field.ty {
            TypeRef::Scalar(ScalarKind::F64) => ScalarValue::F64(values[index] as f64),
            _ => ScalarValue::F32(values[index]),
        };
        write(component, field.name, value);
        changed = true;
    }
    changed
}

/// 把值写回组件。类型不匹配只可能来自"元数据与 `Reflect` 实现脱节"，
/// 那是编程错误而不是用户输入错误 —— 记一条日志，不打断编辑。
fn write(component: &mut dyn Reflect, field: &str, value: ScalarValue) {
    match component.get_mut_checked(field) {
        Ok(mut slot) => {
            if let Err(err) = slot.set(&value) {
                log::warn!("反射写回失败：{field} —— {err}");
            }
        }
        Err(err) => log::warn!("反射取不到可写字段：{field} —— {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::reflect::ReflectMeta;
    use xgame_core::{Camera, Entity};

    /// 两个 spec 的 `access` 必须真的拿到组件、且拿到的是**同一个**值 ——
    /// 写错了 `Some(...)` 的分支会让 Inspector 画出一个永远改不动的副本。
    #[test]
    fn spec_access_reads_and_writes_the_real_component() {
        let mut entity = Entity::new("probe");
        entity.camera = Some(Camera::default());
        {
            let view = (CAMERA.access)(&mut entity).expect("camera 应在");
            assert_eq!(view.type_meta().name, "Camera");
            write(view, "fov", ScalarValue::F32(45.0));
        }
        assert_eq!(entity.camera.as_ref().unwrap().fov, 45.0);

        // 组件不存在时必须返回 `None`（否则会画出空 section）。
        let mut bare = Entity::new("bare");
        assert!((CAMERA.access)(&mut bare).is_none());
        assert!((AUDIO.access)(&mut bare).is_none());
    }

    #[test]
    fn camera_normalize_keeps_far_beyond_near() {
        let mut entity = Entity::new("probe");
        entity.camera = Some(Camera {
            near: 10.0,
            far: 1.0,
            ..Camera::default()
        });
        (CAMERA.normalize.unwrap())(&mut entity);
        let camera = entity.camera.unwrap();
        assert!(camera.far > camera.near, "far = {}", camera.far);
    }

    #[test]
    fn audio_normalize_keeps_max_above_min() {
        let mut entity = Entity::new("probe");
        entity.audio = Some(xgame_core::AudioSource {
            min_distance: 20.0,
            max_distance: 5.0,
            ..xgame_core::AudioSource::default()
        });
        (AUDIO.normalize.unwrap())(&mut entity);
        let audio = entity.audio.unwrap();
        assert!(audio.max_distance > audio.min_distance);
    }

    /// `visible_when` 必须让距离区间跟着 `spatial` 走 —— 这是 S1 里唯一的
    /// 条件显示，也是"元数据能表达布局"的最小证明。
    #[test]
    fn distance_fields_hide_when_not_spatial() {
        use xgame_core::AudioSource;
        let min = AudioSource::meta().field("min_distance").unwrap();
        let off = AudioSource {
            spatial: false,
            ..AudioSource::default()
        };
        let on = AudioSource {
            spatial: true,
            ..AudioSource::default()
        };
        assert!(!is_visible(&off, min));
        assert!(is_visible(&on, min));
    }

    /// 组标签取组内第一个字段的显示名，组尾取最后一个字段的 `note`。
    #[test]
    fn distance_group_is_a_pair_with_one_caption() {
        use xgame_core::AudioSource;
        let meta = AudioSource::meta();
        let min = meta.field("min_distance").unwrap();
        let max = meta.field("max_distance").unwrap();
        assert_eq!(min.row_group, max.row_group);
        assert_eq!(min.display_name(), "Distance");
        assert!(min.note.is_none(), "说明只该挂一个字，否则会画两遍");
        assert!(max.note.is_some());
    }

    /// 无头跑一遍全部字段，抓"画的时候才炸"的问题：id 冲突、借用顺序违规、
    /// 元数据与 `Reflect` 实现不匹配导致的分支走错。
    ///
    /// 断言形状数量而不是截图：这里要守的是"渲染能跑通且条件显示真的少画了东西"，
    /// 逐像素一致由 `--ui-reflect-inspector` + `pngdiff.py` 负责（那需要 GPU）。
    #[test]
    fn renders_headlessly_and_hides_gated_rows() {
        let ctx = egui::Context::default();
        crate::panels::theme::install_fonts(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 900.0),
            )),
            ..Default::default()
        };

        let make = |spatial: bool| xgame_core::AudioSource {
            clip: "Resources/Audio/Beep.wav".into(),
            spatial,
            ..xgame_core::AudioSource::default()
        };

        let count = |component: &mut dyn Reflect| -> usize {
            let mut shapes = 0usize;
            // 跑两帧：egui 第一帧分配 id、第二帧才稳定，只跑一帧会漏掉
            // "第二帧才出现的 id 冲突"。
            for _ in 0..2 {
                let output = ctx.run(input.clone(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut sig = FieldSignals::default();
                        show_fields(ui, component, &mut sig);
                    });
                });
                shapes = output.shapes.len();
            }
            shapes
        };

        let with_distance = count(&mut make(true));
        let without_distance = count(&mut make(false));
        assert!(
            without_distance < with_distance,
            "关掉 spatial 后必须少画两行（实际 {without_distance} vs {with_distance}）"
        );

        assert!(count(&mut Camera::default()) > 0);
    }
}
