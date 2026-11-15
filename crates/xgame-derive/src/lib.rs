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
        data.variants
            .iter()
            .map(|v| v.ident.clone())
            .collect::<Vec<_>>()
    } else {
        panic!("Panel can only be derived for enums");
    };

    let match_arms = variants.iter().map(|variant| {
        let snake = to_snake_case(&variant.to_string());
        // Build path: crate::panels::<snake>::show_content
        let path_str = format!("crate::panels::{}::show_content", snake);
        let path: syn::Path =
            syn::parse_str(&path_str).expect("invalid module path generated from variant name");
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

// ---------------------------------------------------------------------------
// `#[derive(Reflect)]` —— 类型元数据 + 借用式字段访问
// ---------------------------------------------------------------------------

/// `#[reflect(...)]` 收集到的字段选项。
#[derive(Default)]
struct ReflectOpts {
    skip: bool,
    hidden: bool,
    read_only: bool,
    label: Option<String>,
    unit: Option<String>,
    speed: Option<f32>,
    min: Option<f32>,
    max: Option<f32>,
    decimals: Option<u8>,
    note: Option<String>,
    hint: Option<String>,
    visible_when: Option<String>,
    row_group: Option<String>,
    item_prefix: Option<String>,
    add_label: Option<String>,
    variant_note: bool,
    doc: String,
}

/// 把 `#[doc = "..."]` 拼成一行说明。
fn collect_doc(attrs: &[syn::Attribute]) -> String {
    let mut out = String::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                let line = s.value();
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(line);
            }
        }
    }
    out
}

fn parse_f32_lit(input: syn::parse::ParseStream) -> syn::Result<f32> {
    let lit: syn::Lit = input.parse()?;
    match lit {
        syn::Lit::Float(f) => f.base10_parse::<f32>(),
        syn::Lit::Int(i) => i.base10_parse::<i64>().map(|v| v as f32),
        other => Err(syn::Error::new_spanned(other, "这里需要数字字面量")),
    }
}

fn parse_u8_lit(input: syn::parse::ParseStream) -> syn::Result<u8> {
    let lit: syn::Lit = input.parse()?;
    match lit {
        syn::Lit::Int(i) => i.base10_parse::<u8>(),
        other => Err(syn::Error::new_spanned(other, "这里需要 0..=255 的整数字面量")),
    }
}

fn parse_reflect_opts(field: &syn::Field) -> syn::Result<ReflectOpts> {
    let mut opts = ReflectOpts {
        doc: collect_doc(&field.attrs),
        ..ReflectOpts::default()
    };
    for attr in &field.attrs {
        if !attr.path().is_ident("reflect") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let ident = match meta.path.get_ident() {
                Some(ident) => ident.to_string(),
                None => return Err(meta.error("`#[reflect(...)]` 只接受裸标识符选项")),
            };
            match ident.as_str() {
                "skip" => opts.skip = true,
                "hidden" => opts.hidden = true,
                "read_only" => opts.read_only = true,
                "label" => opts.label = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                "unit" => opts.unit = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                "doc" => opts.doc = meta.value()?.parse::<syn::LitStr>()?.value(),
                "speed" => opts.speed = Some(parse_f32_lit(meta.value()?)?),
                "min" => opts.min = Some(parse_f32_lit(meta.value()?)?),
                "max" => opts.max = Some(parse_f32_lit(meta.value()?)?),
                "decimals" => opts.decimals = Some(parse_u8_lit(meta.value()?)?),
                "note" => opts.note = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                "hint" => opts.hint = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                "visible_when" => {
                    opts.visible_when = Some(meta.value()?.parse::<syn::LitStr>()?.value())
                }
                "row_group" => opts.row_group = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                "item_prefix" => {
                    opts.item_prefix = Some(meta.value()?.parse::<syn::LitStr>()?.value())
                }
                "add_label" => opts.add_label = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                "variant_note" => opts.variant_note = true,
                other => {
                    return Err(meta.error(format!(
                        "不认识的选项 `{other}`（可用：skip / hidden / read_only / label / unit / \
                         speed / min / max / decimals / note / hint / visible_when / row_group / \
                         item_prefix / add_label / variant_note / doc）"
                    )))
                }
            }
            Ok(())
        })?;
    }
    Ok(opts)
}

