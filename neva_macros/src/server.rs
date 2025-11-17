//! Macros for MCP servers

use syn::{Expr, Lit, Type};
use syn::{ItemFn, Meta, punctuated::Punctuated, token::Comma};
use proc_macro2::TokenStream;
use quote::quote;

pub(crate) mod tool;
pub(crate) mod resource;
pub(super) mod prompt;

pub(super) fn expand_handler(attr: &Punctuated<Meta, Comma>, function: &ItemFn) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    let mut command = None;
    let mut middleware = None;

    for meta in attr {
        match &meta {
            Meta::Path(_) => {},
            Meta::List(_) => {},
            Meta::NameValue(nv) => {
                if let Some(ident) = nv.path.get_ident() {
                    match ident.to_string().as_str() {
                        "command" => {
                            command = get_str_param(&nv.value);
                        }
                        "middleware" => {
                            middleware = get_exprs_arr(&nv.value);
                        }
                        _ => {}
                    } 
                }
            },
        }
    }

    let command = command.expect("command parameter must be specified");
    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());
    let middleware_code = middleware.map(|mws| {
        let mw_calls = mws.iter().map(|mw| {
            quote! { .wrap_command(#command, #mw) }
        });
        quote! { #(#mw_calls)* }
    });
    
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a handler function
        fn #module_name(app: &mut neva::App) {
            app
                #middleware_code
                .map_handler(#command, #func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    Ok(expanded)
}

#[inline]
pub(super) fn get_arg_type(t: &Type) -> &str {
    match t {
        Type::Array(_) => "array",
        Type::Slice(_) => "slice",
        Type::Reference(_) => "none",
        Type::Path(type_path) => {
            let type_ident = type_path.path.segments
                .last()
                .unwrap()
                .ident
                .to_string();
            match type_ident.as_str() {
                "String" => "string",
                "str" => "string",
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => "number",
                "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => "number",
                "f32" | "f64" => "number",
                "bool" => "boolean",
                "Vec" => "array",
                "Context" => "none",
                "Meta" => "none",
                "Result" => "none",
                "Option" => "none",
                "Uri" => "string",
                "Error" => "none",
                _ => "object", // Default case for unknown types
            }
        }
        _ => "object", // Default fallback
    }
}

#[inline]
pub(super) fn get_inner_type_from_generic(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last() {
        match segment.ident.to_string().as_str() {
            "Result" | "Option" | "Vec" | "Meta" | "Json" => {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments
                    && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                    return Some(inner_ty);
                }
            }
            _ => {} 
        }
    }
    None
}


#[inline]
pub(super) fn get_str_param(value: &Expr) -> Option<String> {
    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = value {
        Some(lit_str.value())
    } else {
        None
    }
}

#[inline]
pub(super) fn get_bool_param(value: &Expr) -> bool {
    if let Expr::Lit(syn::ExprLit { lit: Lit::Bool(lit), .. }) = value {
        lit.value
    } else {
        false
    }
}

#[inline]
pub(super) fn get_params_arr(value: &Expr) -> Option<Vec<String>> {
    match value {
        Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) => {
            Some(vec![lit_str.value()])
        }
        Expr::Array(array) => {
            let mut role_list = Vec::new();
            for elem in &array.elems {
                if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = elem {
                    role_list.push(lit_str.value());
                }
            }
            if !role_list.is_empty() {
                Some(role_list)
            } else {
                None
            }
        }
        _ => None
    }
}

#[inline]
pub(super) fn get_exprs_arr(value: &Expr) -> Option<Vec<Expr>> {
    match value {
        Expr::Array(array) => {
            let mut exprs = Vec::new();
            for elem in &array.elems {
                exprs.push(elem.clone());
            }
            if !exprs.is_empty() {
                Some(exprs)
            } else {
                None
            }
        }
        expr => {
            Some(vec![expr.clone()])
        }
    }
}
