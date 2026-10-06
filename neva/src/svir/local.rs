//! This server's own tools, handed to a model in the same process.

use super::convert;
use super::offer::Offer;
use crate::app::context::ServerRuntime;
use crate::error::{Error, ErrorCode};
use crate::transport::TransportProtoSender;
use crate::types::{CallToolResponse, Message, Request, RequestId, Tool};
use crate::{App, Context};
use ::svir::{ToolCall, ToolResult, Toolbox};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[cfg(feature = "http-server")]
use crate::auth::Claims;

/// The `_meta` keys MCP 2026-07-28 requires of every request.
#[cfg(not(feature = "legacy-spec"))]
const PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
#[cfg(not(feature = "legacy-spec"))]
const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

impl App {
    /// This server's tools as a [`Toolbox`], called in this process: a
    /// function written once as a tool is handed to a model with no MCP
    /// transport in between. See [`LocalTools`].
    ///
    /// Consumes the server, which then serves nothing over MCP: a transport
    /// configured with `with_stdio` or `with_http` is not started.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{App, error::Error};
    ///
    /// # async fn run() -> Result<(), Error> {
    /// let mut app = App::new();
    /// app.map_tool("add", |a: i64, b: i64| async move { a + b });
    ///
    /// let tools = app.into_toolbox().load().await?;
    /// // `svir::Request::new(model).tools(&tools)` offers `add`.
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_toolbox(mut self) -> LocalTools {
        self.prepare();
        self.compose_pipeline();
        LocalTools::new(Some(self.build_runtime(TransportProtoSender::None)))
    }

    /// This server's tools as a [`Toolbox`] as well as over MCP: the server
    /// runs as configured, and the toolbox calls the same tools in this
    /// process. See [`LocalTools`].
    ///
    /// The toolbox is bound to the server once [`Self::run`] has built it, so
    /// [`LocalTools::load`] waits for that; it fails if the server is dropped
    /// without running, or stops before it gets that far.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{App, error::Error};
    ///
    /// # async fn run() -> Result<(), Error> {
    /// let mut app = App::new().with_options(|opt| opt.with_stdio());
    /// app.map_tool("add", |a: i64, b: i64| async move { a + b });
    ///
    /// let (app, tools) = app.with_toolbox();
    /// tokio::spawn(app.run());
    ///
    /// let tools = tools.load().await?;
    /// // Served over stdio, and offered to a model here.
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_toolbox(mut self) -> (Self, LocalTools) {
        let runtime = match &self.toolbox {
            Some(toolbox) => toolbox.subscribe(),
            None => {
                let (toolbox, runtime) = watch::channel(None);
                self.toolbox = Some(toolbox);
                runtime
            }
        };
        (self, LocalTools::bound_later(runtime))
    }
}

/// A server's own tools as a [`Toolbox`], from [`App::into_toolbox`]: each
/// call the model makes is a `tools/call` run in this process, through the
/// same pipeline a call over MCP takes.
///
/// The middleware the server was given sees every call, so its authorization,
/// rate limits and audit apply to the model as to any other caller; a tool
/// gets its [`Context`](crate::Context) and its injected dependencies as it
/// would over MCP. What there is not is a peer: a tool that asks for input --
/// elicitation, sampling, roots -- is refused at once, and its progress and
/// log notifications go nowhere.
///
/// The descriptors are a snapshot, taken by [`Self::load`] and renewed by
/// [`Self::refresh`] -- after `ctx.tools().add(..)` or `remove(..)`, for
/// example. Only the tools in it can be called. The snapshot leaves out a tool
/// that requires roles or permissions the caller does not hold -- a toolbox
/// from [`App::into_toolbox`] or [`App::with_toolbox`] holds none, one from
/// `ctx.tools().toolbox()` holds its request's -- and one that can only be
/// called as a task, as well as, with the `apps` feature, one MCP Apps hides
/// from the model. Beyond that,
/// [`Self::filter`], [`Self::rename`] and [`Self::prefixed`] work as they do
/// on `RemoteTools`.
///
/// [`Toolbox::call_all`] runs the calls concurrently, answering in order.
///
/// # Examples
/// ```no_run
/// use neva::{App, error::Error};
///
/// # async fn run() -> Result<(), Error> {
/// let mut app = App::new();
/// app.map_tool("add", |a: i64, b: i64| async move { a + b });
/// app.map_tool("reset", || async { "done" });
///
/// let tools = app
///     .into_toolbox()
///     .filter(|tool| tool.name != "reset")
///     .load()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct LocalTools {
    /// The runtime the calls run on, once there is one: at once for
    /// [`App::into_toolbox`], when the server runs for [`App::with_toolbox`].
    runtime: watch::Receiver<Option<ServerRuntime>>,
    /// What the caller holds, for a toolbox handed out inside a handler: the
    /// claims of that handler's request.
    #[cfg(feature = "http-server")]
    claims: Option<Arc<dyn Claims>>,
    offer: Offer,
}

