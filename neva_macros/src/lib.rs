//! A proc macro implementation for configuring tool

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::{parse_macro_input, ItemFn, FnArg, Pat, Type, Lit, Expr, punctuated::Punctuated, Token, Meta};

/// Maps the function to a tool
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut description = None;
    let mut schema = None;
    let mut roles = None;
    let mut permissions = None;
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

    expanded.into()
}

/// Maps the function to a resource template
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn resource(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut uri = None;
    let mut description = None;
    let mut mime = None;
    let mut annotations = None;
    let mut roles = None;
    let mut permissions = None;
    
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
                            uri = get_str_param(&nv.value);
                        }
                        "descr" => {
                            description = get_str_param(&nv.value);
                        }
                        "mime" => {
                            mime = get_str_param(&nv.value);
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
                #description_code
                #mime_code
                #annotations_code
                #roles_code
                #permission_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

/// Maps the list of resources function
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn resources(_: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;

    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a resource function
        fn #module_name(app: &mut neva::App) {
            app.map_resources(#func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

/// Maps the function to a prompt
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn prompt(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let func_name = &function.sig.ident;
    // Parse the attribute for metadata
    let mut description = None;
    let mut args = None;
    let mut roles = None;
    let mut permissions = None;
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
                            description = get_str_param(&nv.value);   
                        }
                        "args" => {
                            args = get_str_param(&nv.value);
                        }
                        "no_args" => {
                            no_args = get_bool_param(&nv.value);
                        }
                        "roles" => {
                            roles = get_params_arr(&nv.value);
                        }
                        "permissions" => {
                            permissions = get_params_arr(&nv.value);
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
        
        fn #module_name(app: &mut App) {
            app.map_prompt(stringify!(#func_name), #func_name)
                #description_code
                #args_code
                #roles_code
                #permission_code;
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

/// Maps the function to a command handler
#[proc_macro_attribute]
#[cfg(feature = "server")]
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
                        command = get_str_param(&nv.value);
                    }
                }
            },
        }
    }

    let command = command.expect("command parameter must be specified");
    let module_name = syn::Ident::new(&format!("map_{func_name}"), func_name.span());

    // Expand the function and apply the tool functionality
    let expanded = quote! {
        // Original function
        #function
        // Register a handler function
        fn #module_name(app: &mut neva::App) {
            app.map_handler(#command, #func_name);
        }
        neva::macros::inventory::submit! {
            neva::macros::server::ItemRegistrar(#module_name)
        }
    };

    expanded.into()
}

#[proc_macro_attribute]
#[cfg(any(feature = "server", feature = "client"))]
pub fn json_schema(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<syn::Path, Token![,]>::parse_terminated);
    let input = parse_macro_input!(item as syn::DeriveInput);

    let mut include_ser = false;
    let mut include_de = false;

    for path in args {
        if path.is_ident("all") {
            include_ser = true;
            include_de = true;
        } else if path.is_ident("ser") {
            include_ser = true;
        } else if path.is_ident("de") {
            include_de = true;
        }
    }

    let mut derives = vec![quote!(neva::json::JsonSchema)];
    if include_ser {
        derives.push(quote!(serde::Serialize));
    }
    if include_de {
        derives.push(quote!(serde::Deserialize));
    }

    let expanded = quote! {
        #[derive(#(#derives),*)]
        #[schemars(crate = "neva::json::schemars")]
        #input
    };

    TokenStream::from(expanded)
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

#[inline]
fn get_str_param(value: &Expr) -> Option<String> {
    if let Expr::Lit(syn::ExprLit { lit: Lit::Str(lit_str), .. }) = value {
        Some(lit_str.value())
    } else { 
        None   
    }
}

#[inline]
fn get_bool_param(value: &Expr) -> bool {
    if let Expr::Lit(syn::ExprLit { lit: Lit::Bool(lit), .. }) = value {
        lit.value
    } else { 
        false
    }
}

#[inline]
fn get_params_arr(value: &Expr) -> Option<Vec<String>> {
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
