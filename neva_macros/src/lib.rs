//! A proc macro implementation for configuring tool

use syn::{parse_macro_input, punctuated::Punctuated, Token};
use proc_macro::TokenStream;

#[cfg(feature = "server")]
mod server;
#[cfg(feature = "client")]
mod client;
mod shared;

/// Maps the function to a tool
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    server::tool::expand(&attr, &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the function to a resource template
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn resource(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    server::resource::expand_resource(&attr, &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the list of resources function
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn resources(_: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    server::resource::expand_resources(&function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the function to a prompt
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn prompt(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    server::prompt::expand(&attr, &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the function to a command handler
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    server::expand_handler(&attr, &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the elicitation handler function
#[proc_macro_attribute]
#[cfg(feature = "client")]
pub fn elicitation(_: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    client::expand_elicitation(&function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the sampling handler function
#[proc_macro_attribute]
#[cfg(feature = "client")]
pub fn sampling(_: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    client::expand_sampling(&function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_attribute]
pub fn json_schema(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::DeriveInput);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Path, Token![,]>::parse_terminated
    );
    shared::expand_json_schema(&attr, &input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
