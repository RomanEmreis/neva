//! Represents an MCP prompt

#[cfg(feature = "server")]
use super::helpers::TypeCategory;
#[cfg(feature = "server")]
use crate::error::{Error, ErrorCode};
use crate::shared;
#[cfg(feature = "server")]
use crate::shared::BoxFuture;
use crate::types::request::RequestParamsMeta;
#[cfg(feature = "server")]
use crate::types::{ArgNames, FromHandlerArgs, FromRequest};
use crate::types::{Cursor, Icon};
#[cfg(feature = "server")]
use crate::types::{IntoResponse, Page, PropertyType, Request, RequestId, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
#[cfg(feature = "server")]
use std::future::Future;
#[cfg(feature = "server")]
use std::sync::Arc;

#[cfg(feature = "server")]
use crate::app::{
    context::Context,
    handler::{FromHandlerParams, GenericHandler, Handler, HandlerParams, RequestHandler},
};

pub use get_prompt_result::{GetPromptResult, PromptMessage};

#[cfg(feature = "server")]
mod from_request;
mod get_prompt_result;

/// List of commands for Prompts
pub mod commands {
    /// Command name that returns a list of prompts the server has.
    pub const LIST: &str = "prompts/list";

    /// Notification name that indicates that the list of prompts has changed.
    pub const LIST_CHANGED: &str = "notifications/prompts/list_changed";

    /// Command name that returns a prompt provided by the server.
    pub const GET: &str = "prompts/get";
}

/// A prompt or prompt template that the server offers.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Clone, Serialize, Deserialize)]
pub struct Prompt {
    /// The name of the prompt or prompt template.
    pub name: String,

    /// An optional description of what this prompt provides
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// A list of arguments to use for templating the prompt.
    #[serde(rename = "arguments", skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<PromptArgument>>,

    /// Intended for UI and end-user contexts - optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    ///
    /// If not provided, the name should be used for display (except for Tool,
    /// where `annotations.title` should be given precedence over using `name`, if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Optional set of sized icons that the client can display in a user interface.
    ///
    /// Clients that support rendering icons **MUST** support at least the following MIME types:
    /// - `image/png` - PNG images (safe, universal compatibility)
    /// - `image/jpeg` (and `image/jpg`) - JPEG images (safe, universal compatibility)
    ///
    /// Clients that support rendering icons **SHOULD** also support:
    /// - `image/svg+xml` - SVG images (scalable but requires security precautions)
    /// - `image/webp` - WebP images (modern, efficient format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,

    /// A get prompt handler
    #[serde(skip)]
    #[cfg(feature = "server")]
    handler: Option<RequestHandler<GetPromptResult>>,

    /// A list of roles that are allowed to get the prompt
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub(crate) roles: Option<Vec<String>>,

    /// A list of permissions that are allowed to get the prompt
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub(crate) permissions: Option<Vec<String>>,

    /// The names the handler's arguments are read from `arguments` by.
    ///
    /// Server-side only, and always kept in step with [`Self::args`] -- the
    /// argument list a peer sees is the one extraction reads by. See
    /// [`Prompt::with_args`].
    #[serde(skip)]
    #[cfg(feature = "server")]
    pub(crate) arg_names: ArgNames,
}

/// Describes an argument that a prompt can accept.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    /// The name of the argument.
    pub name: String,

    /// A human-readable description of the argument.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// Whether this argument must be provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Sent from the client to request a list of prompts and prompt templates the server has.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct ListPromptsRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// Used by the client to get a prompt provided by the server.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct GetPromptRequestParams {
    /// The name of the prompt or prompt template.
    pub name: String,

    /// Arguments to use for templating the prompt.
    #[serde(rename = "arguments", skip_serializing_if = "Option::is_none")]
    pub args: Option<HashMap<String, Value>>,

    /// Metadata related to the request that provides additional protocol-level information.
    ///
    /// > **Note:** This can include progress tracking tokens and other protocol-specific properties
    /// > that are not part of the primary request parameters.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

/// The server's response to a prompts/list request from the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ListPromptsResult {
    /// A list of prompts or prompt templates that the server offers.
    pub prompts: Vec<Prompt>,

    /// An opaque token representing the pagination position after the last returned result.
    ///
    /// When a paginated result has more data available, the `next_cursor`
    /// field will contain `Some` token that can be used in subsequent requests
    /// to fetch the next page. When there are no more results to return, the `next_cursor` field
    /// will be `None`.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,

    /// How long (in milliseconds) the client may cache this result before
    /// re-fetching, analogous to HTTP `Cache-Control: max-age`.
    ///
    /// Mandatory under MCP 2026-07-28, not an optional hint: `0` means treat
    /// the result as immediately stale. neva always emits it; on the way in a
    /// peer that omits it is read as `0` rather than failing the parse.
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(rename = "ttlMs", default)]
    pub ttl_ms: u64,

    /// Whether this result may be cached across authorization contexts.
    ///
    /// Mandatory under MCP 2026-07-28. Defaults to
    /// [`CacheScope::Private`](crate::types::cache::CacheScope::Private).
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(rename = "cacheScope", default)]
    pub cache_scope: crate::types::CacheScope,
}

