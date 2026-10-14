//! 组件字段的检视契约（P1）——**一份元数据，两个消费者**。
//!
//! `#[derive(Inspect)]`（见 `xgame-derive`）为组件生成一张静态字段表，然后：
//!
//! - **编辑器**照着表在 Inspector 里画控件（滑块 / 数框 / 复选框 / 文本 / 向量），
//!   并把用户输入经 [`Inspect::inspect_set`] 写回组件；
//! - **Lua 宿主**（P2）照着同一张表把组件字段暴露成脚本参数 —— 每帧多写几个
//!   字段是它的日常，而表是 `'static` 的，零分配、零查表。
//!
//! # 为什么是"表 + 下标"而不是回调
//!
//! 检视对象活在**插件 DLL 里**（`Box<dyn Inspect>` 由插件侧构造）。跨这条边界
//! 传闭包要处理 panic 隔离与函数指针生命周期，而"读第 i 个字段 / 写第 i 个字段"
//! 只是两个对象安全的方法 —— 下标与表一一对应，热路径（每帧每字段一次）上
//! 连一次哈希都没有。字段数通常是个位数，线性扫描在名字查找里也够快。
//!
//! # 值的取舍：`FieldValue` 是**拥有**的
//!
//! `inspect_get` 返回拥有值而不是引用：字段里可能有 `String`，借用版需要
//! `FieldValueRef<'_>` 与 `Cow`，把 API 复杂度推给每一个消费者。每帧每可见字段
//! 一次小分配可以接受（Inspector 只读**可见**的组件，不是全场景）。
//! 真到了要优化的那天，加一个 `inspect_get_into(&self, i, &mut FieldValue)` 即可，
//! 表结构不动。

use glam::{Vec2, Vec3, Vec4};

/// 字段的类型分类。
///
/// 它同时决定两件事：编辑器画什么控件、Lua 侧把这个字段当什么类型绑定。
/// 所以它是**契约**而不是提示 —— `FieldValue` 的变体必须与之一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    F32,
    F64,
    I32,
    I64,
    U32,
    Bool,
    String,
    Vec2,
    Vec3,
    Vec4,
}

impl FieldKind {
    /// 该类型的零值 —— Lua 侧给参数兜底、测试造默认值都用它。
    pub fn default_value(self) -> FieldValue {
        match self {
            FieldKind::F32 => FieldValue::F32(0.0),
            FieldKind::F64 => FieldValue::F64(0.0),
            FieldKind::I32 => FieldValue::I32(0),
            FieldKind::I64 => FieldValue::I64(0),
            FieldKind::U32 => FieldValue::U32(0),
            FieldKind::Bool => FieldValue::Bool(false),
            FieldKind::String => FieldValue::String(String::new()),
            FieldKind::Vec2 => FieldValue::Vec2(Vec2::ZERO),
            FieldKind::Vec3 => FieldValue::Vec3(Vec3::ZERO),
            FieldKind::Vec4 => FieldValue::Vec4(Vec4::ZERO),
        }
    }

    /// 这个类型是否是"可拖拽的数值"（编辑器据此决定用拖拽值还是别的控件）。
    pub fn is_numeric(self) -> bool {
        !matches!(
            self,
            FieldKind::Bool | FieldKind::String
        )
    }
}

/// 一个可检视字段的静态描述。
///
/// 全部字段都是 `Copy` + `'static`：整张表是编译期常量，装配期零成本。
/// `#[derive(Inspect)]` 负责填这张表，`#[inspect(...)]` 属性覆盖默认值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldDesc {
    /// 字段名（默认 = Rust 字段名）。Lua 侧按它寻址。
    pub name: &'static str,
    /// 类型分类：决定控件与绑定方式。
    pub kind: FieldKind,
    /// 显示名；`None` 时用 [`name`](Self::name)。改显示名不动寻址名，
    /// 于是"重命名 Rust 字段"不会破坏已有场景 / 脚本。
    pub label: Option<&'static str>,
    /// 数值下界（仅对 `is_numeric` 有意义）。
    pub min: Option<f32>,
    /// 数值上界。
    pub max: Option<f32>,
    /// 拖拽灵敏度（每像素变化量）。`None` = 编辑器按类型取默认值。
    pub speed: Option<f32>,
    /// 离散步长（Lua 侧的枚举步进、编辑器的 Shift 精调也用它）。
    pub step: Option<f32>,
    /// 单位后缀，如 `"°/s"`、`"m"`。空串 = 无。
    pub unit: &'static str,
    /// 悬停提示。空串 = 无。
    pub tooltip: &'static str,
}

