///! A proc macro implementation for configuring tool

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::{
    parse_macro_input, 
    ItemFn, FnArg, Pat, Type, Lit, Expr, 
    punctuated::Punctuated, 
    MetaNameValue,
    Token
};

#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut description = None;
    let mut schema = None;

    // Parse the attribute input as key-value pairs
    let parser = Punctuated::<MetaNameValue, Token![,]>::parse_terminated;
    let parsed_attrs = parser
        .parse(attr.into())
        .expect("Failed to parse attributes");

    for meta in parsed_attrs {
        if let Some(ident) = meta.path.get_ident() {
            match ident.to_string().as_str() {
                "descr" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        description = Some(lit_str.value());
                    }
                }
                "schema" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        schema = Some(lit_str.value());
                    }
                }
                _ => {}
            }
        }
    }

    // Generate the function registration and metadata setup
    let description_code = description.map(|desc| {
        quote! { .with_description(#desc) }
    });

    // If no schema is provided, generate it automatically from function arguments
    let schema_code = if let Some(schema_json) = schema {
        quote! {
            .with_schema(|schema| {
                let mut schema_data: neva::types::tool::InputSchema = serde_json::from_str(#schema_json).unwrap();
                schema_data.r#type = neva::types::PropertyType::Object;
                schema_data
            })
        }
    } else {
        let mut schema_entries = Vec::new();

        for arg in &function.sig.inputs {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    let arg_name = pat_ident.ident.to_string();
                    let arg_type = match &*pat_type.ty {
                        Type::Path(type_path) => {
                            let type_ident = type_path.path.segments.last().unwrap().ident.to_string();
                            match type_ident.as_str() {
                                "String" => "string",
                                "i16" | "i32" | "i64" | "u32" | "u64" | "usize" => "number",
                                "f32" | "f64" => "number",
                                "bool" => "boolean",
                                _ => "object", // Default case for unknown types
                            }
                        }
                        _ => "object", // Default fallback
                    };

                    schema_entries.push(quote! {
                        .add_property(#arg_name, #arg_type, #arg_type)
                    });
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
    };
    
    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());
    
    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register the tool with the app
        pub fn #module_name(app: &mut neva::App) {
            app.map_tool(stringify!(#func_name), #func_name)
                #description_code
                #schema_code;
        }
    };

    expanded.into()
}

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
    let parser = Punctuated::<MetaNameValue, Token![,]>::parse_terminated;
    let parsed_attrs = parser
        .parse(attr.into())
        .expect("Failed to parse attributes");

    for meta in parsed_attrs {
        if let Some(ident) = meta.path.get_ident() {
            match ident.to_string().as_str() {
                "uri" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        uri = Some(lit_str.value());
                    }
                }
                "descr" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        description = Some(lit_str.value());
                    }
                }
                "mime" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        mime = Some(lit_str.value());
                    }
                }
                "annotations" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        annotations = Some(lit_str.value());
                    }
                }
                _ => {}
            }
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
                let annotations = serde_json::from_str(#annotations_json).unwrap();
                annotations
            }) 
        }
    });

    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register resource function
        pub fn #module_name(app: &mut neva::App) {
            app.map_resource(#uri_code, stringify!(#func_name), #func_name)
                #description_code
                #mime_code
                #annotations_code;
        }
    };

    expanded.into()
}

#[proc_macro_attribute]
pub fn prompt(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut description = None;
    let mut args = None;

    // Parse the attribute input as key-value pairs
    let parser = Punctuated::<MetaNameValue, Token![,]>::parse_terminated;
    let parsed_attrs = parser
        .parse(attr.into())
        .expect("Failed to parse attributes");

    for meta in parsed_attrs {
        if let Some(ident) = meta.path.get_ident() {
            match ident.to_string().as_str() {
                "descr" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        description = Some(lit_str.value());
                    }
                }
                "args" => {
                    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = meta.value {
                        args = Some(lit_str.value());
                    }
                }
                _ => {}
            }
        }
    }

    // Generate the function registration and metadata setup
    let description_code = description.map(|desc| {
        quote! { .with_description(#desc) }
    });

    // If no schema is provided, generate it automatically from function arguments
    let args_code = if let Some(args_json) = args {
        quote! {
            .with_args(serde_json::from_str::<Vec<serde_json::Value>>(#args_json).unwrap())
        }
    } else {
        let mut arg_entries = Vec::new();
        for arg in &function.sig.inputs {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    let arg_name = pat_ident.ident.to_string();
                    arg_entries.push(quote! {
                        #arg_name
                    });
                }
            }
        }
        if !arg_entries.is_empty() {
            quote! {
                .with_args([#(#arg_entries,)*])
            }
        } else {
            quote! {}
        }
    };

    let module_name = syn::Ident::new(&format!("map_{}", func_name), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        
        pub fn #module_name(app: &mut App) {
            app.map_prompt(stringify!(#func_name), #func_name)
                #description_code
                #args_code;
        }
    };

    expanded.into()
}