#[cfg(feature = "server")]
impl IntoResponse for ListPromptsResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into()),
        }
    }
}

#[cfg(feature = "server")]
impl From<Vec<Prompt>> for ListPromptsResult {
    #[inline]
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    fn from(prompts: Vec<Prompt>) -> Self {
        Self {
            next_cursor: None,
            prompts,
            ..Default::default()
        }
    }
}

#[cfg(feature = "server")]
impl From<Page<'_, Prompt>> for ListPromptsResult {
    #[inline]
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    fn from(page: Page<'_, Prompt>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            prompts: page.items.to_vec(),
            ..Default::default()
        }
    }
}

#[cfg(feature = "server")]
impl ListPromptsResult {
    /// Create a new [`ListPromptsResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for ListPromptsRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for GetPromptRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl From<&str> for PromptArgument {
    #[inline]
    fn from(name: &str) -> Self {
        Self {
            name: name.into(),
            descr: None,
            required: Some(true),
        }
    }
}

#[cfg(feature = "server")]
impl From<Box<str>> for PromptArgument {
    #[inline]
    fn from(name: Box<str>) -> Self {
        Self {
            name: name.into(),
            descr: None,
            required: Some(true),
        }
    }
}

#[cfg(feature = "server")]
impl From<Value> for PromptArgument {
    #[inline]
    fn from(json: Value) -> Self {
        serde_json::from_value(json).expect("A correct PromptArgument value must be provided")
    }
}

#[cfg(feature = "server")]
impl From<String> for PromptArgument {
    #[inline]
    fn from(name: String) -> Self {
        Self {
            name,
            descr: None,
            required: Some(true),
        }
    }
}

#[cfg(feature = "server")]
impl<T: Into<String>> From<(T, T)> for PromptArgument {
    #[inline]
    fn from((name, description): (T, T)) -> Self {
        Self::required(name, description)
    }
}

#[cfg(feature = "server")]
impl<T: Into<String>> From<(T, T, bool)> for PromptArgument {
    #[inline]
    fn from((name, description, required): (T, T, bool)) -> Self {
        Self {
            name: name.into(),
            descr: Some(description.into()),
            required: Some(required),
        }
    }
}

/// Describes a generic get prompt handler
#[cfg(feature = "server")]
pub trait PromptHandler<Args>: GenericHandler<Args> {
    /// Returns the handler's value-carrying arguments, in declaration order.
    ///
    /// Parameters extracted from request metadata ([`crate::types::Meta`],
    /// [`Context`], DI) are not arguments and do not appear here, so the
    /// position in this list is the slot [`ArgNames`] indexes.
    #[inline]
    fn args() -> Option<Vec<PromptArgument>> {
        None
    }
}

#[cfg(feature = "server")]
pub(crate) struct PromptFunc<F, R, Args>
where
    F: PromptHandler<Args, Output = R>,
    R: TryInto<GetPromptResult>,
    R::Error: Into<Error>,
    Args: FromHandlerArgs<GetPromptRequestParams>,
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

#[cfg(feature = "server")]
impl<F, R, Args> PromptFunc<F, R, Args>
where
    F: PromptHandler<Args, Output = R>,
    R: TryInto<GetPromptResult>,
    R::Error: Into<Error>,
    Args: FromHandlerArgs<GetPromptRequestParams>,
{
    /// Creates a new [`PromptFunc`] wrapped into [`Arc`]
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self {
            func,
            _marker: std::marker::PhantomData,
        };
        Arc::new(func)
    }
}

#[cfg(feature = "server")]
impl<F, R, Args> Handler<GetPromptResult> for PromptFunc<F, R, Args>
where
    F: PromptHandler<Args, Output = R>,
    R: TryInto<GetPromptResult>,
    R::Error: Into<Error>,
    Args: FromHandlerArgs<GetPromptRequestParams> + Send + Sync,
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<'_, Result<GetPromptResult, Error>> {
        let HandlerParams::Prompt(params, names) = params else {
            unreachable!()
        };
        Box::pin(async move {
            let args = Args::from_args(params, &names)?;
            self.func.call(args).await.try_into().map_err(Into::into)
        })
    }
}

