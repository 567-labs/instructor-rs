use crate::{
    helpers::{description as docs, library},
    validation::validators,
};
use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use std::collections::HashSet;
use syn::{
    ext::IdentExt, parse_macro_input, spanned::Spanned, Attribute, Data, DeriveInput, Error, Expr,
    ExprArray, Fields, LitBool, LitStr, Path,
};

pub(crate) fn choices(input: TokenStream) -> TokenStream {
    expand_choices(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

pub(crate) fn model(input: TokenStream) -> TokenStream {
    expand_model(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn annotation<'a>(attrs: &'a [Attribute], name: &str) -> syn::Result<Option<&'a Attribute>> {
    let mut matching = attrs.iter().filter(|attr| attr.path().is_ident(name));
    let first = matching.next();
    if let Some(duplicate) = matching.next() {
        return Err(Error::new_spanned(
            duplicate,
            format!("duplicate {name} annotation"),
        ));
    }
    Ok(first)
}

#[derive(Clone, Eq, PartialEq, Hash)]
enum Label {
    Text(String),
    Boolean(bool),
}

fn expand_choices(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let lib = library();
    let api = quote!(::#lib::decisions);
    let name = &input.ident;
    let instructions = docs(&input.attrs)?;
    let checks = validators(&input.attrs, quote!(self), &name.to_string(), true)?;
    if let Some(attr) = annotation(&input.attrs, "choice")? {
        return Err(Error::new_spanned(attr, "apply choice to an enum variant"));
    }
    let Data::Enum(data) = &input.data else {
        return Err(Error::new_spanned(name, "Choices requires an enum"));
    };
    if !(1..=255).contains(&data.variants.len()) {
        return Err(Error::new_spanned(
            name,
            "Choices requires 1 to 255 options",
        ));
    }
    let mut seen = HashSet::new();
    let mut options = Vec::new();
    let mut matches = Vec::new();
    let mut bool_matches = Vec::new();
    for variant in &data.variants {
        let ident = &variant.ident;
        if !matches!(variant.fields, Fields::Unit) {
            return Err(Error::new_spanned(variant, "choices must be unit variants"));
        }
        let mut value = None;
        let mut description = None;
        let mut examples = None;
        if let Some(attr) = annotation(&variant.attrs, "choice")? {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("value") && value.is_none() {
                    let input = meta.value()?;
                    value = Some(if input.peek(LitBool) {
                        Label::Boolean(input.parse::<LitBool>()?.value)
                    } else {
                        Label::Text(input.parse::<LitStr>()?.value())
                    });
                } else if meta.path.is_ident("description") && description.is_none() {
                    let expr = meta.value()?.parse::<Expr>()?;
                    description = Some(quote!(.description(#expr)));
                } else if meta.path.is_ident("examples") && examples.is_none() {
                    examples = Some(meta.value()?.parse::<ExprArray>()?.elems);
                } else {
                    return Err(meta.error("expected value, description or examples once"));
                }
                Ok(())
            })?;
        }
        let value = value.unwrap_or_else(|| Label::Text(ident.unraw().to_string().to_lowercase()));
        if matches!(&value, Label::Text(s) if s.is_empty()) || !seen.insert(value.clone()) {
            return Err(Error::new_spanned(
                ident,
                "choice values must be unique booleans or nonempty strings",
            ));
        }
        let doc = docs(&variant.attrs)?;
        if description.is_none() && !doc.is_empty() {
            description = Some(quote!(.description(#doc)));
        }
        let examples = examples.into_iter().flatten();
        let variant = quote!(::std::option::Option::Some(Self::#ident));
        let choice = match value {
            Label::Text(value) => {
                matches.push(quote!(#value => #variant));
                quote!(#api::Choice::new(#value))
            }
            Label::Boolean(value) => {
                bool_matches.push(quote!(::std::option::Option::Some(#value) => #variant));
                quote!(#api::Choice::boolean(#value))
            }
        };
        options.push(quote!(#choice #description #(.example(#examples))*));
    }
    let (impl_params, params, bounds) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_params #api::Choices for #name #params #bounds {
            fn validate_choice(&mut self, __context: &#api::Value) -> ::std::result::Result<(), ::std::string::String> {
                #checks
                ::std::result::Result::Ok(())
            }
            fn instructions() -> &'static str { #instructions }
            fn options() -> ::std::vec::Vec<#api::Choice> { ::std::vec![#(#options),*] }
            fn from_choice(label: &str) -> ::std::option::Option<Self> {
                match label {
                    #(#matches,)*
                    _ => ::std::option::Option::None,
                }
            }
            fn from_choice_value(value: &#api::Value) -> ::std::option::Option<Self> {
                match value.as_bool() {
                    #(#bool_matches,)*
                    _ => <Self as #api::Choices>::from_choice(value.as_str()?),
                }
            }
        }
    })
}

fn expand_model(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let lib = library();
    let api = quote!(::#lib::decisions);
    let name = &input.ident;
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(name, "DecisionModel requires a struct"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(Error::new_spanned(
            name,
            "DecisionModel requires named fields",
        ));
    };
    if fields.named.is_empty() {
        return Err(Error::new_spanned(
            name,
            "DecisionModel requires at least one question",
        ));
    }
    let mut validate = None;
    if let Some(attr) = annotation(&input.attrs, "decision")? {
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("validate") || validate.is_some() {
                return Err(meta.error("expected validate = function"));
            }
            let function = meta.value()?.parse::<Path>()?;
            validate = Some(quote! { #function(self, __context, __strict)?; });
            Ok(())
        })?;
    }
    let mut questions = Vec::new();
    let mut values = Vec::new();
    let mut checks = proc_macro2::TokenStream::new();
    for field in &fields.named {
        let ident = field.ident.as_ref().expect("named field");
        let key = ident.unraw().to_string();
        let ty = &field.ty;
        let doc = docs(&field.attrs)?;
        checks.extend(quote! {
            #api::DecisionValue::validate_value(&mut self.#ident, __context)
                .map_err(|err| format!("{}: {}", #key, err))?;
        });
        checks.extend(validators(
            &field.attrs,
            quote!(&mut self.#ident),
            &key,
            true,
        )?);
        let question = if let Some(attr) = annotation(&field.attrs, "decision")? {
            let expr = attr.parse_args::<Expr>()?;
            quote_spanned!(expr.span()=> #api::Question::<#ty>::from(#expr))
        } else if doc.is_empty() {
            quote_spanned!(field.span()=> #api::Question::<#ty>::from_enum())
        } else {
            quote_spanned!(field.span()=> #api::Question::<#ty>::new(#doc))
        };
        questions.push(quote!(#question.named(#key)));
        values.push(quote!(#ident: answers.take(#key)?));
    }
    checks.extend(validators(
        &input.attrs,
        quote!(self),
        &name.to_string(),
        true,
    )?);
    let (impl_params, params, bounds) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_params #api::DecisionModel for #name #params #bounds {
            fn questions() -> ::std::vec::Vec<#api::NamedQuestion> {
                ::std::vec![#(#questions),*]
            }
            fn from_answers(mut answers: #api::Answers) -> ::std::result::Result<Self, #api::DecisionError> {
                ::std::result::Result::Ok(Self { #(#values),* })
            }
            fn validate_decision(
                &mut self, __context: &#api::Value, __strict: bool
            ) -> ::std::result::Result<(), ::std::string::String> {
                #checks
                #validate
                ::std::result::Result::Ok(())
            }
        }
    })
}
