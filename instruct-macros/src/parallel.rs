use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse_quote, Data, DeriveInput, Error, Fields};

pub fn derive(input: DeriveInput) -> syn::Result<TokenStream> {
    let library = crate::helpers::library();
    let name = input.ident;
    let Data::Enum(data) = input.data else {
        return Err(Error::new_spanned(name, "ParallelTools requires an enum"));
    };
    if data.variants.is_empty() {
        return Err(Error::new_spanned(
            name,
            "expected at least one tool variant",
        ));
    }
    let mut registrations = Vec::new();
    let mut generics = input.generics;
    for variant in data.variants {
        let ident = variant.ident;
        let field = match &variant.fields {
            Fields::Unnamed(values) if values.unnamed.len() == 1 => &values.unnamed[0].ty,
            _ => {
                return Err(Error::new_spanned(
                    ident,
                    "expected a tuple variant containing one tool model",
                ))
            }
        };
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#field: ::#library::ResponseModel));
        registrations.push(quote!(::#library::ParallelTool::new(Self::#ident)));
    }
    generics
        .make_where_clause()
        .predicates
        .push(parse_quote!(Self: 'static));
    let (params, args, bounds) = generics.split_for_impl();
    Ok(quote! {
        impl #params ::#library::ParallelTools for #name #args #bounds {
            fn tools() -> ::std::vec::Vec<::#library::ParallelTool<Self>> {
                ::std::vec![#(#registrations),*]
            }
        }
    })
}
