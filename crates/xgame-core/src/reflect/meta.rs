//! 类型元数据：编译期常量描述，derive 生成。
//!
//! # 为什么是 `fn() -> &'static TypeMeta` 而不是 `&'static TypeMeta`
//!
//! 嵌套字段要指向另一个类型的元数据。如果直接存 `&'static TypeMeta`，那张表必须在
//! **const 上下文**里构造（`static META: TypeMeta = TypeMeta { … }`），而跨类型引用
//! 只能写 `<T as ReflectMeta>::META` 这种关联常量。一旦类型图里出现递归
//! （A 含 B、B 含 A），关联常量会无限展开，编译器直接拒绝。
//!
//! 存函数指针把解析推迟到运行期第一次访问：函数体里的 `static` 是惰性初始化的，
//! 递归类型图也能成立。代价是每次解引用多一次函数调用 —— 这个开销出现在编辑器
//! 展开一行字段的时候，不在每帧循环里。
//!
//! 当前组件图**并没有**递归（`AnimLayer` 不回指 `AnimMachine`），但这条选择保留
//! 了将来的余地，且没有实际成本。

use glam::{Vec2, Vec3, Vec4};

/// 标量类型分类。
///
/// 相对 `inspect::FieldKind` **多了 `U64`**：`ParticleEmitter.seed` 是 `u64`，
/// 而 `FieldKind` 受磁盘契约约束（`FieldValue` 的变体名进了场景文件），
/// 不能为反射去加变体。两者刻意分开各管一件事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarKind {
    F32,
    F64,
    I32,
    I64,
    U32,
    U64,
    Bool,
    String,
    Vec2,
    Vec3,
    Vec4,
}

impl ScalarKind {
    /// 是否是可拖拽的数值（编辑器据此决定用数值框还是别的控件）。
    pub const fn is_numeric(self) -> bool {
        !matches!(self, Self::Bool | Self::String)
    }

    /// 是否是多分量的向量（三轴 / 四分量并排渲染）。
    pub const fn components(self) -> usize {
        match self {
            Self::Vec2 => 2,
            Self::Vec3 => 3,
            Self::Vec4 => 4,
            _ => 1,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::Bool => "bool",
            Self::String => "String",
            Self::Vec2 => "Vec2",
            Self::Vec3 => "Vec3",
            Self::Vec4 => "Vec4",
        }
    }
}

/// 字段的类型引用（递归）。
///
/// **不实现 `PartialEq`**：`Struct` / `Enum` 变体装的是函数指针，而函数指针的地址
/// 不保证唯一（同一个函数可能被合并成多个实例），比较结果没有意义 —— 编译器也会
/// 报警告。需要"是不是同一种类型"时比 `TypeRef::type_name()` 或直接比 `fn` 指针
/// 是设计错误，应该比类型本身。
#[derive(Debug, Clone, Copy)]
pub enum TypeRef {
    Scalar(ScalarKind),
    /// 指向另一个具名类型。**结构体还是枚举由目标 `TypeMeta.kind` 决定**，
    /// 这里不区分 —— derive 只看得到字段的类型名，看不到目标定义，
    /// 早期版本按 `Struct` / `Enum` 两个变体让宏去猜，结果把 `AudioBus`
    /// 猜成了结构体。不猜比猜错好。
    Named(fn() -> &'static TypeMeta),
    /// `Vec<T>`
    List(&'static TypeRef),
    /// `[T; N]`
    Array(&'static TypeRef, usize),
    /// `Option<T>`
    Option(&'static TypeRef),
}

impl TypeRef {
    /// 剥掉 `Option` 外壳，拿内层类型。
    pub const fn unwrap_option(&self) -> &TypeRef {
        match self {
            Self::Option(inner) => inner,
            other => other,
        }
    }

    /// 这个引用指向的类型名（用于报错与调试输出）。
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Scalar(k) => k.name(),
            Self::Named(f) => f().name,
            Self::List(_) => "Vec",
            Self::Array(_, _) => "array",
            Self::Option(inner) => inner.type_name(),
        }
    }

    /// 元素类型（`List` / `Array` / `Option` 的内层）；标量与结构体返回自身。
    pub const fn element(&self) -> Option<&TypeRef> {
        match self {
            Self::List(inner) | Self::Array(inner, _) | Self::Option(inner) => Some(inner),
            _ => None,
        }
    }
}

/// 字段可见性 / 可写性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldFlags {
    /// 不显示（但 `get_path` 仍可读 —— 它是"界面不画"，不是"数据不存在"）。
    pub hidden: bool,
    /// 只读：`get_mut` 返回 `None`。
    pub read_only: bool,
}

impl FieldFlags {
    pub const NONE: Self = Self {
        hidden: false,
        read_only: false,
    };
    pub const HIDDEN: Self = Self {
        hidden: true,
        read_only: false,
    };
    pub const READ_ONLY: Self = Self {
        hidden: false,
        read_only: true,
    };
}

/// 一个字段的静态描述。整张表是编译期常量。
#[derive(Debug, Clone, Copy)]
pub struct FieldMeta {
    /// 寻址名（等于 Rust 字段名）。`get_path` / MCP / Lua 都用它。
    pub name: &'static str,
    /// 显示名。`None` 时用 `name`。改它不影响寻址，所以"重命名显示"不会破坏已有脚本。
    pub label: Option<&'static str>,
    pub ty: TypeRef,
    /// 数值范围（仅对 `is_numeric` 有意义）。
    pub range: Option<(f32, f32)>,
    /// 拖拽灵敏度（每像素变化量）。
    pub speed: Option<f32>,
    /// 单位后缀（`"°"` / `"m/s"`）。
    pub unit: Option<&'static str>,
    pub flags: FieldFlags,
    /// 中文说明。编辑器用它画那一行小字。
    pub doc: &'static str,
}

impl FieldMeta {
    /// 界面显示用的名字。
    pub fn display_name(&self) -> &'static str {
        self.label.unwrap_or(self.name)
    }
}

