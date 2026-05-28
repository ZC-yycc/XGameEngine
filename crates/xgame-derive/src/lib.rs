//! XGameEngine 过程宏。
//!
//! 提供 `#[derive(Panel)]` 派生宏，自动为面板标签枚举生成
//! `show()` 方法，将每个变体分发到
//! `crate::panels::<蛇形命名>::show_content()`。

use proc_macro::TokenStream;
use quote::quote;
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
