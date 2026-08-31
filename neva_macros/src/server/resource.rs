//! Macros for MCP server resources

use super::{get_exprs_arr, get_params_arr, get_str_param};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemFn, Meta, punctuated::Punctuated, token::Comma};

/// Every attribute `#[resource]` accepts.
const RESOURCE_ATTRS: [&str; 8] = [
    "uri",
    "title",
    "descr",
    "mime",
    "annotations",
    "roles",
    "permissions",
    "ui_meta",
];

/// Every attribute `#[resources]` accepts.
const RESOURCES_ATTRS: [&str; 1] = ["middleware"];

pub(crate) fn expand_resource(
    attr: &Punctuated<Meta, Comma>,
    function: &ItemFn,
) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut uri = None;
    let mut title = None;
    let mut description = None;
    let mut mime = None;
    let mut annotations = None;
    let mut roles = None;
    let mut permissions = None;
    let mut mime_expr = None;
    let mut ui_meta_expr = None;

    for meta in attr {
        match &meta {
            Meta::Path(path) => {
                return Err(super::unknown_attr(
                    path,
                    &super::path_name(path),
                    "resource",
                    &RESOURCE_ATTRS,
                ));
            }
            Meta::List(list) => {
                return Err(super::unknown_attr(
                    list,
                    &super::path_name(&list.path),
                    "resource",
                    &RESOURCE_ATTRS,
                ));
            }
            Meta::NameValue(nv) => {
                let Some(ident) = nv.path.get_ident() else {
                    return Err(super::unknown_attr(
                        &nv.path,
                        &super::path_name(&nv.path),
                        "resource",
                        &RESOURCE_ATTRS,
                    ));
                };
                {
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
                            mime_expr = Some(nv.value.clone());
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
                        "ui_meta" => {
                            ui_meta_expr = Some(nv.value.clone());
                        }
                        other => {
                            return Err(super::unknown_attr(
                                &nv.path,
                                other,
                                "resource",
                                &RESOURCE_ATTRS,
                            ));
                        }
                    }
                }
            }
        }
    }

    let uri_code = uri.expect("uri parameter must be specified");

    // A `ui://` URI is what marks a resource as an MCP App, so it -- not a
    // separate flag -- decides the MIME type and gates the app-only attributes.
    let mime = super::apps::resolve_mime(Some(&uri_code), mime.zip(mime_expr))?;
    let ui_meta_code = ui_meta_expr
        .map(|expr| super::apps::resource_ui_meta_code(&expr, Some(&uri_code)))
        .transpose()?;

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
                #permission_code
                #ui_meta_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}

pub(crate) fn expand_resources(
    attr: &Punctuated<Meta, Comma>,
    function: &ItemFn,
) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut middleware = None;

    for meta in attr {
        match &meta {
            Meta::Path(path) => {
                return Err(super::unknown_attr(
                    path,
                    &super::path_name(path),
                    "resources",
                    &RESOURCES_ATTRS,
                ));
            }
            Meta::List(list) => {
                return Err(super::unknown_attr(
                    list,
                    &super::path_name(&list.path),
                    "resources",
                    &RESOURCES_ATTRS,
                ));
            }
            Meta::NameValue(nv) => match nv.path.get_ident().map(ToString::to_string).as_deref() {
                Some("middleware") => middleware = get_exprs_arr(&nv.value),
                _ => {
                    return Err(super::unknown_attr(
                        &nv.path,
                        &super::path_name(&nv.path),
                        "resources",
                        &RESOURCES_ATTRS,
                    ));
                }
            },
        }
    }

    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());
    let middleware_code = middleware.map(|mws| {
        let mw_calls = mws.iter().map(|mw| {
            quote! { .wrap_list_resources(#mw) }
        });
        quote! { #(#mw_calls)* }
    });

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(app: &mut neva::App) {
            app
                #middleware_code
                .map_resources(#func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}