/// 枚举变体的静态描述。
#[derive(Debug, Clone, Copy)]
pub struct VariantMeta {
    pub name: &'static str,
    pub doc: &'static str,
    /// 带载荷变体的载荷类型；无载荷为 `None`。
    pub payload: Option<TypeRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaKind {
    Struct,
    Enum,
}

/// 一个类型的完整元数据。
#[derive(Debug)]
pub struct TypeMeta {
    pub name: &'static str,
    pub doc: &'static str,
    pub kind: MetaKind,
    /// 结构体字段（`MetaKind::Struct`），按声明顺序。
    pub fields: &'static [FieldMeta],
    /// 枚举变体（`MetaKind::Enum`），按声明顺序。
    pub variants: &'static [VariantMeta],
}

impl TypeMeta {
    pub fn field(&self, name: &str) -> Option<&'static FieldMeta> {
        // 表是 `'static`，所以这里能直接返回 `'static` 引用。
        let fields: &'static [FieldMeta] = self.fields;
        fields.iter().find(|f| f.name == name)
    }

    pub fn variant(&self, name: &str) -> Option<&'static VariantMeta> {
        let variants: &'static [VariantMeta] = self.variants;
        variants.iter().find(|v| v.name == name)
    }

    /// 出现在界面上、且按声明顺序排列的字段。
    pub fn visible_fields(&self) -> impl Iterator<Item = &'static FieldMeta> {
        let fields: &'static [FieldMeta] = self.fields;
        fields.iter().filter(|f| !f.flags.hidden)
    }
}

/// 类型 → 静态元数据。由 `#[derive(Reflect)]` 或手写 impl 提供。
///
/// 与 [`Reflect`](super::Reflect) 分开的原因：这里的方法不是 `&self`，
/// 因此这个 trait **不是** dyn 兼容的；而 `dyn Reflect` 必须能取 `meta()`。
/// 两者职责不同，故意不设 supertrait 关系。
pub trait ReflectMeta {
    fn meta() -> &'static TypeMeta;
}

// ---------------------------------------------------------------------------
// 标量类型的元数据：没有字段，只用来表达"这是一个标量"。
// ---------------------------------------------------------------------------

macro_rules! scalar_meta {
    ($($ty:ty => $kind:expr,)*) => {
        $(
            impl ReflectMeta for $ty {
                fn meta() -> &'static TypeMeta {
                    // 标量在 `TypeRef` 里走 `Scalar` 分支，这张表只是为了满足
                    // 泛型约束（例如 `Vec<T>` 的 `ReflectList` 实现要求 `T: ReflectMeta`）。
                    const META: TypeMeta = TypeMeta {
                        name: $kind.name(),
                        doc: "",
                        kind: MetaKind::Struct,
                        fields: &[],
                        variants: &[],
                    };
                    &META
                }
            }
        )*
    };
}

scalar_meta! {
    f32 => ScalarKind::F32,
    f64 => ScalarKind::F64,
    i32 => ScalarKind::I32,
    i64 => ScalarKind::I64,
    u32 => ScalarKind::U32,
    u64 => ScalarKind::U64,
    bool => ScalarKind::Bool,
    String => ScalarKind::String,
    Vec2 => ScalarKind::Vec2,
    Vec3 => ScalarKind::Vec3,
    Vec4 => ScalarKind::Vec4,
}