impl FieldDesc {
    /// 构造一个只定了名字与类型的最小描述，其余走默认 —— derive 的兜底路径，
    /// 也让手写 `Inspect` 的成本降到两行。
    pub const fn new(name: &'static str, kind: FieldKind) -> Self {
        Self {
            name,
            kind,
            label: None,
            min: None,
            max: None,
            speed: None,
            step: None,
            unit: "",
            tooltip: "",
        }
    }

    /// 显示名（有 `label` 用它，否则用 `name`）。
    pub fn display_label(&self) -> &'static str {
        self.label.unwrap_or(self.name)
    }

    /// 这个字段是否有边界（编辑器据此决定画滑块还是裸拖拽值）。
    pub fn has_range(&self) -> bool {
        self.min.is_some() && self.max.is_some()
    }
}

/// 字段的运行期值 —— get/set 的载体，也是 Lua 侧的参数值类型。
///
/// 与 [`FieldKind`] 一一对应；[`kind`](Self::kind) 可以反查。
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    F32(f32),
    F64(f64),
    I32(i32),
    I64(i64),
    U32(u32),
    Bool(bool),
    String(String),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
}

impl FieldValue {
    /// 反查类型分类。
    pub fn kind(&self) -> FieldKind {
        match self {
            FieldValue::F32(_) => FieldKind::F32,
            FieldValue::F64(_) => FieldKind::F64,
            FieldValue::I32(_) => FieldKind::I32,
            FieldValue::I64(_) => FieldKind::I64,
            FieldValue::U32(_) => FieldKind::U32,
            FieldValue::Bool(_) => FieldKind::Bool,
            FieldValue::String(_) => FieldKind::String,
            FieldValue::Vec2(_) => FieldKind::Vec2,
            FieldValue::Vec3(_) => FieldKind::Vec3,
            FieldValue::Vec4(_) => FieldKind::Vec4,
        }
    }

    /// 尽力转成 `f32`（编辑器把向量拆成标量编辑、Lua 传数字给数值字段时用）。
    ///
    /// 转不了返回 `None` —— **不**静默变 0：写错类型的后果应当被看见。
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            FieldValue::F32(v) => Some(*v),
            FieldValue::F64(v) => Some(*v as f32),
            FieldValue::I32(v) => Some(*v as f32),
            FieldValue::I64(v) => Some(*v as f32),
            FieldValue::U32(v) => Some(*v as f32),
            _ => None,
        }
    }

    /// 供控制台 / 日志显示的一行摘要（不追求完整，追求"看得出是什么"）。
    pub fn display(&self) -> String {
        match self {
            FieldValue::F32(v) => format!("{v}"),
            FieldValue::F64(v) => format!("{v}"),
            FieldValue::I32(v) => format!("{v}"),
            FieldValue::I64(v) => format!("{v}"),
            FieldValue::U32(v) => format!("{v}"),
            FieldValue::Bool(v) => format!("{v}"),
            FieldValue::String(v) => format!("{v:?}"),
            FieldValue::Vec2(v) => format!("({}, {})", v.x, v.y),
            FieldValue::Vec3(v) => format!("({}, {}, {})", v.x, v.y, v.z),
            FieldValue::Vec4(v) => format!("({}, {}, {}, {})", v.x, v.y, v.z, v.w),
        }
    }
}

/// 可在编辑器里检视 / 调参的组件。
///
/// [`xgame::prelude::Lifecycle`] 是组件的另一半契约（四钩子），两者合起来是
/// [`Behavior`]（`xgame-runtime`）。不要手写这个 trait —— `#[derive(Inspect)]`
/// 会生成表与下标映射，手写极易与表错位（错位的表现是"改 A 字段动了 B 字段"）。
pub trait Inspect {
    /// 字段描述表（**静态**，编译期常量）。
    ///
    /// 这张表同时是编辑器与 Lua 宿主的真相源 —— 加一个字段、改一处属性，
    /// 两边自动同步。
    fn inspect_fields(&self) -> &'static [FieldDesc];

