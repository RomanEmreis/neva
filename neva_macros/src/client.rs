//! Macros for MCP clients

use syn::ItemFn;
use proc_macro2::TokenStream;
use quote::quote;

pub fn expand_elicitation(function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(client: &mut neva::Client) {
            client.map_elicitation(#func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::client::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}

pub fn expand_sampling(function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(client: &mut neva::Client) {
            client.map_sampling(#func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::client::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}