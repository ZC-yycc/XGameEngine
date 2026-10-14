//! XGameEngine 过程宏。
//!
//! 两个派生宏：
//!
//! - `#[derive(Panel)]` —— 为面板标签枚举生成 `show()`，把每个变体分发到
//!   `crate::panels::<蛇形命名>::show_content()`（编辑器内部用）。
//! - `#[derive(Inspect)]` —— 为组件生成字段检视表（P1），同时服务编辑器
//!   Inspector 与（P2）Lua 参数绑定。

use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_macro_input, DeriveInput};

/// 将大驼峰标识符转换为蛇形命名。
///
/// 示例：
/// - `Hierarchy`  → `hierarchy`
/// - `SceneView`  → `scene_view`
/// - `Inspector`  → `inspector`
/// - `Console`    → `console`
fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, ch) in name.char_indices() {
        if i > 0 && ch.is_uppercase() {
            result.push('_');
        }
        for c in ch.to_lowercase() {
            result.push(c);
        }
    }
    result
}

/// 派生宏，为面板标签枚举生成 `show()` 方法。
///
/// 每个变体被分发到 `crate::panels::<蛇形>::show_content()`。
///
/// # Example
///
/// ```ignore
/// #[derive(Panel)]
/// enum PanelTab {
///     Hierarchy,
///     SceneView,
///     Inspector,
///     Console,
/// }
/// ```
#[proc_macro_derive(Panel)]
pub fn panel_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    let variants = if let syn::Data::Enum(data) = input.data {
        data.variants.iter().map(|v| v.ident.clone()).collect::<Vec<_>>()
    } else {
        panic!("Panel can only be derived for enums");
    };

    let match_arms = variants.iter().map(|variant| {
        let snake = to_snake_case(&variant.to_string());
        // Build path: crate::panels::<snake>::show_content
        let path_str = format!("crate::panels::{}::show_content", snake);
        let path: syn::Path = syn::parse_str(&path_str)
            .expect("invalid module path generated from variant name");
        quote! {
            Self::#variant => #path(state, ui),
        }
    });

    let expanded = quote! {
        impl #name {
            /// Dispatch to the corresponding panel's `show_content`.
            pub fn show(&self, state: &mut crate::state::EditorState, ui: &mut egui::Ui) {
                match self {
                    #(#match_arms)*
                }
            }
        }
    };

    TokenStream::from(expanded)
}

// ---------------------------------------------------------------------------
// #[derive(Inspect)]
// ---------------------------------------------------------------------------

/// 派生宏，为组件生成 [`Inspect`] 实现 —— 编辑器 Inspector 与（P2）Lua
/// 参数绑定的**同一份字段表**。
///
/// ```ignore
/// use xgame::prelude::*;
///
/// #[derive(Inspect)]
/// pub struct Spinner {
///     #[inspect(min = 0.0, max = 720.0, speed = 10.0, unit = "°/s")]
///     pub speed: f32,
///     #[inspect(label = "Axis", tooltip = "旋转轴")]
///     pub axis: Vec3,
///     #[inspect(skip)]
///     cached: f64,   // 内部状态，不进编辑器
/// }
/// ```
///
/// # 属性
///
/// | 属性 | 作用 |
/// |---|---|
/// | `skip` | 字段不进表（内部状态 / 不想暴露） |
/// | `label = "..."` | 显示名 —— **不改**寻址名，重命名 Rust 字段不会破坏已有脚本 |
/// | `min = -1.0` / `max = 1.0` | 数值边界（两者齐全 ⇒ 编辑器画滑块而非裸拖拽） |
/// | `speed = 10.0` | 拖拽灵敏度 |
/// | `step = 0.5` | 离散步长 |
/// | `unit = "m/s"` | 单位后缀 |
/// | `tooltip = "..."` | 悬停提示 |
///
/// # 支持的字段类型
///
/// `f32` `f64` `i32` `i64` `u32` `bool` `String` `Vec2` `Vec3` `Vec4`。
/// 其他类型请标 `#[inspect(skip)]`（编译期报错并提示，不会静默漏掉字段）。
///
/// # 路径假设
///
/// 生成的代码引用 `::xgame::inspect::*`，所以**使用方必须依赖名为 `xgame`
/// 的门面 crate** —— 与 serde 默认引用 `::serde` 同一约定。引擎内部的测试组件
/// 不想引入这层依赖时，用 `xgame_core::impl_no_inspect!` 生成空实现。
///
/// # 为什么不该手写这个 trait
///
/// 表、`inspect_get`、`inspect_set` 是**三处必须对齐的手工下标**；一旦错位，
/// 表现是"改 A 字段动了 B 字段"，而编译器一声不吭（两边都是 `bool`/`usize`
/// 这种普通类型）。derive 从同一份字段清单生成三处，结构上不可能错位。
#[proc_macro_derive(Inspect, attributes(inspect))]
pub fn inspect_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_inspect(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `#[inspect(...)]` 解析出的一组属性（默认值 = 什么都不设）。
#[derive(Default)]
struct FieldAttr {
    skip: bool,
    label: Option<String>,
    min: Option<f32>,
    max: Option<f32>,
    speed: Option<f32>,
    step: Option<f32>,
    unit: Option<String>,
    tooltip: Option<String>,
}

/// 可被检视的字段类型：源码里的写法 → `FieldKind` / `FieldValue` 的变体名。
///
/// 只取**最后一个路径段**匹配，是为了让 `glam::Vec3` / `xgame_math::glam::Vec3` /
/// `Vec3` 三种写法都认；而 `Vec<T>`、`Option<f32>` 这类带泛型参数的一律不认
/// （泛型参数非空即拒，避免 `Foo<Vec3>` 这种误匹配）。
///
/// 两列**必须**分开：`f32` 是小写，而变体叫 `F32`。第一版把它们混成一列，
/// 生成了 `FieldKind::f32` —— 编译器报"no variant named `f32`"，
/// 位置却指向用户结构体（宏展开后的映射），不好一眼定位。
const INSPECTABLE: &[(&str, &str)] = &[
    ("f32", "F32"),
    ("f64", "F64"),
    ("i32", "I32"),
    ("i64", "I64"),
    ("u32", "U32"),
    ("bool", "Bool"),
    ("String", "String"),
    ("Vec2", "Vec2"),
    ("Vec3", "Vec3"),
    ("Vec4", "Vec4"),
];

/// 从 `syn::Type` 推出 `FieldKind` / `FieldValue` 共用的那个变体名。
fn type_kind(ty: &syn::Type) -> Option<&'static str> {
    let syn::Type::Path(tp) = ty else {
        return None;
    };
    let seg = tp.path.segments.last()?;
    if !matches!(seg.arguments, syn::PathArguments::None) {
        return None;
    }
    let id = seg.ident.to_string();
    INSPECTABLE
        .iter()
        .find(|(src, _)| *src == id)
        .map(|(_, variant)| *variant)
}

