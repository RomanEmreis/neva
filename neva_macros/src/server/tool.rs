//! Macros for MCP server tools.
//!
//! # JSON Schema 2020-12 (MCP 2026-07-28)
//!
//! Under MCP 2026-07-28 the generated `inputSchema` /
//! `outputSchema` are full JSON Schema 2020-12 documents:
//!
//! - **Primitive arguments** (`String`, integers, `bool`, `Vec<_>`, ...) become
//!   inline primitive property schemas, exactly as before.
//! - **Structured arguments** passed as `Json<T>` produce a rich, self-contained
//!   subschema when the inner `T` derives `JsonSchema` (via
//!   `#[derive(neva::json_schema)]` or `schemars::JsonSchema`). An inner type
//!   that does **not** derive it degrades gracefully to `{"type":"object"}`.
//!   Deriving is therefore recommended for structured argument and return types.
//!   No `schemars` dependency is required in your crate -- it is re-exported by
//!   neva.
//! - **Recursive types cannot be inlined**; model them with an explicit
//!   `input_schema = "..."` instead.
//! - **Explicit `input_schema` / `output_schema` string literals** are validated
//!   at compile time; malformed JSON is a compile error (on every feature
//!   configuration).

use super::{
    get_arg_type, get_bool_param, get_exprs_arr, get_inner_type_from_generic, get_option_inner,
    get_param_type, get_params_arr, get_str_param, param_idents_and_types,
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
    let mut ui_code = None;
    let mut visibility_code = None;

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
                        "ui" => {
                            ui_code = Some(super::apps::tool_ui_code(&nv.value)?);
                        }
                        "visibility" => {
                            visibility_code = Some(super::apps::tool_visibility_code(&nv.value)?);
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

    // The names of the value-carrying parameters, in declaration order.
    //
    // Arguments are extracted from a call's `arguments` map by name, and these
    // are the only place the handler's own names survive to runtime -- Rust
    // keeps no parameter names past compilation.
    //
    // Which parameters count is decided by `neva::__arg_names!` from the
    // *resolved* type rather than here from its spelling: metadata-served
    // parameters (`Context`, `Meta<_>`, `Dc<_>`) may reach the signature
    // through a type alias, which this macro cannot see through but trait
    // resolution can. Deciding it syntactically would name an argument
    // `ToolHandler::args` does not count, and `App::run` refuses to start on
    // exactly that disagreement.
    let params = param_idents_and_types(function);
    let arg_names_code = if params.is_empty() {
        quote! {}
    } else {
        let pairs = params.iter().map(|(name, ty)| quote! { #name: #ty });
        quote! { .with_arg_names(neva::__arg_names!(#(#pairs),*)) }
    };

    // If no schema is provided, generate it automatically from function arguments.
    let input_schema_code = if let Some(schema_json) = input_schema {
        if cfg!(not(feature = "legacy-spec")) {
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
        if cfg!(not(feature = "legacy-spec")) {
            // 2026-07-28: assemble a JSON Schema 2020-12 object schema via neva helpers
            // so generated code never names `serde_json`. Primitive args use
            // `primitive_subschema`; object/custom args use
            // `__tool_arg_subschema!` (rich-or-fallback).
            let mut entries = Vec::new();
            for arg in &function.sig.inputs {
                if let FnArg::Typed(pat_type) = arg
                    && let Pat::Ident(pat_ident) = &*pat_type.pat
                {
                    let arg_name = pat_ident.ident.to_string();
                    if get_param_type(&pat_type.ty).0 == "none" {
                        continue;
                    }
                    // An `Option<T>` argument is described by its `T`, so the
                    // subschema is probed past the `Option` first. Structured
                    // args arrive wrapped (e.g. `Json<T>`); probe the inner
                    // type so a `JsonSchema`-deriving `T` yields a rich
                    // schema. Bare `Value` (no inner) probes itself.
                    let ty = get_option_inner(&pat_type.ty).unwrap_or(&pat_type.ty);
                    let probe_ty = get_inner_type_from_generic(ty).unwrap_or(ty);
                    // Everything the schema says about the parameter -- whether
                    // it is an argument, whether it is required, and which JSON
                    // type it publishes -- is settled from the resolved type
                    // for the same reason the names are: a type alias hides
                    // `Meta<_>`, `Option<_>` and the underlying primitive alike
                    // from a syntactic test, and a schema that disagrees with
                    // `ToolHandler::args` describes a call the handler will not
                    // read. The probe stays syntactic because no trait can name
                    // the `T` inside an aliased `Json<T>`.
                    let param_ty = &*pat_type.ty;
                    entries.push(quote! {
                        if <#param_ty as neva::__macro_support::IsArgument>::is_argument() {
                            let category = <#param_ty as neva::__macro_support::IsArgument>::category();
                            let subschema = if category == "object" {
                                neva::__tool_arg_subschema!(#probe_ty)
                            } else {
                                neva::__macro_support::primitive_subschema(category)
                            };
                            props.push((#arg_name.to_string(), subschema));
                            if <#param_ty as neva::__macro_support::IsArgument>::is_required() {
                                required.push(#arg_name.to_string());
                            }
                        }
                    });
                }
            }
            if entries.is_empty() {
                quote! {}
            } else {
                quote! {
                    .with_input_schema(|_| {
                        let mut props = ::std::vec::Vec::new();
                        let mut required = ::std::vec::Vec::new();
                        #(#entries)*
                        neva::__macro_support::object_schema(props, required)
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
                    if get_param_type(&pat_type.ty).0 == "none" {
                        continue;
                    }
                    // See the 2026-07-28 branch: whether the parameter is an
                    // argument, whether it is required and which JSON type it
                    // publishes all come from the resolved type, so the schema
                    // cannot disagree with the handler.
                    let param_ty = &*pat_type.ty;
                    schema_entries.push(quote! {
                        let schema = if <#param_ty as neva::__macro_support::IsArgument>::is_argument() {
                            let category = <#param_ty as neva::__macro_support::IsArgument>::category();
                            if <#param_ty as neva::__macro_support::IsArgument>::is_required() {
                                schema.with_required(#arg_name, category, category)
                            } else {
                                schema.with_prop(#arg_name, category, category)
                            }
                        } else {
                            schema
                        };
                    });
                }
            }
            if !schema_entries.is_empty() {
                quote! {
                    .with_input_schema(|schema| {
                        #(#schema_entries)*
                        schema
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
        if cfg!(not(feature = "legacy-spec")) {
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
                    if cfg!(not(feature = "legacy-spec")) {
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
                #arg_names_code
                #title_code
                #description_code
                #input_schema_code
                #output_schema_code
                #annotations_code
                #roles_code
                #permission_code
                #task_support_code
                #ui_code
                #visibility_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}