    /// 读第 `index` 个字段（与 [`inspect_fields`](Self::inspect_fields) 同序）。
    ///
    /// 越界返回零值而不是 panic：检视是"尽力而为"的旁路，不该让编辑器
    /// 因为一次越界整个崩掉。
    fn inspect_get(&self, index: usize) -> FieldValue;

    /// 写第 `index` 个字段。
    ///
    /// **类型不匹配时静默忽略**（不覆盖也不 panic）：调用方通常是从
    /// [`inspect_get`](Self::inspect_get) 拿到值再改一个分量写回来，
    /// 类型天然一致；而 Lua 侧传来错类型时，"这次写入没生效"比"组件里
    /// 多了个 NaN"安全得多。
    fn inspect_set(&mut self, index: usize, value: FieldValue);

    // -- 下面是给消费者（编辑器 / Lua）的便利方法，一般不需要覆写 --

    /// 一次拿到"第 i 个字段的描述 + 当前值"。Inspector 每帧每字段调一次。
    ///
    /// 描述是 `&'static`（表本来就是编译期常量）—— 借用它**不会**占用组件，
    /// 于是"读描述 → 画控件 → 写回组件"这一串不需要中途放开借用。
    fn inspect_field(&self, index: usize) -> Option<(&'static FieldDesc, FieldValue)> {
        let desc = self.inspect_fields().get(index)?;
        Some((desc, self.inspect_get(index)))
    }

    /// 按**字段名**写值（Lua 侧的入口；编辑器按下标走，不受此影响）。
    ///
    /// 线性扫描静态表 —— 字段通常个位数，不值得为它上哈希。名字不存在或
    /// 类型不匹配返回 `false`，调用方可据此报错给脚本作者。
    fn inspect_set_by_name(&mut self, name: &str, value: FieldValue) -> bool {
        let Some(index) = self.inspect_fields().iter().position(|f| f.name == name) else {
            return false;
        };
        let expected = self.inspect_fields()[index].kind;
        if value.kind() != expected {
            return false;
        }
        self.inspect_set(index, value);
        true
    }

    /// 按字段名读值（对称的另一半，脚本侧查询参数用）。
    fn inspect_get_by_name(&self, name: &str) -> Option<FieldValue> {
        let index = self.inspect_fields().iter().position(|f| f.name == name)?;
        Some(self.inspect_get(index))
    }
}

