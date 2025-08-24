//! Macros for MCP server tools

use syn::{ItemFn, FnArg, Pat, Meta, punctuated::Punctuated, token::Comma};
use super::{get_str_param, get_params_arr, get_bool_param, get_arg_type};
use proc_macro2::TokenStream;
use quote::quote;

pub fn expand(attr: &Punctuated<Meta, Comma>, function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut description = None;
    let mut schema = None;
    let mut roles = None;
    let mut permissions = None;
    let mut no_schema = false;

    for meta in attr {
        match &meta {
            Meta::Path(path) => {
                if path.is_ident("no_schema") {
                    no_schema = true;
                }
            },
            Meta::NameValue(nv) => {
                if let Some(ident) = nv.path.get_ident() {
                    match ident.to_string().as_str() {
                        "descr" => {
                            description = get_str_param(&nv.value);
                        }
                        "schema" => {
                            schema = get_str_param(&nv.value);
                        }
                        "roles" => {
                            roles = get_params_arr(&nv.value);
                        }
                        "permissions" => {
                            permissions = get_params_arr(&nv.value);
                        }
                        "no_schema" => {
                            no_schema = get_bool_param(&nv.value);
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

    // If no schema is provided, generate it automatically from function arguments
    let schema_code = if let Some(schema_json) = schema {
        quote! {
            .with_schema(|_| {
                neva::types::tool::InputSchema::from_json_str(#schema_json)
            })
        }
    } else if !no_schema {
        let mut schema_entries = Vec::new();

        for arg in &function.sig.inputs {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    let arg_name = pat_ident.ident.to_string();
                    let arg_type = get_arg_type(&pat_type.ty);
                    if !arg_type.eq("none") {
                        schema_entries.push(quote! {
                            .add_property(#arg_name, #arg_type, #arg_type)
                        });
                    }
                }
            }
        }
        if !schema_entries.is_empty() {
            quote! {
                .with_schema(|schema| {
                    schema
                    #(#schema_entries)*
                })
            }
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

    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register the tool with the app
        fn #module_name(app: &mut neva::App) {
            app.map_tool(stringify!(#func_name), #func_name)
                #description_code
                #schema_code
                #roles_code
                #permission_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}