impl Debug for LocalTools {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalTools")
            .field("offer", &self.offer)
            .finish_non_exhaustive()
    }
}

impl LocalTools {
    /// A toolbox on `runtime`, already there or not.
    fn new(runtime: Option<ServerRuntime>) -> Self {
        Self::bound_later(watch::channel(runtime).1)
    }

    fn bound_later(runtime: watch::Receiver<Option<ServerRuntime>>) -> Self {
        Self {
            runtime,
            #[cfg(feature = "http-server")]
            claims: None,
            offer: Offer::default(),
        }
    }

    /// The server's tools, called from inside the handler of `ctx`'s request
    /// and holding its claims. See [`Tools::toolbox`](crate::app::context::api::Tools::toolbox).
    pub(crate) fn of_context(ctx: &Context) -> Self {
        #[cfg_attr(not(feature = "http-server"), allow(unused_mut))]
        let mut tools = Self::new(ctx.runtime.clone());
        #[cfg(feature = "http-server")]
        {
            tools.claims = ctx.claims.clone();
        }
        tools
    }

    /// Offers only the tools `filter` keeps.
    ///
    /// Filters add up: a tool is offered only if every one of them keeps it.
    /// Each sees the tool under the server's name, whatever renames come
    /// before or after it.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{App, error::Error};
    ///
    /// # async fn run(app: App) -> Result<(), Error> {
    /// let tools = app
    ///     .into_toolbox()
    ///     .filter(|tool| !tool.name.starts_with("admin_"))
    ///     .load()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn filter<F>(mut self, filter: F) -> Self
    where
        F: Fn(&Tool) -> bool + Send + Sync + 'static,
    {
        self.offer.filter(Box::new(filter));
        self
    }

    /// Offers each tool under the name `rename` makes of it. A call runs the
    /// tool the server knows by the original name.
    ///
    /// Renames add up, in order: each one is given the name the previous ones
    /// made.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{App, error::Error};
    ///
    /// # async fn run(app: App) -> Result<(), Error> {
    /// let tools = app
    ///     .into_toolbox()
    ///     .rename(|name| name.replace('.', "_"))
    ///     .load()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn rename<F>(mut self, rename: F) -> Self
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        self.offer.rename(Box::new(rename));
        self
    }

    /// Offers each tool as `prefix` followed by its name: a [`Self::rename`]
    /// that prepends `prefix`.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{App, error::Error};
    ///
    /// # async fn run(app: App) -> Result<(), Error> {
    /// let tools = app.into_toolbox().prefixed("local_").load().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn prefixed(self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        self.rename(move |name| format!("{prefix}{name}"))
    }

    /// Takes the first snapshot of the server's tools.
    ///
    /// Fails as [`Self::refresh`] does.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{App, error::Error};
    ///
    /// # async fn run(app: App) -> Result<(), Error> {
    /// let tools = app.into_toolbox().load().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn load(self) -> Result<Self, Error> {
        self.refresh().await?;
        Ok(self)
    }

    /// Replaces the snapshot with the server's current tools.
    ///
    /// Fails if a tool that would be offered has a name a model API cannot
    /// carry (`[a-zA-Z0-9_-]{1,64}`, after the renames), or shares its name
    /// with another. Such a tool is refused rather than renamed behind the
    /// caller's back: [`Self::rename`] can give it a name, and
    /// [`Self::filter`] can leave it out. A failed refresh keeps the previous
    /// snapshot.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{error::Error, svir::LocalTools};
    ///
    /// # async fn run(tools: LocalTools) -> Result<(), Error> {
    /// tools.refresh().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn refresh(&self) -> Result<(), Error> {
        let listed = self.bound().await?.options().all_tools().await;
        self.offer
            .replace(listed.iter().filter(|tool| self.may_call(tool)))
    }

    /// The runtime, once the server has built it.
    async fn bound(&self) -> Result<ServerRuntime, Error> {
        let mut runtime = self.runtime.clone();
        let bound = runtime.wait_for(Option::is_some).await.map_err(|_| {
            Error::new(
                ErrorCode::InternalError,
                "The server stopped before it ran, and has no tools to call",
            )
        })?;
        
        bound
            .clone()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "The server is not running"))
    }

    /// Whether the caller's claims reach `tool`: one that requires roles or
    /// permissions the caller does not hold would refuse every call the model
    /// made. Without `http-server` there are no claims, and no tool requires
    /// any.
    fn may_call(&self, tool: &Tool) -> bool {
        #[cfg(feature = "http-server")]
        return tool.required.validate(self.claims.as_deref()).is_ok();
        #[cfg(not(feature = "http-server"))]
        {
            let _ = tool;
            true
        }
    }

    /// Runs `tools/call` of `name` through the server's pipeline.
    async fn execute(
        &self,
        name: String,
        args: HashMap<String, Value>,
    ) -> Result<CallToolResponse, Error> {
        // A snapshot exists only once the runtime does, so a call never
        // waits here.
        let runtime = self
            .runtime
            .borrow()
            .clone()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "The server is not running"))?;

        // A UUID and not a counter: the server tracks requests by id, and a
        // server that also serves over MCP sees its clients' ids beside these.
        let id = RequestId::Uuid(uuid::Uuid::new_v4());
        #[cfg_attr(not(feature = "http-server"), allow(unused_mut))]
        let mut request = Request::new(
            Some(id),
            crate::types::tool::commands::CALL,
            Some(params(name, args)),
        );
        #[cfg(feature = "http-server")]
        {
            request.claims = self.claims.clone();
        }

        // The dispatcher sends the answer, and the sender keeps it. What the
        // pipeline returns is the answer only when a middleware gave one
        // without calling `next`; over a transport that one is never sent.
        let answer = Arc::new(Mutex::new(None));
        let returned = runtime
            .with_sender(TransportProtoSender::InProcess(answer.clone()))
            .answer(Message::Request(request))
            .await;
        let sent = answer.lock().ok().and_then(|mut slot| slot.take());

        sent.or(returned)
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "The call was not answered"))?
            .into_result()
    }
}

