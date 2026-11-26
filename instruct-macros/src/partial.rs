use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse_quote, punctuated::Punctuated, visit_mut::VisitMut, Attribute, Data, DeriveInput, Expr,
    ExprLit, Lit, Meta, Path, Token, Type,
};

use crate::helpers::{binding, by_variant, members, runtime};

pub fn derive(mut input: DeriveInput) -> syn::Result<TokenStream> {
    let (types, _) = runtime();
    let name = input.ident.clone();
    let snapshot = format_ident!("{}Partial", name);
    let serde_path = quote!(#types::serde).to_string();

    let conversion = by_variant(&input.data, |variant, fields| {
        let values = members(fields, |i| {
            let var = binding(i);
            quote!(#types::PartialModel::into_field(#var))
        });
        Ok(quote!(#snapshot #variant #values))
    })?;
    for generic in input.generics.type_params_mut() {
        generic.bounds.push(parse_quote!(#types::PartialModel));
    }
    let generics = input.generics.clone();
    let (impl_generics, type_generics, predicates) = generics.split_for_impl();
    let turbofish = type_generics.as_turbofish();
    let mut replace_self = ReplaceSelf(parse_quote!(#name #type_generics));
    replace_self.visit_derive_input_mut(&mut input);
    input.ident = snapshot.clone();

    // Default functions return a value of the *full* type. Project it
    // into the corresponding snapshot field, including nested defaults.
    let mut defaults = Vec::new();
    if let Some(make_default) = serde_attributes(&mut input.attrs)? {
        let function = format_ident!("__instructor_default");
        let path = quote!(#snapshot #turbofish :: #function).to_string();
        input.attrs.push(parse_quote!(#[serde(default = #path)]));
        defaults.push(quote! {
            fn #function() -> Self {
                <#name #type_generics as #types::PartialModel>::into_partial(#make_default())
            }
        });
    }
    let mut transform = |fields: &mut syn::Fields| {
        for field in fields.iter_mut() {
            let ty = field.ty.clone();
            field.ty = parse_quote!(<#ty as #types::PartialModel>::Field);
            if let Some(make_default) = serde_attributes(&mut field.attrs)? {
                let function = format_ident!("__instructor_default_{}", defaults.len());
                let path = quote!(#snapshot #turbofish :: #function).to_string();
                field.attrs.push(parse_quote!(#[serde(default = #path)]));
                defaults.push(quote! {
                    fn #function() -> <#ty as #types::PartialModel>::Field {
                        <#ty as #types::PartialModel>::into_field(#make_default())
                    }
                });
            }
        }
        Ok::<_, syn::Error>(())
    };
    match &mut input.data {
        Data::Struct(data) => transform(&mut data.fields)?,
        Data::Enum(data) => {
            for variant in &mut data.variants {
                serde_attributes(&mut variant.attrs)?;
                transform(&mut variant.fields)?;
            }
        }
        Data::Union(_) => unreachable!("by_variant rejects unions"),
    }
    let defaults_impl = if defaults.is_empty() {
        quote!()
    } else {
        quote!(impl #impl_generics #snapshot #type_generics #predicates { #(#defaults)* })
    };
    // Rust's built-in Debug derive adds T: Debug on generic snapshots.
    // Only the projected fields need Debug, as guaranteed by PartialModel.
    let debug = by_variant(&input.data, |variant, fields| {
        let variant_name = if variant.is_empty() {
            snapshot.to_string()
        } else {
            variant
                .to_string()
                .trim_start_matches(':')
                .trim()
                .to_owned()
        };
        let values = (0..fields.len()).map(binding);
        Ok(match fields {
            syn::Fields::Named(_) => {
                let names = fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| crate::helpers::label(f, i));
                quote!(f.debug_struct(#variant_name)#(.field(#names, #values))*.finish())
            }
            syn::Fields::Unnamed(_) => {
                quote!(f.debug_tuple(#variant_name)#(.field(#values))*.finish())
            }
            syn::Fields::Unit => quote!(f.write_str(#variant_name)),
        })
    })?;
    Ok(quote! {
        #[derive(#types::serde::Serialize, #types::serde::Deserialize)]
        #[serde(crate = #serde_path, bound = "")]
        #input
        #defaults_impl
        impl #impl_generics ::core::fmt::Debug for #snapshot #type_generics #predicates {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #debug
            }
        }
        impl #impl_generics #types::PartialModel for #name #type_generics #predicates {
            type Partial = #snapshot #type_generics;
            type Field = ::std::option::Option<Self::Partial>;
            fn into_partial(self) -> Self::Partial { #conversion }
        }
    })
}

/// Preserve serde wire names and variant representations. Hooks whose
/// signatures operate on full field types run on final deserialization.
fn serde_attributes(attrs: &mut Vec<Attribute>) -> syn::Result<Option<Path>> {
    let mut default = None;
    let mut keep = Vec::new();
    for attr in attrs.drain(..) {
        if attr.path().is_ident("serde") {
            let metas = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            for meta in metas {
                let Some(id) = meta.path().get_ident() else {
                    continue;
                };
                match id.to_string().as_str() {
                    "default" => default = Some(default_fn(&meta)?),
                    "rename"
                    | "rename_all"
                    | "rename_all_fields"
                    | "alias"
                    | "tag"
                    | "content"
                    | "untagged"
                    | "deny_unknown_fields"
                    | "skip"
                    | "skip_deserializing"
                    | "skip_serializing"
                    | "flatten"
                    | "transparent"
                    | "other" => {
                        keep.push(parse_quote!(#[serde(#meta)]));
                    }
                    _ => {}
                }
            }
        } else if attr.path().is_ident("doc") || attr.path().is_ident("allow") {
            keep.push(attr);
        }
    }
    *attrs = keep;
    Ok(default)
}

fn default_fn(meta: &Meta) -> syn::Result<Path> {
    if matches!(meta, Meta::Path(_)) {
        return Ok(parse_quote!(::core::default::Default::default));
    }
    if let Meta::NameValue(m) = meta {
        if let Expr::Lit(ExprLit {
            lit: Lit::Str(path),
            ..
        }) = &m.value
        {
            return path.parse();
        }
    }
    Err(syn::Error::new_spanned(
        meta,
        "expected a default function path",
    ))
}

struct ReplaceSelf(Type);

impl VisitMut for ReplaceSelf {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if matches!(&ty, Type::Path(p) if p.path.is_ident("Self")) {
            *ty = self.0.clone();
        } else {
            syn::visit_mut::visit_type_mut(self, ty);
        }
    }
}
