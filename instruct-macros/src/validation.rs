use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, DeriveInput, LitStr, Path};

use crate::helpers::{binding, by_variant, label};

pub fn methods(input: &DeriveInput, types: &TokenStream) -> syn::Result<TokenStream> {
    let body = |context| {
        let fields = by_variant(&input.data, |_, fields| {
            let mut checks = TokenStream::new();
            for (index, field) in fields.iter().enumerate() {
                let value = binding(index);
                let name = label(field, index);
                let nested = if context {
                    quote!(#types::InstructMacro::validate_with_context(#value, __context))
                } else {
                    quote!(#types::InstructMacro::validate(#value))
                };
                checks.extend(quote!(#nested.map_err(|e| format!("{}: {}", #name, e))?;));
                let prefix = format!("Validation failed for field '{}'", name);
                checks.extend(validators(&field.attrs, quote!(#value), &prefix, context)?);
            }
            Ok(quote!({ #checks }))
        })?;
        let prefix = format!("Validation failed for model '{}'", input.ident);
        let model = validators(&input.attrs, quote!(self), &prefix, context)?;
        Ok::<_, syn::Error>(quote! { #fields #model Ok(()) })
    };
    let plain = body(false)?;
    let contextual = body(true)?;
    Ok(quote! {
        fn validate(&self) -> Result<(), String> { #plain }
        fn validate_with_context(&mut self, __context: &#types::serde_json::Value) -> Result<(), String> {
            #contextual
        }
    })
}

pub(crate) fn validators(
    attrs: &[Attribute],
    value: TokenStream,
    prefix: &str,
    with_context: bool,
) -> syn::Result<TokenStream> {
    let mut code = TokenStream::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("validate")) {
        attr.parse_nested_meta(|meta| {
            let func: Path = meta.value()?.parse::<LitStr>()?.parse()?;
            let call = if meta.path.is_ident("custom") {
                quote!(#func(#value))
            } else if meta.path.is_ident("context") {
                if !with_context {
                    return Ok(());
                }
                quote!(#func(#value, __context))
            } else {
                return Err(meta.error("expected 'custom' or 'context'"));
            };
            code.extend(quote! { #call.map_err(|e| format!("{}: {}", #prefix, e))?; });
            Ok(())
        })?;
    }
    Ok(code)
}