impl Toolbox for LocalTools {
    fn tools(&self) -> Vec<::svir::Tool> {
        self.offer.tools()
    }

    async fn call(&self, call: &ToolCall) -> ToolResult {
        let Some(name) = self.offer.server_name(&call.name) else {
            return ToolResult::error(&call.id, format!("There is no tool named `{}`", call.name));
        };

        let args = match convert::arguments(call) {
            Ok(args) => args,
            Err(err) => return ToolResult::error(&call.id, err),
        };

        convert::result(&call.id, self.execute(name, args).await)
    }

    /// Answers the calls concurrently, each a `tools/call` of its own, in the
    /// order they were made.
    fn call_all(&self, calls: &[ToolCall]) -> impl Future<Output = Vec<ToolResult>> + Send
    where
        Self: Sync,
    {
        futures_util::future::join_all(calls.iter().map(|call| self.call(call)))
    }
}

/// The params of a `tools/call` of `name`, with the `_meta` MCP 2026-07-28
/// requires: this protocol version, and a caller that declares it can answer
/// nothing -- so a tool that would ask for input is refused, not left waiting.
fn params(name: String, args: HashMap<String, Value>) -> Value {
    #[cfg_attr(feature = "legacy-spec", allow(unused_mut))]
    let mut params = serde_json::json!({ "name": name, "arguments": args });
    #[cfg(not(feature = "legacy-spec"))]
    {
        params["_meta"] = serde_json::json!({
            PROTOCOL_VERSION: crate::LATEST_PROTOCOL_VERSION,
            CLIENT_CAPABILITIES: {},
        });
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toolbox is handed to whatever drives the model, often a spawned task.
    #[test]
    fn a_toolbox_can_be_shared_across_tasks() {
        fn shared<T: Send + Sync + 'static>() {}
        shared::<LocalTools>();
    }

    /// A toolbox handed out inside a handler holds that request's claims: it
    /// offers what they reach, and the server's own check sees them on the
    /// call.
    #[cfg(feature = "http-server")]
    #[tokio::test]
    async fn a_handlers_toolbox_holds_its_requests_claims() {
        use crate::auth::DefaultClaims;

        let mut app = App::new();
        app.map_tool("wipe", || async { "wiped" })
            .with_roles(["admin"]);
        app.prepare();
        app.compose_pipeline();
        let runtime = app.build_runtime(TransportProtoSender::None);

        let holding = |role: &str| -> Option<Arc<dyn Claims>> {
            Some(Arc::new(DefaultClaims {
                role: Some(role.into()),
                ..Default::default()
            }))
        };
        let offered = |tools: &LocalTools| tools.tools().iter().any(|tool| tool.name == "wipe");

        let admin = runtime.context(None, Default::default(), holding("admin"));
        let tools = LocalTools::of_context(&admin).load().await.expect("load");
        assert!(offered(&tools), "an admin is offered `wipe`");
        let told = tools.call(&ToolCall::new("c1", "wipe", "")).await;
        assert_eq!(told.content, "wiped", "{told:?}");

        let guest = runtime.context(None, Default::default(), holding("guest"));
        let tools = LocalTools::of_context(&guest).load().await.expect("load");
        assert!(!offered(&tools), "a guest is not");
    }
}
