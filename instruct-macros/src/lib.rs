//! Derives for Instructor response types and partial snapshots.

mod helpers;
mod partial;
mod snapshots;
mod validation;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::quote;
use syn::{parse_macro_input, parse_quote, Data, DeriveInput};

use helpers::{description, runtime, walk_attributes};

/// Define a typed response and its streaming snapshot from one declaration.
/// Adds Serde, JSON Schema, Instructor validation and PartialModel derives.
#[proc_macro_attribute]
pub fn model(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);
    if !args.is_empty() {
        return syn::Error::new_spanned(Tokens::from(args), "model takes no arguments")
            .to_compile_error()
            .into();
    }
    let (types, macros) = runtime();
    let serde_path = quote!(#types::serde).to_string();
    let schemars_path = quote!(#types::schemars).to_string();
    if let Err(error) = walk_attributes(&mut input, |attrs| {
        if attrs.iter().any(|a| a.path().is_ident("description")) {
            let text = description(attrs)?;
            attrs.push(parse_quote!(#[schemars(description = #text)]));
        }
        Ok(())
    }) {
        return error.into_compile_error().into();
    }
    quote! {
        #[derive(
            #types::serde::Serialize, #types::serde::Deserialize,
            #types::schemars::JsonSchema, #macros::InstructMacro, #macros::PartialModel
        )]
        #[serde(crate = #serde_path)]
        #[schemars(crate = #schemars_path)]
        #[instructor_schema]
        #input
    }
    .into()
}

#[proc_macro_derive(PartialModel)]
pub fn partial_model(input: TokenStream) -> TokenStream {
    partial::derive(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(InstructMacro, attributes(validate, description, instructor_schema))]
pub fn instruct_validate_derive(input: TokenStream) -> TokenStream {
    instruct(&parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn instruct(input: &DeriveInput) -> syn::Result<Tokens> {
    let (types, _) = runtime();
    let name = &input.ident;
    let mut generics = input.generics.clone();
    for ty in generics.type_params_mut() {
        ty.bounds.push(parse_quote!(#types::InstructMacro));
    }
    let model_description = description(&input.attrs)?;
    let info = match &input.data {
        Data::Struct(data) => {
            let mut params = Vec::new();
            for (i, field) in data.fields.iter().enumerate() {
                let ty = &field.ty;
                let label = helpers::label(field, i);
                let text = description(&field.attrs)?;
                params.push(quote! {
                    <#ty as #types::InstructMacro>::parameter(#label.to_string(), #text.to_string())
                });
            }
            quote! {
                #types::InstructMacroResult::Struct(#types::StructInfo {
                    name: stringify!(#name).to_string(),
                    description: #model_description.to_string(),
                    parameters: #types::legacy_parameters::<Self>(|| vec![#(#params),*]),
                    is_optional: false,
                    is_list: false,
                })
            }
        }
        Data::Enum(data) => {
            let variants = data.variants.iter().map(|v| v.ident.to_string());
            quote! {
                #types::InstructMacroResult::Enum(#types::EnumInfo {
                    title: stringify!(#name).to_string(),
                    r#enum: vec![#(#variants.to_string()),*],
                    r#type: stringify!(#name).to_string(),
                    description: #model_description.to_string(),
                    is_optional: false,
                    is_list: false,
                })
            }
        }
        Data::Union(_) => return Err(syn::Error::new_spanned(name, "unions are unsupported")),
    };
    let schema = if input
        .attrs
        .iter()
        .any(|a| a.path().is_ident("instructor_schema"))
    {
        for ty in generics.type_params_mut() {
            ty.bounds.push(parse_quote!(#types::schemars::JsonSchema));
            ty.bounds.push(parse_quote!(#types::serde::Serialize));
            ty.bounds
                .push(parse_quote!(#types::serde::de::DeserializeOwned));
        }
        generics.make_where_clause().predicates.push(parse_quote!(
            Self: #types::schemars::JsonSchema + #types::serde::Serialize + #types::serde::de::DeserializeOwned
        ));
        let prepare = snapshots::method(input, &types)?;
        quote! {
            #prepare
            fn analysis_schema() -> #types::serde_json::Value {
                #types::schemars::schema_for!(Self).to_value()
            }
            fn describe(generator: &mut #types::schemars::SchemaGenerator) -> #types::schemars::Schema {
                <Self as #types::schemars::JsonSchema>::json_schema(generator)
            }
        }
    } else {
        quote!()
    };
    let methods = validation::methods(input, &types)?;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #types::InstructMacro for #name #ty_generics #where_clause {
            fn get_info() -> #types::InstructMacroResult { #info }
            fn model_name() -> String { stringify!(#name).to_string() }
            #schema
            #methods
        }
    })
}

/// Optional marker on a validator function. Preserves its visibility and attributes.
#[proc_macro_attribute]
pub fn validate(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
