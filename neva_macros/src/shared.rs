//! Shared macros for MCP clients and servers

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, ItemFn, Lit, Path, punctuated::Punctuated, token::Comma};

pub(super) fn expand_json_schema(
    attr: &Punctuated<Path, Comma>,
    input: &syn::DeriveInput,
) -> syn::Result<TokenStream> {
    let mut include_ser = false;
    let mut include_de = false;
    let mut include_debug = false;

    for path in attr {
        if path.is_ident("all") {
            include_ser = true;
            include_de = true;
            include_debug = true;
        } else if path.is_ident("serde") {
            include_ser = true;
            include_de = true;
        } else if path.is_ident("ser") {
            include_ser = true;
        } else if path.is_ident("de") {
            include_de = true;
        } else if path.is_ident("debug") {
            include_debug = true;
        }
    }

    let mut derives = vec![quote!(neva::json::JsonSchema)];
    if include_ser {
        derives.push(quote!(serde::Serialize));
    }
    if include_de {
        derives.push(quote!(serde::Deserialize));
    }
    if include_debug {
        derives.push(quote!(Debug));
    }

    let expanded = quote! {
        #[derive(#(#derives),*)]
        #[schemars(crate = "neva::json::schemars")]
        #input
    };

    Ok(expanded)
}

/// The handler as it is registered: `blocking` wraps it in `neva::blocking`,
/// everything else passes the function through untouched.
///
/// `blocking` moves the body onto Tokio's blocking pool, which only a
/// synchronous function has any reason to do: an `async fn` already yields, and
/// `neva::blocking` would reject it anyway -- with a far less obvious message
/// than the one raised here.
pub(crate) fn handler_code(
    function: &ItemFn,
    blocking: bool,
    kind: &str,
) -> syn::Result<TokenStream> {
    let func_name = &function.sig.ident;
    if !blocking {
        return Ok(quote! { #func_name });
    }
    if let Some(asyncness) = function.sig.asyncness {
        return Err(syn::Error::new_spanned(
            asyncness,
            format!(
                "`#[{kind}(blocking)]` applies to a synchronous fn; this one is `async fn`. \
                 Drop `blocking`, or drop `async` and return the value directly."
            ),
        ));
    }
    Ok(quote! { neva::blocking(#func_name) })
}

/// Refuses an attribute the macro does not know.
///
/// Every one of these loops used to end in `_ => {}`, so a misspelled attribute
/// compiled and did nothing. That is quietly wrong for `descr` and dangerous for
/// `visibility`: `#[tool(visiblity = ["app"])]` would take the model-visible
/// default, publishing to the agent a tool the author meant to keep for the app.
pub(crate) fn unknown_attr<T: quote::ToTokens>(
    spanned: &T,
    name: &str,
    macro_name: &str,
    known: &[&str],
) -> syn::Error {
    syn::Error::new_spanned(
        spanned,
        format!(
            "unknown attribute `{name}` on `#[{macro_name}]`, expected one of: {}",
            known.join(", ")
        ),
    )
}

/// How to name a path in a diagnostic: its ident, or the whole path when it has
/// no single one.
pub(crate) fn path_name(path: &syn::Path) -> String {
    path.get_ident().map_or_else(
        || quote!(#path).to_string().replace(' ', ""),
        |ident| ident.to_string(),
    )
}

#[inline]
pub(crate) fn get_bool_param(value: &Expr) -> bool {
    if let Expr::Lit(syn::ExprLit {
        lit: Lit::Bool(lit),
        ..
    }) = value
    {
        lit.value
    } else {
        false
    }
}
