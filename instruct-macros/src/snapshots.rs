use proc_macro2::TokenStream;
use quote::quote;
use serde_derive_internals::{ast, Ctxt, Derive};
use syn::DeriveInput;

/// Walk unambiguous fields of an open struct by their Serde wire names.
/// A tagged or untagged enum needs the complete variant before normalization.
pub fn method(input: &DeriveInput, types: &TokenStream) -> syn::Result<TokenStream> {
    let cx = Ctxt::new();
    let container = ast::Container::from_ast(&cx, input, Derive::Deserialize);
    cx.check()?;
    let data = container.expect("invalid containers report an error to Ctxt");
    let value_ty = quote!(#types::serde_json::Value);
    let mut walk = TokenStream::new();
    if let ast::Data::Struct(style, fields) = &data.data {
        for (index, field) in fields.iter().enumerate() {
            if field.attrs.skip_deserializing()
                || field.attrs.deserialize_with().is_some()
                || field.attrs.flatten()
            {
                continue;
            }
            let ty = field.ty;
            if field.attrs.transparent() || matches!(style, ast::Style::Newtype) {
                walk.extend(quote!(
                    <#ty as #types::InstructMacro>::prepare_partial(value, open_path, context)?;
                ));
            } else if matches!(style, ast::Style::Tuple) {
                let key = index.to_string();
                walk.extend(quote! {
                    if let Some(child) = value.get_mut(#index) {
                        #types::snapshots::child::<#ty>(#key, child, open_path, context)?;
                    }
                });
            } else {
                let aliases = field.attrs.aliases();
                walk.extend(quote! {
                    #(if let Some(child) = value.get_mut(#aliases) {
                        #types::snapshots::child::<#ty>(#aliases, child, open_path, context)?;
                    })*
                });
            }
        }
    }
    Ok(quote! {
        fn prepare_partial(value: &mut #value_ty, open_path: Option<&[String]>, context: &#value_ty) -> Result<(), String> {
            if open_path.is_none() {
                return #types::snapshots::normalize::<Self>(value, context);
            }
            #walk
            Ok(())
        }
    })
}
