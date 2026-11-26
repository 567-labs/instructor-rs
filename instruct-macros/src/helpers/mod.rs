use proc_macro2::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};
use quote::{format_ident, quote};
use syn::{
    ext::IdentExt, Attribute, Data, DeriveInput, Expr, ExprLit, Field, Fields, Ident, Lit, LitStr,
    Meta,
};

/// Resolve through the public library when used downstream, or through the
/// individual crates for projects using the original two-crate imports.
pub fn runtime() -> (TokenStream, TokenStream) {
    if let Ok(FoundCrate::Name(lib)) = crate_name("instructor-ai") {
        let name = format_ident!("{}", lib);
        return (quote!(::#name::__private), quote!(::#name));
    }
    fn dependency(package: &str) -> TokenStream {
        let alias = match crate_name(package) {
            Ok(FoundCrate::Name(name)) => name,
            _ => package.replace('-', "_"),
        };
        let ident = format_ident!("{}", alias);
        quote!(::#ident)
    }
    (
        dependency("instruct-macros-types"),
        dependency("instruct-macros"),
    )
}

pub fn description(attrs: &[Attribute]) -> syn::Result<String> {
    let mut descriptions = Vec::new();
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("description") {
            if let Meta::NameValue(nv) = &attr.meta {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(value),
                    ..
                }) = &nv.value
                {
                    descriptions.push(value.value());
                    continue;
                }
            }
            descriptions.push(attr.parse_args::<LitStr>()?.value());
        } else if attr.path().is_ident("doc") {
            if let Meta::NameValue(nv) = &attr.meta {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(value),
                    ..
                }) = &nv.value
                {
                    docs.push(value.value().trim().to_owned());
                }
            }
        }
    }
    if descriptions.is_empty() {
        Ok(docs.join(" "))
    } else {
        Ok(descriptions.join(" "))
    }
}

pub fn walk_attributes(
    input: &mut DeriveInput,
    mut visit: impl FnMut(&mut Vec<Attribute>) -> syn::Result<()>,
) -> syn::Result<()> {
    visit(&mut input.attrs)?;
    match &mut input.data {
        Data::Struct(data) => {
            for field in &mut data.fields {
                visit(&mut field.attrs)?;
            }
        }
        Data::Enum(data) => {
            for variant in &mut data.variants {
                visit(&mut variant.attrs)?;
                for field in &mut variant.fields {
                    visit(&mut field.attrs)?;
                }
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "unions are unsupported",
            ))
        }
    }
    Ok(())
}

pub fn label(field: &Field, index: usize) -> String {
    field
        .ident
        .as_ref()
        .map_or_else(|| index.to_string(), |i| i.unraw().to_string())
}

/// Build both a destructuring pattern and a constructor from the same fields.
pub fn members(fields: &Fields, mut value: impl FnMut(usize) -> TokenStream) -> TokenStream {
    let tokens = fields.iter().enumerate().map(|(i, field)| {
        let val = value(i);
        if let Some(name) = &field.ident {
            quote!(#name: #val)
        } else {
            val
        }
    });
    match fields {
        Fields::Named(_) => quote!({ #(#tokens),* }),
        Fields::Unnamed(_) => quote!(( #(#tokens),* )),
        Fields::Unit => quote!(),
    }
}

pub fn binding(index: usize) -> Ident {
    format_ident!("__instructor_field_{}", index)
}

pub fn by_variant(
    data: &Data,
    mut emit: impl FnMut(TokenStream, &Fields) -> syn::Result<TokenStream>,
) -> syn::Result<TokenStream> {
    let mut arm = |variant: TokenStream, fields: &Fields| {
        let pattern = members(fields, |i| {
            let id = binding(i);
            quote!(#id)
        });
        let body = emit(variant.clone(), fields)?;
        Ok(quote!(Self #variant #pattern => #body))
    };
    let arms = match data {
        Data::Struct(data) => vec![arm(quote!(), &data.fields)?],
        Data::Enum(data) => data
            .variants
            .iter()
            .map(|v| {
                let ident = &v.ident;
                arm(quote!(::#ident), &v.fields)
            })
            .collect::<syn::Result<_>>()?,
        Data::Union(data) => {
            return Err(syn::Error::new_spanned(
                data.union_token,
                "unions are unsupported",
            ))
        }
    };
    Ok(quote!(match self { #(#arms),* }))
}