impl GetPromptRequestParams {
    /// Creates a new [`GetPromptRequestParams`] for the given tool name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: None,
            meta: None,
        }
    }

    /// Specifies tool arguments
    pub fn with_args<Args: shared::IntoArgs>(mut self, args: Args) -> Self {
        self.args = args.into_args();
        self
    }
}

#[cfg(feature = "server")]
impl GetPromptRequestParams {
    /// Includes [`Context`] into request metadata. If metadata is `None` it creates a new.
    pub(crate) fn with_context(mut self, ctx: Context) -> Self {
        self.meta.get_or_insert_default().context = Some(ctx);
        self
    }
}

impl Debug for Prompt {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("name", &self.name)
            .field("title", &self.title)
            .field("descr", &self.descr)
            .field("meta", &self.meta)
            .field("args", &self.args)
            .finish()
    }
}

#[cfg(feature = "server")]
impl Prompt {
    /// Creates a new [`Prompt`]
    #[inline]
    pub fn new<F, R, Args>(name: impl Into<String>, handler: F) -> Self
    where
        F: PromptHandler<Args, Output = R>,
        R: TryInto<GetPromptResult> + Send + 'static,
        R::Error: Into<Error>,
        Args: FromHandlerArgs<GetPromptRequestParams> + Send + Sync + 'static,
    {
        let handler = PromptFunc::new(handler);
        let args = F::args();
        Self {
            name: name.into(),
            title: None,
            descr: None,
            meta: None,
            arg_names: args.as_deref().map(arg_names).unwrap_or_default(),
            args,
            handler: Some(handler),
            #[cfg(feature = "http-server")]
            roles: None,
            #[cfg(feature = "http-server")]
            permissions: None,
            icons: None,
        }
    }

    /// Sets a [`Prompt`] title
    pub fn with_title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    /// Sets a [`Prompt`] description
    pub fn with_description(&mut self, descr: impl Into<String>) -> &mut Self {
        self.descr = Some(descr.into());
        self
    }

    /// Sets arguments for the [`Prompt`]
    ///
    /// The list is both what a peer sees in `prompts/list` and what the
    /// handler's arguments are extracted by: the *n*-th argument here names
    /// the value the handler's *n*-th value-carrying parameter reads. Setting
    /// it therefore cannot leave the published arguments and the handler
    /// disagreeing.
    ///
    /// Parameters served from request metadata -- [`crate::types::Meta`],
    /// [`Context`], a DI-injected `Dc<T>` -- are not arguments and are not
    /// listed here. An `Option<T>` parameter is: list it with
    /// [`PromptArgument::optional`] or [`PromptArgument::named`] so peers know
    /// they may leave it out.
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::Role};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_prompt("analyze", |lang: String, code: String| async move {
    ///     (format!("Analyze this {lang} code: {code}"), Role::User)
    /// })
    /// .with_args(["lang", "code"]);
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn with_args<T, A>(&mut self, args: T) -> &mut Self
    where
        T: IntoIterator<Item = A>,
        A: Into<PromptArgument>,
    {
        let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
        // `declare` keeps the handler's own arity rather than adopting the
        // length of this list, so a list that does not cover every argument
        // stays detectable instead of silently leaving the trailing parameters
        // reading unpublished `argN` keys.
        self.arg_names = self
            .arg_names
            .declare(args.iter().map(|arg| arg.name.as_str()));

        self.args = Some(args);
        self
    }

    /// Describes how the prompt's published argument list and its handler
    /// disagree, if they do.
    ///
    /// The list is what peers are told to send *and* what extraction reads by,
    /// so one that does not cover every argument the handler takes leaves the
    /// trailing parameters reading keys no peer was ever asked for. Worth
    /// catching at startup rather than on a peer's first request.
    pub(crate) fn arg_name_conflict(&self) -> Option<String> {
        if !self.arg_names.is_declared() {
            return None;
        }

        let arity = self.arg_names.arity();
        let declared = self.arg_names.len();
        if declared != arity {
            return Some(format!(
                "prompt `{}` publishes {declared} argument(s) but its handler takes {arity}. \
                 List every argument the handler reads, metadata parameters (`Context`, \
                 `Meta<_>`, `Dc<_>`) excluded.",
                self.name,
            ));
        }

        self.arg_names.duplicate().map(|duplicate| {
            format!(
                "prompt `{}` publishes the argument `{duplicate}` twice. Arguments are read \
                 from a request by name, so two parameters sharing one name would both be \
                 handed the same value.",
                self.name,
            )
        })
    }

    /// Sets the [`Prompt`] icons
    pub fn with_icons(&mut self, icons: impl IntoIterator<Item = Icon>) -> &mut Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }

    /// Sets a list of roles that are allowed to get the prompt
    #[cfg(feature = "http-server")]
    pub fn with_roles<T, I>(&mut self, roles: T) -> &mut Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.roles = Some(roles.into_iter().map(Into::into).collect());
        self
    }

    /// Sets a list of permissions that are allowed to get the prompt
    #[cfg(feature = "http-server")]
    pub fn with_permissions<T, I>(&mut self, permissions: T) -> &mut Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.permissions = Some(permissions.into_iter().map(Into::into).collect());
        self
    }

    /// Get prompt result
    #[inline]
    pub(crate) async fn call(
        &self,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, Error> {
        match self.handler {
            Some(ref handler) => {
                handler
                    .call(HandlerParams::Prompt(params, self.arg_names.clone()))
                    .await
            }
            None => Err(Error::new(
                ErrorCode::InternalError,
                "Prompt handler not specified",
            )),
        }
    }
}

