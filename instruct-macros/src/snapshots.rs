use proc_macro2::TokenStream;
use quote::quote;
use serde_derive_internals::{ast, attr::TagType, Ctxt, Derive};
use syn::DeriveInput;

/// Walk fields of an open model, preserving the wire path used by Serde.
pub fn method(input: &DeriveInput, types: &TokenStream) -> syn::Result<TokenStream> {
    let cx = Ctxt::new();
    let container = ast::Container::from_ast(&cx, input, Derive::Deserialize);
    cx.check()?;
    let model = container.expect("invalid containers report an error to Ctxt");
    let value_ty = quote!(#types::serde_json::Value);
    let walk = if model.attrs.type_from().is_some() || model.attrs.type_try_from().is_some() {
        TokenStream::new()
    } else {
        match &model.data {
            ast::Data::Struct(style, fields) => walk_fields(*style, fields, types, None),
            ast::Data::Enum(variants) => walk_variants(model.attrs.tag(), variants, types),
        }
    };
    Ok(quote! {
        fn prepare_partial(
            value: &mut #value_ty,
            open_path: ::std::option::Option<&[::std::string::String]>,
            context: &#value_ty,
        ) -> ::std::result::Result<(), ::std::string::String> {
            if open_path.is_none() {
                return #types::snapshots::normalize::<Self>(value, context);
            }
            #walk
            ::std::result::Result::Ok(())
        }
    })
}

fn walk_fields(
    style: ast::Style,
    fields: &[ast::Field<'_>],
    types: &TokenStream,
    internal_tag: Option<&str>,
) -> TokenStream {
    let mut walk = TokenStream::new();
    let fields_in_input = fields.iter().filter(|f| !f.attrs.skip_deserializing());
    let flattened = fields_in_input
        .clone()
        .filter(|f| f.attrs.flatten())
        .count();
    for (index, field) in fields_in_input.clone().enumerate() {
        if field.attrs.deserialize_with().is_some() {
            continue;
        }
        let ty = field.ty;
        if field.attrs.flatten() {
            // Only one flattened field has an unambiguous set of remaining keys.
            if flattened == 1 {
                let claimed = fields_in_input
                    .clone()
                    .filter(|f| !f.attrs.flatten())
                    .flat_map(|f| f.attrs.aliases().iter().map(String::as_str))
                    .chain(internal_tag);
                walk.extend(quote! {
                    #types::snapshots::flattened::<#ty>(value, &[#(#claimed),*], open_path, context)?;
                });
            }
        } else if field.attrs.transparent() || matches!(style, ast::Style::Newtype) {
            walk.extend(quote!(
                <#ty as #types::InstructMacro>::prepare_partial(value, open_path, context)?;
            ));
        } else if matches!(style, ast::Style::Tuple) {
            let key = index.to_string();
            walk.extend(quote! {
                if let ::std::option::Option::Some(child) = value.get_mut(#index) {
                    #types::snapshots::child::<#ty>(#key, child, open_path, context)?;
                }
            });
        } else {
            let aliases = field.attrs.aliases();
            walk.extend(quote! {
                #(if let ::std::option::Option::Some(child) = value.get_mut(#aliases) {
                    #types::snapshots::child::<#ty>(#aliases, child, open_path, context)?;
                })*
            });
        }
    }
    walk
}

fn walk_variants(tag: &TagType, variants: &[ast::Variant<'_>], types: &TokenStream) -> TokenStream {
    // Untagged fallbacks require Serde to try the complete alternatives.
    if variants.iter().any(|v| v.attrs.untagged()) {
        return TokenStream::new();
    }
    let selector = match tag {
        TagType::None => return TokenStream::new(),
        TagType::Internal { tag } | TagType::Adjacent { tag, .. } => {
            quote!(#types::snapshots::discriminator(value, #tag, open_path))
        }
        TagType::External => quote!(value
            .as_object()
            .filter(|m| m.len() == 1)
            .and_then(|m| m.keys().next())
            .map(|k| k.as_str())),
    };
    let internal_tag = match tag {
        TagType::Internal { tag } => Some(tag.as_str()),
        _ => None,
    };
    let mut arms = TokenStream::new();
    for variant in variants {
        if variant.attrs.skip_deserializing() || variant.attrs.deserialize_with().is_some() {
            continue;
        }
        let walk = walk_fields(variant.style, &variant.fields, types, internal_tag);
        if walk.is_empty() {
            continue;
        }
        for alias in variant.attrs.aliases() {
            let payload = match tag {
                TagType::Adjacent { content, .. } => Some(content),
                TagType::External => Some(alias),
                _ => None,
            };
            let action = if let Some(payload) = payload {
                quote! {
                    if let ::std::option::Option::Some(value) = value.get_mut(#payload).filter(|v| v.is_array() || v.is_object()) {
                        let open_path = #types::snapshots::child_path(#payload, open_path);
                        #walk
                    }
                }
            } else {
                walk.clone()
            };
            arms.extend(quote!(::std::option::Option::Some(#alias) => { #action }));
        }
    }
    if arms.is_empty() {
        return arms;
    }
    quote! { match #selector { #arms _ => {} } }
}
