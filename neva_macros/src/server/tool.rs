//! Macros for MCP server tools

use syn::{ItemFn, FnArg, Pat, Meta, ReturnType, punctuated::Punctuated, token::Comma};
use super::{get_str_param, get_params_arr, get_bool_param, get_arg_type, get_inner_type_from_generic};
use proc_macro2::TokenStream;
use quote::quote;

pub fn expand(attr: &Punctuated<Meta, Comma>, function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut description = None;
    let mut input_schema = None;
    let mut output_schema = None;
    let mut annotations = None;
    let mut title = None;
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
                        "title" => {
                            title = get_str_param(&nv.value);
                        }
                        "descr" => {
                            description = get_str_param(&nv.value);
                        }
                        "input_schema" => {
                            input_schema = get_str_param(&nv.value);
                        }
                        "output_schema" => {
                            output_schema = get_str_param(&nv.value);
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

    let title_code = title.map(|title| {
        quote! { .with_title(#title) }
    });

    // If no schema is provided, generate it automatically from function arguments
    let input_schema_code = if let Some(schema_json) = input_schema {
        quote! {
            .with_input_schema(|_| {
                neva::types::tool::ToolSchema::from_json_str(#schema_json)
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
                            .with_required(#arg_name, #arg_type, #arg_type)
                        });
                    }
                }
            }
        }
        if !schema_entries.is_empty() {
            quote! {
                .with_input_schema(|schema| {
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

    // If no schema is provided, generate it automatically from function arguments
    let output_schema_code = if let Some(output_schema_json) = output_schema {
        quote! {
            .with_output_schema(|_| {
                neva::types::tool::ToolSchema::from_json_str(#output_schema_json)
            })
        }
    } else if !no_schema {
        // Extract return type from a function signature
        let return_type_schema = match &function.sig.output {
            ReturnType::Default => {
                // Function returns () - no schema needed
                quote! {}
            },
            ReturnType::Type(_, return_type) => {
                let type_str = get_arg_type(return_type);
                if type_str == "object" {
                    match get_inner_type_from_generic(return_type) {
                        Some(inner_type) => quote! {
                            .with_output_schema(|schema| {
                                schema.with_schema::<#inner_type>()
                            })
                        },
                        None => quote! {
                            .with_output_schema(|schema| {
                                schema.with_schema::<#return_type>()
                            })
                        }
                    }
                } else if type_str == "array" {
                    // For array types
                    quote! {}
                } else {
                    // For primitive types
                    quote! {}
                }
            }
        };
        return_type_schema
    } else { 
        quote! {}
    };

    let annotations_code = annotations.map(|annotations_json| {
        quote! { 
            .with_annotations(|_| {
                neva::types::ToolAnnotations::from_json_str(#annotations_json)
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
        // Register the tool with the app
        fn #module_name(app: &mut neva::App) {
            app.map_tool(stringify!(#func_name), #func_name)
                #title_code
                #description_code
                #input_schema_code
                #output_schema_code
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