//! 从 Rust 字段声明生成现有反射接口。宏只生成描述和投影，路径与事务仍由调用方解释。

#![deny(unsafe_code)]

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Ident, LitInt, LitStr, Type};

#[proc_macro_derive(Component, attributes(component, field))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    component(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(ReflectValue, attributes(reflect, field))]
pub fn derive_reflect_value(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    reflect_value(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn component(input: &DeriveInput) -> syn::Result<Tokens> {
    let fields = named_fields(input, true)?;
    let mut id = None;
    let mut name = None;
    let mut doc = None;
    for attr in &input.attrs {
        if attr.path().is_ident("component") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") && id.is_none() {
                    id = Some(meta.value()?.parse::<LitInt>()?);
                } else if meta.path.is_ident("name") && name.is_none() {
                    name = Some(meta.value()?.parse::<LitStr>()?);
                } else if meta.path.is_ident("doc") && doc.is_none() {
                    doc = Some(meta.value()?.parse::<LitStr>()?);
                } else {
                    return Err(meta.error("未知或重复的组件属性"));
                }
                Ok(())
            })?;
        }
    }
    let id = id.ok_or_else(|| syn::Error::new(input.ident.span(), "组件必须声明 id"))?;
    let name = name.ok_or_else(|| syn::Error::new(input.ident.span(), "组件必须声明 name"))?;
    let doc = doc.unwrap_or_else(|| LitStr::new("", input.ident.span()));
    let ident = &input.ident;
    let descriptors = descriptors(&fields);
    let reads = fields.iter().map(|field| {
        let Field { ident, name, ty, .. } = field;
        quote_spanned!(ty.span()=> #name => Some(<#ty as ::xgc_reflect::ReflectValue>::to_value(&self.#ident)),)
    });
    let hashes = fields.iter().map(|field| {
        let Field { ident, name, ty, .. } = field;
        quote_spanned!(ty.span()=> #name => <#ty as ::xgc_reflect::ReflectValue>::hash_into(&self.#ident, hash),)
    });
    let component_name = &name;
    let writes = fields.iter().map(|field| {
        let Field {
            ident, name, ty, ..
        } = field;
        quote_spanned!(ty.span()=> #name => {
            self.#ident = <#ty as ::xgc_reflect::ReflectValue>::from_value(value)
                .map_err(|err| format!("{}.{} {}", #component_name, #name, err))?;
            Ok(())
        },)
    });
    let skipped: Vec<_> = fields
        .iter()
        .filter(|field| field.skip_hash)
        .map(|field| &field.name)
        .collect();
    let state_fields = if skipped.is_empty() {
        Tokens::new()
    } else {
        quote! {
            fn state_fields() -> ::std::vec::Vec<::xgc_reflect::FieldDescriptor> {
                Self::fields().into_iter().filter(|field| !matches!(field.name, #(#skipped)|*)).collect()
            }
        }
    };
    Ok(quote! {
        impl ::xgc_reflect::Component for #ident {
            const ID: ::xgc_reflect::ComponentId = ::xgc_reflect::ComponentId(#id);
            const NAME: &'static str = #name;

            fn default_value() -> Self {
                <Self as ::std::default::Default>::default()
            }

            fn doc() -> &'static str { #doc }

            fn fields() -> ::std::vec::Vec<::xgc_reflect::FieldDescriptor> { #descriptors }

            #state_fields

            fn get_field(&self, field: &str) -> Option<::xgc_reflect::Value> {
                match field { #(#reads)* _ => None }
            }

            fn hash_field_into(&self, field: &str, hash: &mut u64) {
                match field { #(#hashes)* _ => () }
            }

            fn set_field(&mut self, field: &str, value: &::xgc_reflect::Value) -> Result<(), String> {
                match field { #(#writes)* _ => Err(format!("{} 没有字段 {field}", #name)) }
            }
        }
    })
}

fn reflect_value(input: &DeriveInput) -> syn::Result<Tokens> {
    let fields = named_fields(input, false)?;
    let mut element_name = None;
    for attr in &input.attrs {
        if attr.path().is_ident("reflect") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("element_name") && element_name.is_none() {
                    element_name = Some(meta.value()?.parse::<LitStr>()?);
                    Ok(())
                } else {
                    Err(meta.error("未知或重复的结构体属性"))
                }
            })?;
        }
    }
    let ident = &input.ident;
    let descriptors = descriptors(&fields);
    let count = fields.len();
    let projections = fields.iter().map(|field| {
        let Field {
            ident, name, ty, ..
        } = field;
        quote_spanned!(ty.span()=> (#name, &self.#ident))
    });
    let writes = fields.iter().map(|field| {
        let Field {
            ident, name, ty, ..
        } = field;
        quote_spanned!(ty.span()=> #ident: <#ty as ::xgc_reflect::ReflectValue>::from_value(
            fields.get(#name).ok_or_else(|| format!("缺少字段 {}", #name))?
        ).map_err(|err| format!("{}：{}", #name, err))?,)
    });
    let element_name = element_name.map(|name| quote!(const ELEMENT_NAME: &'static str = #name;));
    Ok(quote! {
        impl ::xgc_reflect::ReflectValue for #ident {
            #element_name

            fn field_type() -> ::xgc_reflect::FieldType {
                ::xgc_reflect::FieldType::Struct({ #descriptors })
            }

            fn to_value(&self) -> ::xgc_reflect::Value {
                ::xgc_reflect::ValueProjection::projected_value(self)
            }

            fn project_value(&self, writer: &mut ::xgc_reflect::ValueWriter<'_>) {
                writer.record([#(#projections),*]);
            }

            fn from_value(value: &::xgc_reflect::Value) -> Result<Self, String> {
                let ::xgc_reflect::Value::Struct(fields) = value else {
                    return Err(format!("期望 struct，得到 {}", value.type_name()));
                };
                if fields.len() != #count {
                    return Err(format!("期望 {} 个字段，得到 {}", #count, fields.len()));
                }
                Ok(Self { #(#writes)* })
            }
        }
    })
}

struct Field<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    name: String,
    metadata: Tokens,
    skip_hash: bool,
}

fn named_fields(input: &DeriveInput, is_component: bool) -> syn::Result<Vec<Field<'_>>> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "反射派生需要具体类型",
        ));
    }
    let Data::Struct(structure) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "反射派生需要具名字段的结构体",
        ));
    };
    let Fields::Named(fields) = &structure.fields else {
        return Err(syn::Error::new_spanned(
            &structure.fields,
            "反射派生需要具名字段",
        ));
    };
    fields
        .named
        .iter()
        .map(|field| {
            let ident = field.ident.as_ref().expect("具名字段");
            let mut metadata = Tokens::new();
            let mut skip_hash = false;
            let mut seen = HashSet::new();
            for attr in &field.attrs {
                if attr.path().is_ident("field") {
                    attr.parse_nested_meta(|meta| {
                        let key = meta
                            .path
                            .get_ident()
                            .ok_or_else(|| meta.error("字段属性须为标识符"))?;
                        if !seen.insert(key.to_string()) {
                            return Err(meta.error("重复的字段属性"));
                        }
                        metadata.extend(match key.to_string().as_str() {
                            "unit" | "group" | "doc" => {
                                let text: LitStr = meta.value()?.parse()?;
                                quote!(.#key(#text))
                            }
                            "step" => {
                                let number: Expr = meta.value()?.parse()?;
                                quote!(.step(#number))
                            }
                            "asset" => {
                                let kind: Ident = meta.value()?.parse()?;
                                quote!(.asset(::xgc_reflect::AssetKind::#kind))
                            }
                            "range" => {
                                let Expr::Tuple(tuple) = meta.value()?.parse()? else {
                                    return Err(meta.error("range 需要 (min, max)"));
                                };
                                if tuple.elems.len() != 2 {
                                    return Err(meta.error("range 需要 (min, max)"));
                                }
                                let bounds = tuple.elems.iter();
                                quote!(.range(#(#bounds),*))
                            }
                            "read_only" => quote!(.read_only()),
                            "skip_hash" if is_component => {
                                skip_hash = true;
                                Tokens::new()
                            }
                            _ => return Err(meta.error("未知的字段属性")),
                        });
                        Ok(())
                    })?;
                }
            }
            Ok(Field {
                ident,
                ty: &field.ty,
                name: ident.unraw().to_string(),
                metadata,
                skip_hash,
            })
        })
        .collect()
}

fn descriptors(fields: &[Field<'_>]) -> Tokens {
    if fields.is_empty() {
        return quote!(::std::vec::Vec::new());
    }
    let descriptors = fields.iter().map(|field| {
        let Field { ident, name, ty, metadata, .. } = field;
        quote_spanned!(ty.span()=> <#ty as ::xgc_reflect::ReflectValue>::descriptor(#name, &default.#ident)#metadata)
    });
    quote! {
        let default = <Self as ::std::default::Default>::default();
        ::std::vec![#(#descriptors),*]
    }
}
