//! A proc macro implementation for configuring tool

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::{parse_macro_input, ItemFn, FnArg, Pat, Type, Lit, Expr, punctuated::Punctuated, Token, Meta};

/// Maps the function to a tool
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut description = None;
    let mut schema = None;
    let mut no_schema = false;

    // Parse the attribute input as key-value pairs
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let parsed_attrs = parser
        .parse(attr)
        .expect("Failed to parse attributes");

    for meta in parsed_attrs {
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
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                description = Some(lit_str.value());
                            }
                        }
                        "schema" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                schema = Some(lit_str.value());
                            }
                        }
                        "no_schema" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Bool(lit), .. }) = &nv.value {
                                if lit.value {
                                    no_schema = true;
                                } 
                            }
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
    
    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());
    
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register the tool with the app
        fn #module_name(app: &mut neva::App) {
            app.map_tool(stringify!(#func_name), #func_name)
                #description_code
                #schema_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

/// Maps the function to a resource template
#[proc_macro_attribute]
pub fn resource(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut uri = None;
    let mut description = None;
    let mut mime = None;
    let mut annotations = None;
    
    // Parse the attribute input as key-value pairs
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let parsed_attrs = parser
        .parse(attr)
        .expect("Failed to parse attributes");

    for meta in parsed_attrs {
        match &meta {
            Meta::Path(_) => {},
            Meta::List(_) => {},
            Meta::NameValue(nv) => {
                if let Some(ident) = nv.path.get_ident() {
                    match ident.to_string().as_str() {
                        "uri" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                uri = Some(lit_str.value());
                            }
                        }
                        "descr" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                description = Some(lit_str.value());
                            }
                        }
                        "mime" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                mime = Some(lit_str.value());
                            }
                        }
                        "annotations" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                annotations = Some(lit_str.value());
                            }
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

    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register resource function
        fn #module_name(app: &mut neva::App) {
            app.map_resource(#uri_code, stringify!(#func_name), #func_name)
                #description_code
                #mime_code
                #annotations_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

/// Maps the list of resources function
#[proc_macro_attribute]
pub fn resources(_: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;

    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register resource function
        fn #module_name(app: &mut neva::App) {
            app.map_resources(#func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

/// Maps the function to a prompt
#[proc_macro_attribute]
pub fn prompt(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut description = None;
    let mut args = None;
    let mut no_args = false;

    // Parse the attribute input as key-value pairs
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let parsed_attrs = parser
        .parse(attr)
        .expect("Failed to parse attributes");

    for meta in parsed_attrs {
        match &meta {
            Meta::Path(path) => {
                if path.is_ident("no_args") {
                    no_args = true;
                }
            },
            Meta::NameValue(nv) => {
                if let Some(ident) = nv.path.get_ident() {
                    match ident.to_string().as_str() {
                        "descr" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                description = Some(lit_str.value());
                            }
                        }
                        "args" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                                args = Some(lit_str.value());
                            }
                        }
                        "no_args" => {
                            if let Expr::Lit(syn::ExprLit { lit: Lit::Bool(lit), .. }) = &nv.value {
                                if lit.value {
                                    no_args = true;
                                }
                            }
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
    let args_code = if let Some(args_json) = args {
        quote! { .with_args(neva::types::prompt::PromptArguments::from_json_str(#args_json)) }
    } else if !no_args {
        let mut arg_entries = Vec::new();
        for arg in &function.sig.inputs {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    let arg_name = pat_ident.ident.to_string();
                    let arg_type = get_arg_type(&pat_type.ty);
                    if !arg_type.eq("none") {
                        arg_entries.push(quote! {
                            #arg_name
                        });   
                    }
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

    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        
        fn #module_name(app: &mut App) {
            app.map_prompt(stringify!(#func_name), #func_name)
                #description_code
                #args_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

/// Maps the function to a command handler
#[proc_macro_attribute]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut command = None;

    // Parse the attribute input as key-value pairs
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let parsed_attrs = parser
        .parse(attr)
        .expect("Failed to parse attributes");

    for meta in parsed_attrs {
        match &meta {
            Meta::Path(_) => {},
            Meta::List(_) => {},
            Meta::NameValue(nv) => {
                if let Some(ident) = nv.path.get_ident() {
                    if let "command" = ident.to_string().as_str() {
                        if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value {
                            command = Some(lit_str.value());
                        }
                    }
                }
            },
        }
    }

    let command = command.expect("command parameter must be specified");
    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register resource function
        fn #module_name(app: &mut neva::App) {
            app.map_handler(#command, #func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

#[inline]
fn get_arg_type(t: &Type) -> &str {
    match t {
        Type::Array(_) => "array",
        Type::Slice(_) => "slice",
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
                "Uri" => "string",
                _ => "object", // Default case for unknown types
            }
        }
        _ => "object", // Default fallback
    }
}
