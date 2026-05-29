//! Macros for MCP server tools.
//!
//! # JSON Schema 2020-12 (`proto-2026-07-28-rc`)
//!
//! Under the `proto-2026-07-28-rc` feature the generated `inputSchema` /
//! `outputSchema` are full JSON Schema 2020-12 documents:
//!
//! - **Primitive arguments** (`String`, integers, `bool`, `Vec<_>`, …) become
//!   inline primitive property schemas, exactly as before.
//! - **Structured arguments** passed as `Json<T>` produce a rich, self-contained
//!   subschema when the inner `T` derives `JsonSchema` (via
//!   `#[derive(neva::json_schema)]` or `schemars::JsonSchema`). An inner type
//!   that does **not** derive it degrades gracefully to `{"type":"object"}`.
//!   Deriving is therefore recommended for structured argument and return types.
//!   No `schemars` dependency is required in your crate — it is re-exported by
//!   neva.
//! - **Recursive types cannot be inlined**; model them with an explicit
//!   `input_schema = "…"` instead.
//! - **Explicit `input_schema` / `output_schema` string literals** are validated
//!   at compile time; malformed JSON is a compile error (on every feature
//!   configuration).

use super::{
    get_arg_type, get_bool_param, get_exprs_arr, get_inner_type_from_generic, get_params_arr,
    get_str_param,
};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, Meta, Pat, ReturnType, punctuated::Punctuated, token::Comma};

pub(crate) fn expand(
    attr: &Punctuated<Meta, Comma>,
    function: &ItemFn,
) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut description = None;
    let mut input_schema = None;
    let mut output_schema = None;
    let mut annotations = None;
    let mut title = None;
    let mut roles = None;
    let mut permissions = None;
    let mut middleware = None;
    let mut task_support = None;
    let mut no_schema = false;

    for meta in attr {
        match &meta {
            Meta::Path(path) => {
                if path.is_ident("no_schema") {
                    no_schema = true;
                }
            }
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
                            if let Some(ref js) = input_schema {
                                super::validate_schema_json(js, &nv.value, "input_schema")?;
                            }
                        }
                        "output_schema" => {
                            output_schema = get_str_param(&nv.value);
                            if let Some(ref js) = output_schema {
                                super::validate_schema_json(js, &nv.value, "output_schema")?;
                            }
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
                        "middleware" => {
                            middleware = get_exprs_arr(&nv.value);
                        }
                        "task_support" => {
                            task_support = get_str_param(&nv.value);
                        }
                        "no_schema" => {
                            no_schema = get_bool_param(&nv.value);
                        }
                        _ => {}
                    }
                }
            }
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

    // If no schema is provided, generate it automatically from function arguments.
    let input_schema_code = if let Some(schema_json) = input_schema {
        if cfg!(feature = "proto-2026-07-28-rc") {
            quote! {
                .with_input_schema(|_| {
                    neva::types::schema_2020::InputSchema::from_json_str(#schema_json).unwrap_or_default()
                })
            }
        } else {
            quote! {
                .with_input_schema(|_| {
                    neva::types::tool::ToolSchema::from_json_str(#schema_json)
                })
            }
        }
    } else if !no_schema {
        if cfg!(feature = "proto-2026-07-28-rc") {
            // RC: assemble a JSON Schema 2020-12 object schema via neva helpers
            // so generated code never names `serde_json`. Primitive args use
            // `primitive_subschema`; object/custom args use
            // `__tool_arg_subschema!` (rich-or-fallback).
            let mut prop_pairs = Vec::new();
            let mut required = Vec::new();
            for arg in &function.sig.inputs {
                if let FnArg::Typed(pat_type) = arg
                    && let Pat::Ident(pat_ident) = &*pat_type.pat
                {
                    let arg_name = pat_ident.ident.to_string();
                    let cat = get_arg_type(&pat_type.ty);
                    if cat == "none" {
                        continue;
                    }
                    let ty = &*pat_type.ty;
                    let prop_value = if cat == "object" {
                        // Structured args arrive wrapped (e.g. `Json<T>`); probe
                        // the inner type so a `JsonSchema`-deriving `T` yields a
                        // rich schema. Bare `Value` (no inner) probes itself.
                        let probe_ty = get_inner_type_from_generic(ty).unwrap_or(ty);
                        quote! { neva::__tool_arg_subschema!(#probe_ty) }
                    } else {
                        let json_type = if cat == "slice" { "array" } else { cat };
                        quote! { neva::__macro_support::primitive_subschema(#json_type) }
                    };
                    prop_pairs.push(quote! { (#arg_name.to_string(), #prop_value) });
                    required.push(arg_name);
                }
            }
            if prop_pairs.is_empty() {
                quote! {}
            } else {
                quote! {
                    .with_input_schema(|_| {
                        neva::__macro_support::object_schema(
                            ::std::vec![ #(#prop_pairs),* ],
                            ::std::vec![ #(#required.to_string()),* ],
                        )
                    })
                }
            }
        } else {
            let mut schema_entries = Vec::new();
            for arg in &function.sig.inputs {
                if let FnArg::Typed(pat_type) = arg
                    && let Pat::Ident(pat_ident) = &*pat_type.pat
                {
                    let arg_name = pat_ident.ident.to_string();
                    let arg_type = get_arg_type(&pat_type.ty);
                    if !arg_type.eq("none") {
                        schema_entries.push(quote! {
                            .with_required(#arg_name, #arg_type, #arg_type)
                        });
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
        }
    } else {
        quote! {}
    };

    // If no schema is provided, generate it automatically from the return type.
    let output_schema_code = if let Some(output_schema_json) = output_schema {
        if cfg!(feature = "proto-2026-07-28-rc") {
            quote! {
                .with_output_schema(|_| {
                    neva::types::schema_2020::InputSchema::from_json_str(#output_schema_json).unwrap_or_default()
                })
            }
        } else {
            quote! {
                .with_output_schema(|_| {
                    neva::types::tool::ToolSchema::from_json_str(#output_schema_json)
                })
            }
        }
    } else if !no_schema {
        match &function.sig.output {
            ReturnType::Default => quote! {},
            ReturnType::Type(_, return_type) => {
                let type_str = get_arg_type(return_type);
                if type_str == "object" {
                    let target = match get_inner_type_from_generic(return_type) {
                        Some(inner_type) => quote! { #inner_type },
                        None => quote! { #return_type },
                    };
                    if cfg!(feature = "proto-2026-07-28-rc") {
                        quote! {
                            .with_output_schema(|_| {
                                neva::types::schema_2020::InputSchema::from_schema::<#target>()
                            })
                        }
                    } else {
                        quote! {
                            .with_output_schema(|schema| {
                                schema.with_schema::<#target>()
                            })
                        }
                    }
                } else {
                    // array / primitive return types: no output schema (parity).
                    quote! {}
                }
            }
        }
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

    let middleware_code = middleware.map(|mws| {
        let mw_calls = mws.iter().map(|mw| {
            quote! { .wrap_tool(stringify!(#func_name), #mw) }
        });
        quote! { #(#mw_calls)* }
    });

    let task_support_code = task_support.map(|ts| {
        quote! { .with_task_support(#ts) }
    });

    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register the tool with the app
        fn #module_name(app: &mut neva::App) {
            app
                #middleware_code
                .map_tool(stringify!(#func_name), #func_name)
                #title_code
                #description_code
                #input_schema_code
                #output_schema_code
                #annotations_code
                #roles_code
                #permission_code
                #task_support_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}
