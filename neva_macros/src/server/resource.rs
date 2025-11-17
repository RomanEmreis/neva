//! Macros for MCP server resources

use syn::{ItemFn, Meta, punctuated::Punctuated, token::Comma};
use super::{get_str_param, get_params_arr};
use proc_macro2::TokenStream;
use quote::quote;

pub(crate) fn expand_resource(attr: &Punctuated<Meta, Comma>, function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut uri = None;
    let mut title = None;
    let mut description = None;
    let mut mime = None;
    let mut annotations = None;
    let mut roles = None;
    let mut permissions = None;

    for meta in attr {
        match &meta {
            Meta::Path(_) => {},
            Meta::List(_) => {},
            Meta::NameValue(nv) => {
                if let Some(ident) = nv.path.get_ident() {
                    match ident.to_string().as_str() {
                        "uri" => {
                            uri = get_str_param(&nv.value);
                        }
                        "title" => {
                            title = get_str_param(&nv.value);
                        }
                        "descr" => {
                            description = get_str_param(&nv.value);
                        }
                        "mime" => {
                            mime = get_str_param(&nv.value);
                        }
                        "annotations" => {
                            annotations = get_str_param(&nv.value);
                        }
                        "roles" => {
                            roles = get_params_arr(&nv.value);
                        }
                        "permissions" => {
                            permissions = get_params_arr(&nv.value);
                        }
                        _ => {}
                    }
                }
            },
        }
    }

    let uri_code = uri.expect("uri parameter must be specified");

    // Generate the function registration and metadata setup
    let description_code = description.map(|desc| {
        quote! { .with_description(#desc) }
    });

    let title_code = title.map(|title| {
        quote! { .with_title(#title) }
    });

    let mime_code = mime.map(|mime| {
        quote! { .with_mime(#mime) }
    });

    let annotations_code = annotations.map(|annotations_json| {
        quote! { 
            .with_annotations(|_| {
                neva::types::Annotations::from_json_str(#annotations_json)
            }) 
        }
    });

    let roles_code = roles.map(|roles| {
        let role_literals = roles.iter().map(|r| quote::quote! { #r });
        quote! { .with_roles([#(#role_literals),*]) }
    });

    let permission_code = permissions.map(|permission| {
        let permission_literals = permission.iter().map(|r| quote::quote! { #r });
        quote! { .with_permissions([#(#permission_literals),*]) }
    });

    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(app: &mut neva::App) {
            app.map_resource(#uri_code, stringify!(#func_name), #func_name)
                #title_code
                #description_code
                #mime_code
                #annotations_code
                #roles_code
                #permission_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}

pub(crate) fn expand_resources(function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(app: &mut neva::App) {
            app.map_resources(#func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}