/// `Inspect` 的**空实现**：组件没有（或不打算暴露）任何可检视字段时用。
///
/// 比 `#[derive(Inspect)]` 轻得多，而且**不依赖门面 crate** —— 引擎自己的
/// 测试组件（`xgame-runtime` / `xgame` 的单测）用它，免得为了一个空表把
/// `xgame` 拖成 dev-dependency 的循环。插件作者正常情况下用 derive。
///
/// ```
/// use xgame_core::inspect::{FieldDesc, FieldKind, FieldValue, Inspect};
///
/// struct Internal;
/// xgame_core::impl_no_inspect!(Internal);
///
/// let mut c = Internal;
/// assert!(c.inspect_fields().is_empty());
/// // 空表的写入一律被忽略，不 panic（检视是旁路，不该拖垮宿主）。
/// c.inspect_set(0, FieldValue::Bool(true));
/// ```
#[macro_export]
macro_rules! impl_no_inspect {
    ($t:ty) => {
        impl $crate::inspect::Inspect for $t {
            fn inspect_fields(&self) -> &'static [$crate::inspect::FieldDesc] {
                &[]
            }
            fn inspect_get(&self, _index: usize) -> $crate::inspect::FieldValue {
                $crate::inspect::FieldKind::F32.default_value()
            }
            fn inspect_set(&mut self, _index: usize, _value: $crate::inspect::FieldValue) {}
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个**手写**实现的检视对象：绕开 derive 直接验证契约本身。
    ///
    /// 手写很笨，这正是"要 derive"的理由 —— 但契约必须在没有 derive 的
    /// 情况下也成立（比如将来有别的语言实现同一个 trait）。
    struct Handwritten {
        speed: f32,
        enabled: bool,
    }

    impl Inspect for Handwritten {
        fn inspect_fields(&self) -> &'static [FieldDesc] {
            static FIELDS: &[FieldDesc] = &[
                FieldDesc {
                    name: "speed",
                    kind: FieldKind::F32,
                    label: Some("Speed"),
                    min: Some(0.0),
                    max: Some(100.0),
                    speed: None,
                    step: None,
                    unit: "m/s",
                    tooltip: "每秒移动距离",
                },
                FieldDesc::new("enabled", FieldKind::Bool),
            ];
            FIELDS
        }

        fn inspect_get(&self, index: usize) -> FieldValue {
            match index {
                0 => FieldValue::F32(self.speed),
                1 => FieldValue::Bool(self.enabled),
                _ => FieldKind::F32.default_value(),
            }
        }

        fn inspect_set(&mut self, index: usize, value: FieldValue) {
            match (index, value) {
                (0, FieldValue::F32(v)) => self.speed = v,
                (1, FieldValue::Bool(v)) => self.enabled = v,
                _ => {}
            }
        }
    }

    #[test]
    fn desc_helpers_and_defaults() {
        let fields = Handwritten { speed: 0.0, enabled: false }.inspect_fields();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].display_label(), "Speed", "有 label 用 label");
        assert_eq!(fields[1].display_label(), "enabled", "没 label 用字段名");
        assert!(fields[0].has_range(), "min + max 齐全 ⇒ 有边界");
        assert!(!fields[1].has_range());
        assert!(FieldKind::Vec3.is_numeric());
        assert!(!FieldKind::Bool.is_numeric());
        assert!(!FieldKind::String.is_numeric());
    }

    #[test]
    fn roundtrip_get_set_and_name_lookup() {
        let mut c = Handwritten { speed: 1.5, enabled: false };

        // 下标读改写回
        let (desc, value) = c.inspect_field(0).expect("字段 0 存在");
        assert_eq!(desc.name, "speed");
        assert_eq!(value, FieldValue::F32(1.5));
        c.inspect_set(0, FieldValue::F32(9.0));
        assert_eq!(c.speed, 9.0);

        // 按名读改写回（Lua 侧的路径）
        assert_eq!(c.inspect_get_by_name("enabled"), Some(FieldValue::Bool(false)));
        assert!(c.inspect_set_by_name("enabled", FieldValue::Bool(true)));
        assert!(c.enabled);

        // 名字不存在 / 类型不匹配：明确返回 false，不静默改错东西
        assert!(!c.inspect_set_by_name("nope", FieldValue::F32(1.0)));
        assert!(!c.inspect_set_by_name("speed", FieldValue::Bool(true)));
        assert_eq!(c.speed, 9.0, "类型不匹配时值必须没被动过");
        assert_eq!(c.inspect_get_by_name("nope"), None);
    }

    #[test]
    fn set_with_wrong_variant_is_ignored_and_out_of_range_reads_default() {
        let mut c = Handwritten { speed: 3.0, enabled: true };

        // 错误变体写入：忽略（不覆盖、不 panic）
        c.inspect_set(0, FieldValue::String("oops".into()));
        assert_eq!(c.speed, 3.0);

        // 越界读：给零值而不是 panic —— 检视是旁路，不该拖垮编辑器
        assert_eq!(c.inspect_get(99), FieldKind::F32.default_value());
        assert!(c.inspect_field(99).is_none());

        // 零值与类型反查自洽
        for kind in [
            FieldKind::F32,
            FieldKind::F64,
            FieldKind::I32,
            FieldKind::I64,
            FieldKind::U32,
            FieldKind::Bool,
            FieldKind::String,
            FieldKind::Vec2,
            FieldKind::Vec3,
            FieldKind::Vec4,
        ] {
            assert_eq!(kind.default_value().kind(), kind, "{kind:?} 的零值类型必须自洽");
        }
    }

    #[test]
    fn display_is_human_readable() {
        assert_eq!(FieldValue::F32(1.5).display(), "1.5");
        assert_eq!(FieldValue::Bool(true).display(), "true");
        assert_eq!(FieldValue::Vec3(Vec3::new(1.0, 2.0, 3.0)).display(), "(1, 2, 3)");
        assert_eq!(FieldValue::String("hi".into()).display(), "\"hi\"");
        assert_eq!(FieldValue::F32(1.0).as_f32(), Some(1.0));
        assert_eq!(FieldValue::F64(2.0).as_f32(), Some(2.0));
        assert_eq!(FieldValue::Bool(true).as_f32(), None, "布尔转不了数字");
    }
}
