//! A proc macro implementation for configuring tool

use proc_macro::TokenStream;
use syn::{Token, parse_macro_input, punctuated::Punctuated};

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;
mod shared;

/// Maps the function to a tool
///
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the call, so
/// it suits computation and lookups; blocking I/O belongs in an `async fn`.
///
/// # Synchronous Example
/// ```ignore
/// use neva::prelude::*;
///
/// #[tool(descr = "Sums two numbers")]
/// fn sum(a: i32, b: i32) -> i32 {
///     a + b
/// }
/// ```
///
/// # Blocking Example
/// ```ignore
/// use neva::prelude::*;
///
/// #[tool(descr = "Reads a text file", blocking)]
/// fn read_file(path: String) -> Result<String, Error> {
///     std::fs::read_to_string(path).map_err(|err| Error::new(ErrorCode::InternalError, err))
/// }
/// ```
///
/// # Parameters
/// * `title` - Tool title.
/// * `descr` - Tool description.
/// * `input_schema` - Schema for the tool input.
/// * `output_schema` - Schema for the tool output.
/// * `annotations` - Arbitrary [metadata](https://docs.rs/neva/latest/neva/types/tool/struct.ToolAnnotations.html).
/// * `roles` & `permissions` - Define which users can run the tool when using Streamable HTTP transport with OAuth.
/// * `middleware` - Middleware list to apply to the tool.
/// * `task_support` - Specifies task augmentation support for this tool.
/// * `no_schema` - Explicitly disables input schema generation if it's not set in `input_schema`.
/// * `blocking` - Runs the handler on Tokio's blocking pool instead of the runtime thread that
///   dispatched the call. Applies to a synchronous function; on an `async fn` it is a compile
///   error.
///
/// # Which shape to write
///
/// * The body awaits something -> `async fn`.
/// * The body is computation on data already in hand -> plain `fn`. It runs inline, with no task
///   spawn and no yield point.
/// * The body blocks -- `std::fs`, a synchronous driver, `Command::output`, a long computation ->
///   plain `fn` plus `blocking`. Left inline it would hold a runtime worker for its whole
///   duration.
///
/// `blocking` on a short body is a pessimization: the hand-off to another thread costs more than
/// the body does.
/// * `ui` - MCP Apps: the `ui://` resource that renders this tool's results.
///   Needs neva's `apps` feature; the URI must use the reserved `ui://` scheme.
/// * `visibility` - MCP Apps: who may call the tool, `["model"]`, `["app"]` or
///   both (the default when omitted). Enforcement is the host's job.
///
/// # MCP Apps Example
/// ```ignore
/// use neva::prelude::*;
///
/// #[tool(descr = "The current time.", ui = "ui://clock/app.html")]
/// async fn get_time() -> String {
///     // A UI-bound tool MUST still return meaningful `content`: the model
///     // reads it, and not every client has an iframe.
///     "The time is 12:00 UTC.".into()
/// }
///
/// // Callable by the app, invisible to the model.
/// #[tool(descr = "Re-read the clock.", ui = "ui://clock/app.html", visibility = ["app"])]
/// async fn refresh_clock() -> String {
///     "The time is 12:01 UTC.".into()
/// }
/// ```
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
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the request;
/// add the `blocking` attribute when the body really blocks, and it runs on
/// Tokio's blocking pool instead. See `neva::marker` for which shape to write.
///
/// # Parameters
/// * `uri` - Resource URI.
/// * `title` - Resource title.
/// * `descr` - Resource description.
/// * `mime` - Resource MIME type.
/// * `annotations` - Resource content arbitrary [metadata](https://docs.rs/neva/latest/neva/types/struct.Annotations.html).
/// * `roles` & `permissions` - Define which users can read the resource when using Streamable HTTP transport with OAuth.
/// * `ui_meta` - MCP Apps: the resource's `_meta.ui` block as JSON (`csp`,
///   `permissions`, `domain`, `prefersBorder`). Needs neva's `apps` feature and
///   a `ui://` URI. Keys are checked at compile time -- `_meta` is an open map,
///   so a snake_case typo would otherwise serialize fine and be ignored by every
///   host.
/// * `blocking` - Runs the handler on Tokio's blocking pool. See [`macro@tool`].
///
/// A `uri` on the `ui://` scheme also defaults `mime` to
/// `text/html;profile=mcp-app`; naming a different one is an error, since no
/// host renders an MCP App under another type.
///
/// # MCP Apps Example
/// ```ignore
/// use neva::prelude::*;
///
/// #[resource(
///     uri = "ui://report/{id}",
///     title = "Report",
///     ui_meta = r#"{
///         "csp": { "resourceDomains": ["https://cdn.jsdelivr.net"] },
///         "prefersBorder": false
///     }"#
/// )]
/// async fn report(id: String) -> TextResourceContents {
///     TextResourceContents::new(
///         format!("ui://report/{id}"),
///         format!("<!doctype html><title>Report {id}</title>"))
///         .with_mime(APP_MIME_TYPE)
/// }
/// ```
///
/// # Simple Example
/// ```ignore
/// use neva::prelude::*;
///
/// #[resource(uri = "res://{name}")]
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
///
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the request;
/// add the `blocking` attribute when the body really blocks, and it runs on
/// Tokio's blocking pool instead. See `neva::marker` for which shape to write.
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn resources(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    server::resource::expand_resources(&attr, &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the function to a prompt
///
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the request;
/// add the `blocking` attribute when the body really blocks, and it runs on
/// Tokio's blocking pool instead. See `neva::marker` for which shape to write.
///
/// # Parameters
/// * `title` - Prompt title.
/// * `descr` - Prompt description.
/// * `args` - Prompt arguments.
/// * `no_args` - Explicitly disables argument generation if it's not set in `args`.
/// * `middleware` - Middleware list to apply to the prompt.
/// * `blocking` - Runs the handler on Tokio's blocking pool. See [`macro@tool`].
/// * `roles` & `permissions` - Define which users can read the resource when using Streamable HTTP transport with OAuth.
///
/// # Simple Example
/// ```ignore
/// use neva::prelude::*;
///
/// #[prompt(descr = "Analyze code for potential improvements")]
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
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the request;
/// add the `blocking` attribute when the body really blocks, and it runs on
/// Tokio's blocking pool instead. See `neva::marker` for which shape to write.
///
/// # Parameters
/// * `command` - Command name.
/// * `middleware` - Middleware list to apply to the command.
/// * `blocking` - Runs the handler on Tokio's blocking pool. See [`macro@tool`].
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

/// Maps the completion function
///
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the request;
/// add the `blocking` attribute when the body really blocks, and it runs on
/// Tokio's blocking pool instead. See `neva::marker` for which shape to write.
#[proc_macro_attribute]
#[cfg(feature = "server")]
pub fn completion(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    server::expand_completion(&attr, &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the elicitation handler function
///
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the request;
/// add the `blocking` attribute when the body really blocks, and it runs on
/// Tokio's blocking pool instead. See `neva::marker` for which shape to write.
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
pub fn elicitation(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    client::expand_elicitation(&attr, &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Maps the sampling handler function
///
/// The function may be `async` or synchronous. A synchronous one returns its
/// value directly and runs on the runtime thread that dispatched the request;
/// add the `blocking` attribute when the body really blocks, and it runs on
/// Tokio's blocking pool instead. See `neva::marker` for which shape to write.
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
pub fn sampling(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr = parse_macro_input!(
        attr with Punctuated::<syn::Meta, Token![,]>::parse_terminated
    );
    client::expand_sampling(&attr, &function)
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
