//! Macros for MCP prompts

use syn::{ItemFn, FnArg, Pat, Meta, punctuated::Punctuated, token::Comma};
use super::{get_str_param, get_params_arr, get_exprs_arr, get_bool_param, get_arg_type};
use proc_macro2::TokenStream;
use quote::quote;

pub(crate) fn expand(attr: &Punctuated<Meta, Comma>, function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut description = None;
    let mut args = None;
    let mut title = None;
    let mut roles = None;
    let mut permissions = None;
    let mut middleware = None;
    let mut no_args = false;

    for meta in attr {
        match &meta {
            Meta::Path(path) => {
                if path.is_ident("no_args") {
                    no_args = true;
                }
            },
            Meta::NameValue(nv) => {
                if let Some(ident) = nv.path.get_ident() {
                    match ident.to_string().as_str() {
                        "title" => {
                            title = get_str_param(&nv.value);
                        }
                        "descr" => {
                            description = get_str_param(&nv.value);
                        }
                        "args" => {
                            args = get_str_param(&nv.value);
                        }
                        "no_args" => {
                            no_args = get_bool_param(&nv.value);
                        }
                        "roles" => {
                            roles = get_params_arr(&nv.value);
                        }
                        "permissions" => {
                            permissions = get_params_arr(&nv.value);
                        }
                        "middleware" => {
                            middleware = get_exprs_arr(&nv.value);
                        }
                        _ => {}
                    }
                }
            },
            Meta::List(_) => {}
        }
    }

    // Generate the function registration and metadata setup
    let description_code = description.map(|desc| {
        quote! { .with_description(#desc) }
    });

    let title_code = title.map(|title| {
        quote! { .with_title(#title) }
    });

    // If no schema is provided, generate it automatically from function arguments
    let args_code = if let Some(args_json) = args {
        quote! { .with_args(neva::types::prompt::PromptArguments::from_json_str(#args_json)) }
    } else if !no_args {
        let mut arg_entries = Vec::new();
        for arg in &function.sig.inputs {
            if let FnArg::Typed(pat_type) = arg
                && let Pat::Ident(pat_ident) = &*pat_type.pat {
                let arg_name = pat_ident.ident.to_string();
                let arg_type = get_arg_type(&pat_type.ty);
                if !arg_type.eq("none") {
                    arg_entries.push(quote! {
                        #arg_name
                    });
                }
            }
        }
        if !arg_entries.is_empty() {
            quote! { .with_args([#(#arg_entries,)*]) }
        } else {
            quote! {}
        }
    } else {
        quote! {}
    };

    let roles_code = roles.map(|roles| {
        let role_literals = roles.iter().map(|r| quote::quote! { #r });
        quote! { .with_roles([#(#role_literals),*]) }
    });

    let permission_code = permissions.map(|permission| {
        let permission_literals = permission.iter().map(|r| quote::quote! { #r });
        quote! { .with_permissions([#(#permission_literals),*]) }
    });

    let middleware_code = middleware.map(|mws| {
        let mw_calls = mws.iter().map(|mw| {
            quote! { .wrap_prompt(stringify!(#func_name), #mw) }
        });
        quote! { #(#mw_calls)* }
    });

    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        
        fn #module_name(app: &mut App) {
            app
                #middleware_code
                .map_prompt(stringify!(#func_name), #func_name)
                #title_code
                #description_code
                #args_code
                #roles_code
                #permission_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}