/// 读一个 `f32` 属性值。
///
/// 负号要单独认：`syn::Lit` 本身**不含符号**，`min = -1.0` 会被解析成
/// `UnOp::Neg` 包着字面量 —— 直接 `parse::<Lit>()` 会在这里报一个看不出
/// 原因的错误。所以先看一眼是不是 `-`，再读字面量。
///
/// 末尾的类型后缀（`1.0f32`）也要切掉：`to_token_stream()` 保留源码文本，
/// 而 `"1.0f32".parse::<f32>()` 是失败的。
fn parse_attr_f32(input: syn::parse::ParseStream) -> syn::Result<f32> {
    let neg = if input.peek(syn::Token![-]) {
        input.parse::<syn::Token![-]>()?;
        true
    } else {
        false
    };
    let lit: syn::Lit = input.parse()?;
    let raw = lit.to_token_stream().to_string();
    let end = raw
        .find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        })
        .unwrap_or(raw.len());
    let v: f32 = raw[..end]
        .parse()
        .map_err(|_| syn::Error::new(lit.span(), format!("`{raw}` 不是合法的数值字面量")))?;
    Ok(if neg { -v } else { v })
}

/// 解析一个字段上的 `#[inspect(...)]`。
fn parse_inspect_attr(attr: &syn::Attribute) -> syn::Result<FieldAttr> {
    let mut out = FieldAttr::default();
    attr.parse_nested_meta(|meta| {
        let path = meta.path.clone();
        if path.is_ident("skip") {
            out.skip = true;
            return Ok(());
        }
        if path.is_ident("label") {
            out.label = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            return Ok(());
        }
        if path.is_ident("unit") {
            out.unit = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            return Ok(());
        }
        if path.is_ident("tooltip") {
            out.tooltip = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            return Ok(());
        }
        if path.is_ident("min") {
            out.min = Some(parse_attr_f32(meta.value()?)?);
            return Ok(());
        }
        if path.is_ident("max") {
            out.max = Some(parse_attr_f32(meta.value()?)?);
            return Ok(());
        }
        if path.is_ident("speed") {
            out.speed = Some(parse_attr_f32(meta.value()?)?);
            return Ok(());
        }
        if path.is_ident("step") {
            out.step = Some(parse_attr_f32(meta.value()?)?);
            return Ok(());
        }
        Err(meta.error(
            "不认识的 `inspect` 属性；可用：skip / label / min / max / speed / step / unit / tooltip",
        ))
    })?;
    Ok(out)
}

/// `Option<f32>` → `Some(0.5f32)` / `None`（`FieldDesc` 的对应字段类型）。
fn opt_f32(v: Option<f32>) -> proc_macro2::TokenStream {
    match v {
        Some(v) => quote!(Some(#v)),
        None => quote!(None),
    }
}

/// 空表实现 —— 单元结构体、字段全是 `skip`、以及"一个字段都没剩"三种情形
/// 共用同一条路径。
fn empty_inspect(
    name: &syn::Ident,
    impl_g: syn::ImplGenerics<'_>,
    ty_g: syn::TypeGenerics<'_>,
    where_c: Option<&syn::WhereClause>,
) -> proc_macro2::TokenStream {
    quote! {
        impl #impl_g ::xgame::inspect::Inspect for #name #ty_g #where_c {
            fn inspect_fields(&self) -> &'static [::xgame::inspect::FieldDesc] {
                &[]
            }
            fn inspect_get(&self, _index: usize) -> ::xgame::inspect::FieldValue {
                ::xgame::inspect::FieldKind::F32.default_value()
            }
            fn inspect_set(&mut self, _index: usize, _value: ::xgame::inspect::FieldValue) {}
        }
    }
}

