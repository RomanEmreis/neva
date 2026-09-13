//! Macros for MCP clients

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemFn, Meta, punctuated::Punctuated, token::Comma};

/// Every attribute `#[elicitation]` and `#[sampling]` accept.
const CLIENT_ATTRS: [&str; 1] = ["blocking"];

/// Reads the `blocking` flag off the attribute list, rejecting anything else.
///
/// Both client macros take exactly this one attribute, in either spelling:
/// `#[elicitation(blocking)]` or `#[elicitation(blocking = true)]`.
fn parse_blocking(attr: &Punctuated<Meta, Comma>, kind: &str) -> syn::Result<bool> {
    let mut blocking = false;
    for meta in attr {
        match meta {
            Meta::Path(path) if path.is_ident("blocking") => blocking = true,
            Meta::NameValue(nv) if nv.path.is_ident("blocking") => {
                blocking = crate::shared::get_bool_param(&nv.value);
            }
            Meta::Path(path) => {
                return Err(crate::shared::unknown_attr(
                    path,
                    &crate::shared::path_name(path),
                    kind,
                    &CLIENT_ATTRS,
                ));
            }
            Meta::NameValue(nv) => {
                return Err(crate::shared::unknown_attr(
                    &nv.path,
                    &crate::shared::path_name(&nv.path),
                    kind,
                    &CLIENT_ATTRS,
                ));
            }
            Meta::List(list) => {
                return Err(crate::shared::unknown_attr(
                    list,
                    &crate::shared::path_name(&list.path),
                    kind,
                    &CLIENT_ATTRS,
                ));
            }
        }
    }
    Ok(blocking)
}

pub(super) fn expand_elicitation(
    attr: &Punctuated<Meta, Comma>,
    function: &ItemFn,
) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());
    let blocking = parse_blocking(attr, "elicitation")?;
    let handler_code = crate::shared::handler_code(function, blocking, "elicitation")?;
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(client: &mut neva::Client) {
            client.map_elicitation(#handler_code);
        }
        neva::macros::inventory::submit! {
            neva::macros::client::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}

pub(super) fn expand_sampling(
    attr: &Punctuated<Meta, Comma>,
    function: &ItemFn,
) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());
    let blocking = parse_blocking(attr, "sampling")?;
    let handler_code = crate::shared::handler_code(function, blocking, "sampling")?;
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(client: &mut neva::Client) {
            client.map_sampling(#handler_code);
        }
        neva::macros::inventory::submit! {
            neva::macros::client::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}