/// `#[reflect(...)]` 收集到的**变体**选项。
#[derive(Default)]
struct VariantOpts {
    label: Option<String>,
    doc: String,
}

fn parse_variant_opts(variant: &syn::Variant) -> syn::Result<VariantOpts> {
    let mut opts = VariantOpts {
        doc: collect_doc(&variant.attrs),
        ..VariantOpts::default()
    };
    for attr in &variant.attrs {
        if !attr.path().is_ident("reflect") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let ident = match meta.path.get_ident() {
                Some(ident) => ident.to_string(),
                None => return Err(meta.error("`#[reflect(...)]` 只接受裸标识符选项")),
            };
            match ident.as_str() {
                "label" => opts.label = Some(meta.value()?.parse::<syn::LitStr>()?.value()),
                "doc" => opts.doc = meta.value()?.parse::<syn::LitStr>()?.value(),
                other => {
                    return Err(meta.error(format!(
                        "不认识的变体选项 `{other}`（可用：label / doc）"
                    )))
                }
            }
            Ok(())
        })?;
    }
    Ok(opts)
}

/// 标量类型名 → `ScalarKind` 变体名。
///
/// 按**末段标识符**判断，因为 `glam::Vec3` 与 `Vec3` 应该映射到同一个标量。
fn scalar_variant(ident: &str) -> Option<&'static str> {
    Some(match ident {
        "f32" => "F32",
        "f64" => "F64",
        "i32" => "I32",
        "i64" => "I64",
        "u32" => "U32",
        "u64" => "U64",
        "bool" => "Bool",
        "String" => "String",
        "Vec2" => "Vec2",
        "Vec3" => "Vec3",
        "Vec4" => "Vec4",
        _ => return None,
    })
}

fn generic_arg<'a>(seg: &'a syn::PathSegment, ty: &'a syn::Type) -> syn::Result<&'a syn::Type> {
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return Err(syn::Error::new_spanned(ty, "缺少泛型参数"));
    };
    for arg in &args.args {
        if let syn::GenericArgument::Type(t) = arg {
            return Ok(t);
        }
    }
    Err(syn::Error::new_spanned(ty, "缺少泛型参数"))
}

/// 字段类型是不是 `bool`（`visible_when` 的编译期校验用）。
fn is_bool_type(ty: &syn::Type) -> bool {
    let syn::Type::Path(tp) = ty else {
        return false;
    };
    tp.path
        .segments
        .last()
        .is_some_and(|seg| seg.ident == "bool" && seg.arguments.is_none())
}

