//! A proc macro implementation for configuring tool

use syn::{parse_macro_input, punctuated::Punctuated, Token};
use proc_macro::TokenStream;

#[cfg(feature = "server")]
mod server;
#[cfg(feature = "client")]
mod client;
mod shared;

/// Maps the function to a tool
/// 
/// # Parameters
/// * `title` - Tool title.
/// * `descr` - Tool description.
/// * `input_schema` - Schema for the tool input.
/// * `output_schema` - Schema for the tool output.
/// * `annotations` - Arbitrary [metadata](https://docs.rs/neva/latest/neva/types/tool/struct.ToolAnnotations.html).
/// * `roles` & `permissions` - Define which users can run the tool when using Streamable HTTP transport with OAuth.
/// * `middleware` - Middleware list to apply to the tool.
/// * `no_schema` - Explicitly disables input schema generation if it's not set in `input_schema`.
/// 
/// # Simple Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[tool(descr = "Hello world tool")]
/// async fn say_hello() -> &'static str {
///     "Hello, world!"
/// }
/// ```
/// 
/// # Full Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[derive(serde::Deserialize)]
/// struct Payload {
///     say: String,
///     name: String,
/// }
/// 
/// #[json_schema(ser)]
/// struct Results {
///     message: String,
/// }
/// 
/// #[tool(
///     title = "JSON Hello",
///     descr = "Say from JSON",
///     roles = ["user"],
///     permissions = ["read"],
///     annotations = r#"{
///         "destructiveHint": false,
///         "idempotentHint": true,
///         "openWorldHint": false,
///         "readOnlyHint": false
///     }"#,
///     input_schema = r#"{
///         "properties": {
///             "arg": { 
///                 "type": "object", 
///                 "description": "A message in JSON format", 
///                 "properties": {
///                     "say": { "type": "string", "description": "A message to say" },
///                     "name": { "type": "string", "description": "A name to whom say Hello" }
///                 },
///                 "required": ["say", "name"] 
///             }
///         },
///         "required": ["arg"]
///     }"#,
///     output_schema = r#"{
///         "properties": {
///             "message": { "type": "string", "description": "A message to say" }
///         },
///         "required": ["message"]
///     }"#
/// )]
/// async fn say_json(arg: Json<Payload>) -> Json<Results> {
///     let result = Results { message: format!("{}, {}!", arg.say, arg.name) };
///     result.into()
/// }
/// ```
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
///
/// # Parameters
/// * `uri` - Resource URI.
/// * `title` - Resource title.
/// * `descr` - Resource description.
/// * `mime` - Resource MIME type.
/// * `annotations` - Resource content arbitrary [metadata](https://docs.rs/neva/latest/neva/types/struct.Annotations.html).
/// * `roles` & `permissions` - Define which users can read the resource when using Streamable HTTP transport with OAuth.
/// 
/// # Simple Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[resource(uri = "res://{name}"]
/// async fn get_res(name: String) -> TextResourceContents {
///     TextResourceContents::new(
///         format!("res://{name}"),
///         format!("Some details about resource: {name}"))
/// }
/// ```
///
/// # Full Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[resource(
///     uri = "res://{name}",
///     title = "Read resource",
///     descr = "Some details about resource",
///     mime = "text/plain",
///     roles = ["user"],
///     permissions = ["read"],
///     annotations = r#"{
///         "audience": ["user"],
///         "priority": 1.0
///     }"#
/// )]
/// async fn get_res(name: String) -> TextResourceContents {
///     TextResourceContents::new(
///         format!("res://{name}"),
///         format!("Some details about resource: {name}"))
/// }
/// ```
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
///
/// # Parameters
/// * `title` - Prompt title.
/// * `descr` - Prompt description.
/// * `args` - Prompt arguments.
/// * `no_args` - Explicitly disables argument generation if it's not set in `args`.
/// * `middleware` - Middleware list to apply to the prompt.
/// * `roles` & `permissions` - Define which users can read the resource when using Streamable HTTP transport with OAuth.
/// 
/// # Simple Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[prompt(descr = "Analyze code for potential improvements"]
/// async fn analyze_code(lang: String) -> PromptMessage {
///     PromptMessage::user()
///         .with(format!("Language: {lang}"))
/// }
/// ```
///
/// # Full Example
/// ```ignore
/// use neva::prelude::*;
///
/// #[prompt(
///     title = "Code Analyzer",
///     descr = "Analyze code for potential improvements",
///     roles = ["user"],
///     permissions = ["read"],
///     args = r#"[
///         {
///             "name": "lang", 
///             "description": "A language to use", 
///             "required": true
///         }    
///     ]"#
/// )]
/// async fn analyze_code(lang: String) -> PromptMessage {
///     PromptMessage::user()
///         .with(format!("Language: {lang}"))
/// }
/// ```
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
///
/// # Parameters
/// * `command` - Command name.
/// * `middleware` - Middleware list to apply to the command.
/// 
/// # Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[handler(command = "ping")]
/// async fn ping_handler() {
///     println!("pong");
/// }
/// ```
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
/// 
/// # Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[json_schema(ser)]
/// struct Contact {
///     name: String,
///     email: String,
///     age: u32,
/// }
/// 
/// #[elicitation]
/// async fn elicitation_handler(params: ElicitRequestParams) -> impl Into<ElicitResult> {
///     let contact = Contact {
///         name: "John".to_string(),
///         email: "john@email.com".to_string(),
///         age: 30,
///     };
///     elicitation::Validator::new(params)
///         .validate(contact)
/// }
/// ```
#[proc_macro_attribute]
#[cfg(feature = "client")]
pub fn elicitation(_: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    client::expand_elicitation(&function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the sampling handler function
/// 
/// # Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[sampling]
/// async fn sampling_handler(params: CreateMessageRequestParams) -> CreateMessageResult {
///     CreateMessageResult::assistant()
///         .with_model("o3-mini")
///         .with_content("Some response")
/// }
/// ```
#[proc_macro_attribute]
#[cfg(feature = "client")]
pub fn sampling(_: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    client::expand_sampling(&function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Provides a utility to extract a JSON schema of this type
///
/// # Optional parameters
/// * `all` - Applies also `derive(Debug, serde::Serialize, serde::Deserialize)`.
/// * `serde` - Applies also `derive(serde::Serialize, serde::Deserialize)`.
/// * `ser` - Applies also `derive(serde::Serialize)`.
/// * `de` - Applies also `derive(serde::Deserialize)`.
/// 
/// # Example
/// ```ignore
/// use neva::prelude::*;
/// 
/// #[json_schema(ser)]
/// struct Results {
///     message: String,
/// }
/// ```
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