fn expand_inspect(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    // `DeriveInput` 自己没有 `split_for_impl`（syn 2），从 `generics` 上取。
    let (impl_g, ty_g, where_c) = input.generics.split_for_impl();

    // 只支持具名字段的结构体：表要靠字段名寻址（Lua 侧按名取值），
    // 元组结构体的 "0"/"1" 当名字会让脚本写起来非常别扭。
    let syn::Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            name,
            "`#[derive(Inspect)]` 只支持结构体（枚举与 union 无法用字段表描述）",
        ));
    };
    let fields = match &data.fields {
        syn::Fields::Named(named) => &named.named,
        syn::Fields::Unit => return Ok(empty_inspect(name, impl_g, ty_g, where_c)),
        syn::Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                name,
                "`#[derive(Inspect)]` 不支持元组结构体 —— 请改用具名字段",
            ));
        }
    };

    let mut descs: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut gets = Vec::new();
    let mut sets = Vec::new();
    let mut index: usize = 0;

    for field in fields {
        let ident = field.ident.as_ref().expect("具名字段必有 ident");
        let mut attr = FieldAttr::default();
        for a in &field.attrs {
            if a.path().is_ident("inspect") {
                attr = parse_inspect_attr(a)?;
            }
        }
        if attr.skip {
            continue;
        }

        let Some(kind) = type_kind(&field.ty) else {
            return Err(syn::Error::new_spanned(
                &field.ty,
                format!(
                    "字段 `{ident}` 的类型 `{}` 不能被检视 —— `#[derive(Inspect)]` \
                     只支持 f32/f64/i32/i64/u32/bool/String/Vec2/Vec3/Vec4；\
                     这是内部状态的话请标 `#[inspect(skip)]`",
                    field.ty.to_token_stream()
                ),
            ));
        };

        let label = match &attr.label {
            Some(s) => quote!(Some(#s)),
            None => quote!(None),
        };
        let unit = attr.unit.as_deref().unwrap_or("");
        let tooltip = attr.tooltip.as_deref().unwrap_or("");
        let fname = ident.to_string();
        let kind_ident = syn::Ident::new(kind, ident.span());
        let (min, max, speed, step) = (
            opt_f32(attr.min),
            opt_f32(attr.max),
            opt_f32(attr.speed),
            opt_f32(attr.step),
        );

        descs.push(quote! {
            ::xgame::inspect::FieldDesc {
                name: #fname,
                kind: ::xgame::inspect::FieldKind::#kind_ident,
                label: #label,
                min: #min,
                max: #max,
                speed: #speed,
                step: #step,
                unit: #unit,
                tooltip: #tooltip,
            },
        });

        // 下标是编译期常量：表、get、set 三处都由同一个 `index` 变量生成，
        // 结构上不可能错位 —— 这正是"不要手写这个 trait"的理由。
        //
        // get 那一臂对 `String` 必须 clone：`inspect_get` 取的是 `&self`，
        // `self.name` 在 Copy 之外的类型上是**移动**，编译器会直接拒
        // （E0507）。数值 / bool / glam 向量都是 Copy，直接搬最省事。
        let get_expr = if kind == "String" {
            quote! { ::xgame::inspect::FieldValue::#kind_ident(self.#ident.clone()) }
        } else {
            quote! { ::xgame::inspect::FieldValue::#kind_ident(self.#ident) }
        };
        gets.push(quote! {
            #index => #get_expr,
        });
        sets.push(quote! {
            (#index, ::xgame::inspect::FieldValue::#kind_ident(v)) => {
                self.#ident = v;
            }
        });
        index += 1;
    }

    // 一个字段都没剩（全是 `skip`）：与单元结构体走同一条空表路径。
    if index == 0 {
        return Ok(empty_inspect(name, impl_g, ty_g, where_c));
    }

    Ok(quote! {
        impl #impl_g ::xgame::inspect::Inspect for #name #ty_g #where_c {
            fn inspect_fields(&self) -> &'static [::xgame::inspect::FieldDesc] {
                static FIELDS: &[::xgame::inspect::FieldDesc] = &[#(#descs)*];
                FIELDS
            }
            fn inspect_get(&self, index: usize) -> ::xgame::inspect::FieldValue {
                match index {
                    #(#gets)*
                    _ => ::xgame::inspect::FieldKind::F32.default_value(),
                }
            }
            fn inspect_set(&mut self, index: usize, value: ::xgame::inspect::FieldValue) {
                match (index, value) {
                    #(#sets)*
                    _ => {}
                }
            }
        }
    })
}
