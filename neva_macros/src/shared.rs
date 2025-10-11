//! Shared macros for MCP clients and servers

use syn::{Path, punctuated::Punctuated, token::Comma};
use proc_macro2::TokenStream;
use quote::quote;

pub fn expand_json_schema(attr: &Punctuated<Path, Comma>, input: &syn::DeriveInput) -> syn::Result<TokenStream>  {
    let mut include_ser = false;
    let mut include_de = false;
    let mut include_debug = false;

    for path in attr {
        if path.is_ident("all") {
            include_ser = true;
            include_de = true;
            include_debug = true;
        } else if path.is_ident("serde") {
            include_ser = true;
            include_de = true;
        } else if path.is_ident("ser") {
            include_ser = true;
        } else if path.is_ident("de") {
            include_de = true;
        } else if path.is_ident("debug") {
            include_debug = true;
        }
    }

    let mut derives = vec![quote!(neva::json::JsonSchema)];
    if include_ser {
        derives.push(quote!(serde::Serialize));
    }
    if include_de {
        derives.push(quote!(serde::Deserialize));
    }
    if include_debug {
        derives.push(quote!(Debug));
    }

    let expanded = quote! {
        #[derive(#(#derives),*)]
        #[schemars(crate = "neva::json::schemars")]
        #input
    };

    Ok(expanded)
}