/// The extraction names of a prompt's declared arguments, in order.
#[cfg(feature = "server")]
#[inline]
fn arg_names(args: &[PromptArgument]) -> ArgNames {
    ArgNames::new(args.iter().map(|arg| arg.name.as_str()))
}

/// Prompt arguments helper
#[cfg(feature = "server")]
#[allow(missing_debug_implementations)]
pub struct PromptArguments;

#[cfg(feature = "server")]
impl PromptArguments {
    /// Deserializes a [`Vec`] of [`PromptArgument`] from a JSON string
    #[inline]
    pub fn from_json_str(json: &str) -> Vec<Value> {
        serde_json::from_str(json).expect("PromptArgument: Incorrect JSON string provided")
    }
}

#[cfg(feature = "server")]
impl PromptArgument {
    /// Creates the [`PromptArgument`] for the `slot`-th argument of a handler
    /// whose parameter names are not known -- a bare closure.
    ///
    /// The positional name is what the prompt publishes *and* what extraction
    /// reads by; [`Prompt::with_args`] replaces both together.
    pub(crate) fn positional(slot: usize, required: bool) -> Self {
        Self::named(
            crate::types::helpers::extract::positional_name(slot),
            required,
        )
    }

    /// Creates a [`PromptArgument`] with no description.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::prompt::PromptArgument;
    ///
    /// let arg = PromptArgument::named("tone", false);
    ///
    /// assert_eq!(arg.name, "tone");
    /// assert_eq!(arg.required, Some(false));
    /// ```
    pub fn named<T: Into<String>>(name: T, required: bool) -> Self {
        Self {
            name: name.into(),
            descr: None,
            required: Some(required),
        }
    }

    /// Creates a new required [`PromptArgument`]
    pub fn required<T: Into<String>>(name: T, descr: T) -> Self {
        Self {
            name: name.into(),
            descr: Some(descr.into()),
            required: Some(true),
        }
    }

    /// Creates a new required [`PromptArgument`]
    pub fn optional<T: Into<String>>(name: T, descr: T) -> Self {
        Self {
            name: name.into(),
            descr: Some(descr.into()),
            required: Some(false),
        }
    }
}

macro_rules! impl_generic_prompt_handler ({ $($param:ident)* } => {
    #[cfg(feature = "server")]
    impl<Func, Fut: Send, $($param: TypeCategory,)*> PromptHandler<($($param,)*)> for Func
    where
        Func: Fn($($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {
        #[inline]
        #[allow(unused_mut)]
        fn args() -> Option<Vec<PromptArgument>> {
            let mut args: Vec<PromptArgument> = Vec::new();
            $(
            {
                // Metadata-served parameters are not arguments: they are
                // neither published nor do they consume an argument slot.
                if $param::category() != PropertyType::None {
                    args.push(PromptArgument::positional(
                        args.len(),
                        !$param::is_optional(),
                    ));
                }
            }
            )*
            if args.is_empty() {
                None
            } else {
                Some(args)
            }
        }
    }
});

impl_generic_prompt_handler! {}
impl_generic_prompt_handler! { T1 }
impl_generic_prompt_handler! { T1 T2 }
impl_generic_prompt_handler! { T1 T2 T3 }
impl_generic_prompt_handler! { T1 T2 T3 T4 }
impl_generic_prompt_handler! { T1 T2 T3 T4 T5 }