/// 生成一个 `TypeRef` 的**运行期表达式**。
///
/// 为什么不是常量：嵌套引用要拿到 `&'static TypeRef`，而 `Box::leak` 是运行期调用，
/// 不能出现在 `const` / `static` 初始化里。元数据整体改成惰性构建（`OnceLock`），
/// 于是这里可以合法分配。每种类型只泄漏一次，进程内总量与类型数同阶。
fn type_ref_expr(ty: &syn::Type) -> syn::Result<proc_macro2::TokenStream> {
    let p = quote!(::xgame_core::reflect);
    match ty {
        syn::Type::Path(tp) => {
            let last = tp
                .path
                .segments
                .last()
                .ok_or_else(|| syn::Error::new_spanned(ty, "空的类型路径"))?;
            let ident = last.ident.to_string();
            if let Some(variant) = scalar_variant(&ident) {
                let variant = syn::Ident::new(variant, last.ident.span());
                return Ok(quote!(#p::TypeRef::Scalar(#p::ScalarKind::#variant)));
            }
            match ident.as_str() {
                "Option" | "Vec" => {
                    let inner = type_ref_expr(generic_arg(last, ty)?)?;
                    let ctor = if ident == "Option" { "Option" } else { "List" };
                    let ctor = syn::Ident::new(ctor, last.ident.span());
                    Ok(quote!(#p::TypeRef::#ctor(::std::boxed::Box::leak(
                        ::std::boxed::Box::new(#inner)
                    ))))
                }
                _ => Ok(quote!(#p::TypeRef::Named(
                    <#ty as #p::ReflectMeta>::meta
                ))),
            }
        }
        syn::Type::Array(arr) => {
            let inner = type_ref_expr(&arr.elem)?;
            let len = &arr.len;
            Ok(quote!(#p::TypeRef::Array(
                ::std::boxed::Box::leak(::std::boxed::Box::new(#inner)),
                #len
            )))
        }
        other => Err(syn::Error::new_spanned(
            other,
            "`#[derive(Reflect)]` 不支持的字段类型（不支持引用 / 元组 / 裸指针；\
             带载荷枚举请手写 impl）",
        )),
    }
}

/// 派生宏：为具名结构体生成元数据 + 字段读写。
///
/// # Example
///
/// ```ignore
/// #[derive(Reflect, Default)]
/// struct Camera {
///     #[reflect(range = 1.0, max = 179.0, unit = "°")]
///     fov: f32,
///     #[reflect(skip)]
///     cache: Vec<u8>,
/// }
/// ```
#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn reflect_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_reflect(&input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// 一个字段在代码生成里要的信息。
struct FieldSpec {
    /// 寻址名（= Rust 字段名）。
    name: String,
    ident: syn::Ident,
    read_only: bool,
}

/// 一份字段表的生成结果。
struct FieldCodegen {
    /// `FieldMeta { … }` 表达式，按声明顺序，已跳过 `skip` 的字段。
    metas: Vec<proc_macro2::TokenStream>,
    /// 参与寻址的字段（顺序同上）。
    specs: Vec<FieldSpec>,
}

/// 生成一份字段表。结构体字段与枚举载荷字段共用这一处 ——
/// 两边唯一的差别是"怎么拿到 `&self.field`"，那由调用方拼。
fn gen_fields(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    p: &proc_macro2::TokenStream,
) -> syn::Result<FieldCodegen> {
    let mut metas = Vec::new();
    let mut specs = Vec::new();
    // `visible_when` 的编译期校验用：`(字段名, 是否 bool)`。
    let mut declared: Vec<(String, bool)> = Vec::new();
    let mut visibility_refs: Vec<(String, String, proc_macro2::Span)> = Vec::new();

    for field in fields {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "字段没有名字"))?;
        let opts = parse_reflect_opts(field)?;
        if opts.skip {
            continue;
        }
        let name = ident.to_string();
        declared.push((name.clone(), is_bool_type(&field.ty)));
        if let Some(when) = &opts.visible_when {
            visibility_refs.push((name.clone(), when.clone(), ident.span()));
        }
        let ty_ref = type_ref_expr(&field.ty)?;

        let label = match &opts.label {
            Some(label) => quote!(::std::option::Option::Some(#label)),
            None => quote!(::std::option::Option::None),
        };
        let unit = match &opts.unit {
            Some(unit) => quote!(::std::option::Option::Some(#unit)),
            None => quote!(::std::option::Option::None),
        };
        let speed = match opts.speed {
            Some(speed) => quote!(::std::option::Option::Some(#speed)),
            None => quote!(::std::option::Option::None),
        };
        let range = match (opts.min, opts.max) {
            (Some(min), Some(max)) => quote!(::std::option::Option::Some((#min, #max))),
            (None, None) => quote!(::std::option::Option::None),
            _ => {
                return Err(syn::Error::new_spanned(
                    field,
                    "`min` 与 `max` 必须同时给出（只给一半的范围没有意义）",
                ))
            }
        };
        let flags = match (opts.hidden, opts.read_only) {
            (true, true) => quote!(#p::FieldFlags { hidden: true, read_only: true }),
            (true, false) => quote!(#p::FieldFlags::HIDDEN),
            (false, true) => quote!(#p::FieldFlags::READ_ONLY),
            (false, false) => quote!(#p::FieldFlags::NONE),
        };
        let decimals = match opts.decimals {
            Some(d) => quote!(::std::option::Option::Some(#d)),
            None => quote!(::std::option::Option::None),
        };
        let opt_str = |v: &Option<String>| match v {
            Some(s) => quote!(::std::option::Option::Some(#s)),
            None => quote!(::std::option::Option::None),
        };
        let note = opt_str(&opts.note);
        let hint = opt_str(&opts.hint);
        let visible_when = opt_str(&opts.visible_when);
        let row_group = opt_str(&opts.row_group);
        let variant_note = opts.variant_note;
        let item_prefix = opt_str(&opts.item_prefix);
        let add_label = opt_str(&opts.add_label);
        let field_doc = opts.doc;

        metas.push(quote! {
            #p::FieldMeta {
                name: #name,
                label: #label,
                ty: #ty_ref,
                range: #range,
                speed: #speed,
                unit: #unit,
                decimals: #decimals,
                note: #note,
                hint: #hint,
                visible_when: #visible_when,
                row_group: #row_group,
                variant_note: #variant_note,
                item_prefix: #item_prefix,
                add_label: #add_label,
                flags: #flags,
                doc: #field_doc,
            }
        });
        specs.push(FieldSpec {
            name,
            ident: ident.clone(),
            read_only: opts.read_only,
        });
    }

    // `visible_when` 指向的字段必须存在且是 `bool`。写错名字时给编译错误，
    // 而不是让 `FieldMeta::visible_given` 按"显示"兜底 —— 那种错误在界面上
    // 看不出来，只会在"为什么这个开关没用"的时候才暴露。
    for (owner, when, span) in &visibility_refs {
        match declared.iter().find(|(declared, _)| declared == when) {
            None => {
                return Err(syn::Error::new(
                    *span,
                    format!("`visible_when = \"{when}\"` 指向的字段不存在（`{owner}` 所在的结构体里没有 `{when}`）"),
                ))
            }
            Some((_, false)) => {
                return Err(syn::Error::new(
                    *span,
                    format!("`visible_when = \"{when}\"` 必须是 `bool` 字段"),
                ))
            }
            Some(_) => {}
        }
    }

    Ok(FieldCodegen { metas, specs })
}

fn expand_reflect(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    match &input.data {
        syn::Data::Struct(_) => expand_struct(input),
        syn::Data::Enum(_) => expand_enum(input),
        syn::Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "`#[derive(Reflect)]` 不支持 union",
        )),
    }
}

/// 具名字段结构体：`ReflectMeta` + `Reflect` + `ReflectItem`。
fn expand_struct(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let p = quote!(::xgame_core::reflect);
    let doc = collect_doc(&input.attrs);

    let syn::Data::Struct(data) = &input.data else {
        unreachable!("expand_struct 只接结构体");
    };
    let syn::Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            name,
            "`#[derive(Reflect)]` 只支持具名字段结构体（元组结构体的字段没有名字可寻址）",
        ));
    };
    let gen = gen_fields(&fields.named, &p)?;

    let get_arms = gen.specs.iter().map(|spec| {
        let (field, ident) = (&spec.name, &spec.ident);
        quote!(#field => ::std::option::Option::Some(#p::ReflectItem::as_value(&self.#ident)),)
    });
    let get_mut_arms = gen.specs.iter().map(|spec| {
        let (field, ident) = (&spec.name, &spec.ident);
        if spec.read_only {
            quote!(#field => ::std::option::Option::None,)
        } else {
            quote!(#field => ::std::option::Option::Some(
                #p::ReflectItem::as_value_mut(&mut self.#ident)
            ),)
        }
    });
    let field_metas = &gen.metas;

    let name_str = name.to_string();
    Ok(quote! {
        impl #p::ReflectMeta for #name {
            fn meta() -> &'static #p::TypeMeta {
                // 惰性构建：元数据里要放 `Box::leak` 出来的嵌套 `TypeRef`，
                // 不能写成 `const`（常量上下文不允许分配）。每种类型只构建一次。
                static META: ::std::sync::OnceLock<#p::TypeMeta> = ::std::sync::OnceLock::new();
                META.get_or_init(|| #p::TypeMeta {
                    name: #name_str,
                    doc: #doc,
                    kind: #p::MetaKind::Struct,
                    fields: ::std::boxed::Box::leak(::std::vec![#(#field_metas),*]
                        .into_boxed_slice()),
                    variants: &[],
                })
            }
        }

        impl #p::Reflect for #name {
            fn type_meta(&self) -> &'static #p::TypeMeta {
                <Self as #p::ReflectMeta>::meta()
            }

            fn get(&self, field: &str) -> ::std::option::Option<#p::ValueRef<'_>> {
                match field {
                    #(#get_arms)*
                    _ => ::std::option::Option::None,
                }
            }

            fn get_mut(&mut self, field: &str) -> ::std::option::Option<#p::ValueRefMut<'_>> {
                match field {
                    #(#get_mut_arms)*
                    _ => ::std::option::Option::None,
                }
            }
        }

        impl #p::ReflectItem for #name {
            fn as_value(&self) -> #p::ValueRef<'_> {
                #p::ValueRef::Struct(self)
            }
            fn as_value_mut(&mut self) -> #p::ValueRefMut<'_> {
                #p::ValueRefMut::Struct(self)
            }
        }
    })
}

/// 一个变体在代码生成里要的信息。
struct VariantGen {
    /// 变体标识符（拼载荷元数据函数名用）。
    ident: syn::Ident,
    name: String,
    /// `Self::Cuboid { .. }` / `Self::Auto`：不需要绑定时用的分支头。
    pat: proc_macro2::TokenStream,
    /// 绑定全部载荷字段的分支头（`Self::Cuboid { half_extents }`）。
    /// 与 [`pat`](Self::pat) 分开是因为 `payload_get` 要用到绑定的名字。
    payload_pat: proc_macro2::TokenStream,
    /// `set_variant` 用的构造表达式。
    ctor: proc_macro2::TokenStream,
    /// 载荷字段表；空 = 无载荷（或全部 `skip`）。
    fields: Option<FieldCodegen>,
}

/// 枚举：`ReflectMeta` + `ReflectEnum` + `ReflectItem`。
///
/// 覆盖 `CollisionShape`（带载荷）与 `BodyKind` / `AudioBus`（纯标签）两种形状。
/// 载荷字段的元数据与结构体字段走同一套 `#[reflect(...)]`，因此 `half_extents`
/// 也能声明范围、步长与文档。
fn expand_enum(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let p = quote!(::xgame_core::reflect);
    let doc = collect_doc(&input.attrs);
    let syn::Data::Enum(data) = &input.data else {
        unreachable!("expand_enum 只接枚举");
    };
    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(name, "枚举至少要有一个变体"));
    }

    let mut variants = Vec::new();
    let mut variant_metas = Vec::new();
    let mut payload_fns = Vec::new();
    let mut names = Vec::new();

    for variant in &data.variants {
        let v_ident = &variant.ident;
        let v_name = v_ident.to_string();
        let opts = parse_variant_opts(variant)?;
        names.push(v_name.clone());

        let label = match &opts.label {
            Some(label) => quote!(::std::option::Option::Some(#label)),
            None => quote!(::std::option::Option::None),
        };
        let v_doc = if opts.doc.is_empty() {
            collect_doc(&variant.attrs)
        } else {
            opts.doc.clone()
        };

        match &variant.fields {
            syn::Fields::Unit => {
                let pat = quote!(Self::#v_ident);
                variant_metas.push(quote! {
                    #p::VariantMeta {
                        name: #v_name,
                        label: #label,
                        doc: #v_doc,
                        payload: ::std::option::Option::None,
                    }
                });
                variants.push(VariantGen {
                    ident: v_ident.clone(),
                    name: v_name,
                    pat: pat.clone(),
                    payload_pat: pat.clone(),
                    ctor: pat,
                    fields: None,
                });
            }
            syn::Fields::Named(named) => {
                let gen = gen_fields(&named.named, &p)?;

                // 全字段被 `skip` 的变体等价于无载荷：不生成空载荷表，
                // 否则编辑器会为它画一个"什么都没有"的载荷区。
                if gen.metas.is_empty() {
                    variant_metas.push(quote! {
                        #p::VariantMeta {
                            name: #v_name,
                            label: #label,
                            doc: #v_doc,
                            payload: ::std::option::Option::None,
                        }
                    });
                    variants.push(VariantGen {
                        ident: v_ident.clone(),
                        name: v_name,
                        pat: quote!(Self::#v_ident { .. }),
                        payload_pat: quote!(Self::#v_ident { .. }),
                        ctor: quote!(Self::#v_ident {}),
                        fields: None,
                    });
                    continue;
                }

                let payload_fn = syn::Ident::new(
                    &format!("__xg_reflect_payload_{name}_{v_ident}"),
                    v_ident.span(),
                );
                let metas = &gen.metas;
                let payload_name = v_name.clone();
                let payload_doc = v_doc.clone();
                payload_fns.push(quote! {
                    #[doc(hidden)]
                    #[allow(non_snake_case)]
                    fn #payload_fn() -> &'static #p::TypeMeta {
                        static META: ::std::sync::OnceLock<#p::TypeMeta> =
                            ::std::sync::OnceLock::new();
                        META.get_or_init(|| #p::TypeMeta {
                            name: #payload_name,
                            doc: #payload_doc,
                            kind: #p::MetaKind::Struct,
                            fields: ::std::boxed::Box::leak(::std::vec![#(#metas),*]
                                .into_boxed_slice()),
                            variants: &[],
                        })
                    }
                });

                variant_metas.push(quote! {
                    #p::VariantMeta {
                        name: #v_name,
                        label: #label,
                        doc: #v_doc,
                        payload: ::std::option::Option::Some(#p::TypeRef::Named(#payload_fn)),
                    }
                });

                let ctor_fields = gen.specs.iter().map(|s| {
                    let ident = &s.ident;
                    quote!(#ident: ::std::default::Default::default())
                });
                let bindings: Vec<&syn::Ident> = gen.specs.iter().map(|s| &s.ident).collect();
                variants.push(VariantGen {
                    ident: v_ident.clone(),
                    name: v_name,
                    pat: quote!(Self::#v_ident { .. }),
                    payload_pat: quote!(Self::#v_ident { #(#bindings),* }),
                    ctor: quote!(Self::#v_ident { #(#ctor_fields),* }),
                    fields: Some(gen),
                });
            }
            syn::Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "`#[derive(Reflect)]` 不支持元组变体（`Variant(T)` 的载荷没有名字可寻址，\
                     编辑器画不出一行；请改成 `Variant { name: T }`）",
                ))
            }
        }
    }

    // -- ReflectEnum 的各个分支 --
    let variant_arms = variants.iter().map(|v| {
        let (pat, v_name) = (&v.pat, &v.name);
        quote!(#pat => #v_name,)
    });
    let set_arms = variants.iter().map(|v| {
        let (v_name, ctor) = (&v.name, &v.ctor);
        quote!(#v_name => { *self = #ctor; true })
    });
    let payload_meta_arms = variants.iter().map(|v| {
        let pat = &v.pat;
        match &v.fields {
            None => quote!(#pat => ::std::option::Option::None,),
            Some(_) => {
                let payload_fn = syn::Ident::new(
                    &format!("__xg_reflect_payload_{name}_{}", v.ident),
                    v.ident.span(),
                );
                quote!(#pat => ::std::option::Option::Some(#payload_fn()),)
            }
        }
    });

    let payload_get_arms = variants.iter().map(|v| {
        let pat = &v.payload_pat;
        match &v.fields {
            None => quote!(#pat => ::std::option::Option::None,),
            Some(gen) => {
                let arms = gen.specs.iter().map(|spec| {
                    let (field, ident) = (&spec.name, &spec.ident);
                    quote!(#field => ::std::option::Option::Some(
                        #p::ReflectItem::as_value(#ident)
                    ),)
                });
                quote! {
                    #pat => match field {
                        #(#arms)*
                        _ => ::std::option::Option::None,
                    },
                }
            }
        }
    });
    let payload_get_mut_arms = variants.iter().map(|v| {
        let pat = &v.payload_pat;
        match &v.fields {
            None => quote!(#pat => ::std::option::Option::None,),
            Some(gen) => {
                let arms = gen.specs.iter().map(|spec| {
                    let (field, ident) = (&spec.name, &spec.ident);
                    if spec.read_only {
                        quote!(#field => ::std::option::Option::None,)
                    } else {
                        quote!(#field => ::std::option::Option::Some(
                            #p::ReflectItem::as_value_mut(#ident)
                        ),)
                    }
                });
                quote! {
                    #pat => match field {
                        #(#arms)*
                        _ => ::std::option::Option::None,
                    },
                }
            }
        }
    });

    let name_str = name.to_string();
    let variant_metas = &variant_metas;
    Ok(quote! {
        #(#payload_fns)*

        impl #p::ReflectMeta for #name {
            fn meta() -> &'static #p::TypeMeta {
                static META: ::std::sync::OnceLock<#p::TypeMeta> = ::std::sync::OnceLock::new();
                META.get_or_init(|| #p::TypeMeta {
                    name: #name_str,
                    doc: #doc,
                    kind: #p::MetaKind::Enum,
                    fields: &[],
                    variants: ::std::boxed::Box::leak(::std::vec![#(#variant_metas),*]
                        .into_boxed_slice()),
                })
            }
        }

        impl #p::ReflectEnum for #name {
            fn variant(&self) -> &'static str {
                match self {
                    #(#variant_arms)*
                }
            }

            fn set_variant(&mut self, name: &str) -> bool {
                match name {
                    #(#set_arms)*
                    _ => false,
                }
            }

            fn variant_names(&self) -> &'static [&'static str] {
                &[#(#names),*]
            }

            fn payload_meta(&self) -> ::std::option::Option<&'static #p::TypeMeta> {
                match self {
                    #(#payload_meta_arms)*
                }
            }

            fn payload_get(&self, field: &str) -> ::std::option::Option<#p::ValueRef<'_>> {
                match self {
                    #(#payload_get_arms)*
                }
            }

            fn payload_get_mut(
                &mut self,
                field: &str,
            ) -> ::std::option::Option<#p::ValueRefMut<'_>> {
                match self {
                    #(#payload_get_mut_arms)*
                }
            }
        }

        impl #p::ReflectItem for #name {
            fn as_value(&self) -> #p::ValueRef<'_> {
                #p::ValueRef::Enum(self)
            }
            fn as_value_mut(&mut self) -> #p::ValueRefMut<'_> {
                #p::ValueRefMut::Enum(self)
            }
        }
    })
}
