//! The primitives this server serves, as a handler reaches them: one namespace
//! per kind, the same shape a client sees them in.
//!
//! [`Context::tools`], [`Context::resources`] and [`Context::prompts`] read the
//! registry (`list`, `find`), run what is in it (`call`, `read`, `get`), and
//! change it (`add`, `remove`). A change emits the matching `list_changed` to
//! every subscriber.
//!
//! Each namespace is a `Copy` view over a shared borrow of the context, so any
//! number of them can be held at once.
//!
//! # Examples
//! ```no_run
//! # #[cfg(feature = "server")] {
//! use neva::prelude::*;
//!
//! # fn main() {
//! let mut app = App::new();
//! app.map_tool("inventory", |ctx: Context| async move {
//!     let (tools, prompts) = (ctx.tools(), ctx.prompts());
//!     format!(
//!         "{} tools, {} prompts",
//!         tools.list().await.len(),
//!         prompts.list().await.len()
//!     )
//! });
//! # }
//! # }
//! ```

use super::Context;
use crate::error::{Error, ErrorCode};
use crate::shared::IntoArgs;
use crate::types::{
    GetPromptRequestParams, GetPromptResult, Prompt, ReadResourceRequestParams, ReadResourceResult,
    Resource, Tool, ToolResult, ToolUse, Uri, resource::SubscribeRequestParams,
};
use std::fmt::{Debug, Formatter};

impl Context {
    /// The tools this server serves: listing, calling and changing them.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("count_tools", |ctx: Context| async move {
    ///     ctx.tools().list().await.len().to_string()
    /// });
    /// # }
    /// # }
    /// ```
    #[inline]
    pub fn tools(&self) -> Tools<'_> {
        Tools { ctx: self }
    }

    /// The resources this server serves: reading and changing them, and
    /// telling subscribers one was updated.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("readme", |ctx: Context| async move {
    ///     let readme = ctx.resources().read("file:///readme.md").await?;
    ///     Ok::<_, Error>(format!("{:?}", readme.contents))
    /// });
    /// # }
    /// # }
    /// ```
    #[inline]
    pub fn resources(&self) -> Resources<'_> {
        Resources { ctx: self }
    }

    /// The prompts this server serves: rendering and changing them.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("greeting", |ctx: Context| async move {
    ///     let prompt = ctx.prompts().get("greet", ("name", "Ada")).await?;
    ///     Ok::<_, Error>(format!("{:?}", prompt.messages))
    /// });
    /// # }
    /// # }
    /// ```
    #[inline]
    pub fn prompts(&self) -> Prompts<'_> {
        Prompts { ctx: self }
    }
}

/// The tools this server serves, from [`Context::tools`].
///
/// # Examples
/// ```no_run
/// # #[cfg(feature = "server")] {
/// use neva::prelude::*;
///
/// # fn main() {
/// let mut app = App::new();
/// app.map_tool("forecast", |ctx: Context| async move {
///     let tools = ctx.tools();
///     let found = tools.find("get_weather").await.is_some();
///     let weather = tools.call(ToolUse::new("get_weather", ("city", "London"))).await;
///     format!("{found}: {weather:?}")
/// });
/// # }
/// # }
/// ```
#[derive(Clone, Copy)]
pub struct Tools<'a> {
    ctx: &'a Context,
}

impl Debug for Tools<'_> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tools").finish_non_exhaustive()
    }
}

impl Tools<'_> {
    /// Every tool this server serves.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("tool_names", |ctx: Context| async move {
    ///     let names: Vec<String> = ctx.tools().list().await.into_iter().map(|t| t.name).collect();
    ///     names.join(", ")
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn list(&self) -> Vec<Tool> {
        self.ctx.options.tools.values().await
    }

    /// This server's tools as a [`svir::Toolbox`], for a tool
    /// that drives a model over the others.
    ///
    /// The calls run in this process, through the server's pipeline, and hold
    /// the claims of the current request: a tool those claims do not reach is
    /// not offered, and the server's checks see them on every call. The
    /// calling tool is offered too unless a filter leaves it out, and a model
    /// that calls it starts the loop over.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("agent", |ctx: Context| async move {
    ///     let tools = ctx
    ///         .tools()
    ///         .toolbox()
    ///         .filter(|tool| tool.name != "agent")
    ///         .load()
    ///         .await?;
    ///     // `svir::Request::new(model).tools(&tools)`, and the model's calls
    ///     // answered with `tools.call_all(..)`.
    ///     Ok::<_, Error>("done".to_string())
    /// });
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "svir")]
    pub fn toolbox(&self) -> crate::svir::LocalTools {
        crate::svir::LocalTools::of_context(self.ctx)
    }

    /// The tool named `name`, if this server serves one.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("has_weather", |ctx: Context| async move {
    ///     ctx.tools().find("get_weather").await.is_some().to_string()
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn find(&self, name: &str) -> Option<Tool> {
        self.ctx.options.tools.get(name).await
    }

    /// The tools named in `names` that this server serves; a name it does not
    /// serve is left out.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("weather_tools", |ctx: Context| async move {
    ///     ctx.tools().find_many(["get_weather", "get_forecast"]).await.len().to_string()
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn find_many(&self, names: impl IntoIterator<Item = &str>) -> Vec<Tool> {
        let tools = &self.ctx.options.tools;
        futures_util::future::join_all(names.into_iter().map(|name| tools.get(name)))
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Runs the tool a [`ToolUse`] names -- one a model asked for within a
    /// sampling window -- and answers with its result.
    ///
    /// For several at once, [`Self::call_all`].
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("analyze_weather", |ctx: Context, city: String| async move {
    ///     let weather = ctx.tools().call(ToolUse::new("get_weather", ("city", city))).await;
    ///     format!("{weather:?}")
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn call(&self, tool: ToolUse) -> ToolResult {
        let id = tool.id.clone();
        match self.ctx.clone().call_tool(tool.into()).await {
            Ok(res) => ToolResult::new(id, res),
            Err(err) => ToolResult::error(id, err),
        }
    }

    /// Runs the tools several [`ToolUse`]s name, concurrently, and answers with
    /// their results in the same order.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("compare_weather", |ctx: Context| async move {
    ///     let weather = ctx.tools().call_all([
    ///         ToolUse::new("get_weather", ("city", "London")),
    ///         ToolUse::new("get_weather", ("city", "Paris")),
    ///     ]).await;
    ///     format!("{weather:?}")
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn call_all<I>(&self, tools: I) -> Vec<ToolResult>
    where
        I: IntoIterator<Item = ToolUse>,
    {
        futures_util::future::join_all(tools.into_iter().map(|tool| self.call(tool))).await
    }

    /// Adds `tool` to the server and tells subscribers the list changed.
    ///
    /// A tool whose input schema its handler cannot read is refused: one added
    /// after startup has no startup check left to fail.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("enable_echo", |ctx: Context| async move {
    ///     let echo = Tool::new("echo", |message: String| async move { message });
    ///     ctx.tools().add(echo).await
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn add(&self, tool: Tool) -> Result<(), Error> {
        if let Some(conflict) = tool.arg_name_conflict() {
            return Err(Error::new(ErrorCode::InternalError, conflict));
        }

        let options = &self.ctx.options;
        options.tools.insert(tool.name.clone(), tool).await?;

        if options.is_tools_list_changed_supported() {
            self.ctx
                .send_notification(crate::types::tool::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes the tool `name`, telling subscribers the list changed if there
    /// was one, and answers with it.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("disable_echo", |ctx: Context| async move {
    ///     ctx.tools().remove("echo").await.map(|removed| removed.is_some().to_string())
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn remove(&self, name: impl Into<String>) -> Result<Option<Tool>, Error> {
        let options = &self.ctx.options;
        let removed = options.tools.remove(&name.into()).await?;

        if removed.is_some() && options.is_tools_list_changed_supported() {
            self.ctx
                .send_notification(crate::types::tool::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }
}

/// The resources this server serves, from [`Context::resources`].
///
/// # Examples
/// ```no_run
/// # #[cfg(feature = "server")] {
/// use neva::prelude::*;
///
/// # fn main() {
/// let mut app = App::new();
/// app.map_tool("publish_report", |ctx: Context| async move {
///     let resources = ctx.resources();
///     resources.add(Resource::new("res://report", "report")).await?;
///     resources.notify_updated("res://report").await
/// });
/// # }
/// # }
/// ```
#[derive(Clone, Copy)]
pub struct Resources<'a> {
    ctx: &'a Context,
}

impl Debug for Resources<'_> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resources").finish_non_exhaustive()
    }
}

impl Resources<'_> {
    /// Reads the contents of the resource at `uri`, through the handler that
    /// serves it.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("summarize", |ctx: Context, doc: String| async move {
    ///     let doc = ctx.resources().read(format!("file://{doc}")).await?;
    ///     Ok::<_, Error>(format!("{:?}", doc.contents))
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn read(&self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        let params = ReadResourceRequestParams::from(uri.into());
        self.ctx.clone().read_resource(params).await
    }

    /// Adds `resource` to the server and tells subscribers the list changed.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("publish", |ctx: Context| async move {
    ///     ctx.resources().add(Resource::new("res://report", "report")).await
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn add(&self, resource: impl Into<Resource>) -> Result<(), Error> {
        let resource: Resource = resource.into();
        let options = &self.ctx.options;
        options
            .resources
            .insert(resource.name.clone(), resource)
            .await?;

        if options.is_resource_list_changed_supported() {
            self.ctx
                .send_notification(crate::types::resource::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes the resource at `uri`, telling subscribers the list changed if
    /// there was one, and answers with it.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("retract", |ctx: Context| async move {
    ///     ctx.resources().remove("res://report").await.map(|r| r.is_some().to_string())
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn remove(&self, uri: impl Into<Uri>) -> Result<Option<Resource>, Error> {
        let options = &self.ctx.options;
        let removed = options.resources.remove(&uri.into()).await?;

        if removed.is_some() && options.is_resource_list_changed_supported() {
            self.ctx
                .send_notification(crate::types::resource::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }

    /// Tells the clients subscribed to the resource at `uri` that it changed
    /// (`notifications/resources/updated`).
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("touch", |ctx: Context| async move {
    ///     ctx.resources().notify_updated("res://config").await
    /// });
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub async fn notify_updated(&self, uri: impl Into<Uri>) -> Result<(), Error> {
        self.ensure_subscriptions()?;

        let uri = uri.into();
        if self.is_subscribed(&uri) {
            let params = serde_json::to_value(SubscribeRequestParams::from(uri)).ok();
            self.ctx
                .send_notification(crate::types::resource::commands::UPDATED, params)
                .await
        } else {
            Ok(())
        }
    }

    /// Tells the clients subscribed to the resource at `uri` that it changed
    /// (`notifications/resources/updated`).
    ///
    /// The notification is emitted unconditionally and routed by the
    /// subscription filters it reaches: every live stream that named this URI
    /// gets it, every other stream gets nothing. There is deliberately no
    /// "is anybody watching?" pre-check -- [`Self::is_subscribed`] can only
    /// answer for *this* instance, so under a
    /// [`NotificationBus`](crate::app::notification_bus::NotificationBus) it
    /// would skip an update a subscriber on another instance was waiting for.
    /// Publishing one nobody wants is cheap; dropping one somebody wants is a
    /// bug.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("touch", |ctx: Context| async move {
    ///     ctx.resources().notify_updated("res://config").await
    /// });
    /// # }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn notify_updated(&self, uri: impl Into<Uri>) -> Result<(), Error> {
        self.ensure_subscriptions()?;

        let params = serde_json::to_value(SubscribeRequestParams::from(uri.into())).ok();
        self.ctx
            .send_notification(crate::types::resource::commands::UPDATED, params)
            .await
    }

    /// Records a client's subscription to changes of the resource at `uri`.
    ///
    /// Legacy only: under MCP 2026-07-28 a per-resource subscription is a URI
    /// in the `subscriptions/listen` filter, established by the client and
    /// scoped to that stream, so there is nothing for the server to add.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(all(feature = "server", feature = "legacy-spec"))] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("watch", |ctx: Context| async move {
    ///     ctx.resources().subscribe("res://config");
    /// });
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub fn subscribe(&self, uri: impl Into<Uri>) {
        self.ctx.options.resource_subscriptions.insert(uri.into());
    }

    /// Drops a client's subscription to changes of the resource at `uri`.
    ///
    /// Legacy only; see [`Self::subscribe`].
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(all(feature = "server", feature = "legacy-spec"))] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("unwatch", |ctx: Context| async move {
    ///     ctx.resources().unsubscribe(&"res://config".into());
    /// });
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub fn unsubscribe(&self, uri: &Uri) {
        self.ctx.options.resource_subscriptions.remove(uri);
    }

    /// Whether a client is subscribed to changes of the resource at `uri`.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(all(feature = "server", feature = "legacy-spec"))] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("watched", |ctx: Context| async move {
    ///     ctx.resources().is_subscribed(&"res://config".into()).to_string()
    /// });
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub fn is_subscribed(&self, uri: &Uri) -> bool {
        self.ctx.options.resource_subscriptions.contains(uri)
    }

    /// Whether any live `subscriptions/listen` stream watches the resource at
    /// `uri`.
    ///
    /// **Node-local.** A subscription lives in the process holding its socket
    /// open, so this answers for *this instance only*. In a horizontally
    /// scaled deployment a `false` here means "nobody on this instance", not
    /// "nobody anywhere" -- so do not use it to decide whether to emit a
    /// notification. [`Self::notify_updated`] deliberately does not:
    /// notifications are published unconditionally and routed by the
    /// subscription filters they reach, wherever those live. Use this only
    /// where a node-local answer is what you actually want, such as skipping
    /// expensive local work no one on this instance is streaming.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(all(feature = "server", not(feature = "legacy-spec")))] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("watched", |ctx: Context| async move {
    ///     ctx.resources().is_subscribed(&"res://config".into()).to_string()
    /// });
    /// # }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn is_subscribed(&self, uri: &Uri) -> bool {
        self.ctx.options.subscriptions().is_resource_subscribed(uri)
    }

    #[inline]
    fn ensure_subscriptions(&self) -> Result<(), Error> {
        if self.ctx.options.is_resource_subscription_supported() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support sending resource/updated notifications",
            ))
        }
    }
}

/// The prompts this server serves, from [`Context::prompts`].
///
/// # Examples
/// ```no_run
/// # #[cfg(feature = "server")] {
/// use neva::prelude::*;
///
/// # fn main() {
/// let mut app = App::new();
/// app.map_tool("greeting", |ctx: Context, name: String| async move {
///     let prompt = ctx.prompts().get("greet", ("name", name)).await?;
///     Ok::<_, Error>(format!("{:?}", prompt.messages))
/// });
/// # }
/// # }
/// ```
#[derive(Clone, Copy)]
pub struct Prompts<'a> {
    ctx: &'a Context,
}

impl Debug for Prompts<'_> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompts").finish_non_exhaustive()
    }
}

impl Prompts<'_> {
    /// Every prompt this server serves.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("prompt_count", |ctx: Context| async move {
    ///     ctx.prompts().list().await.len().to_string()
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn list(&self) -> Vec<Prompt> {
        self.ctx.options.prompts.values().await
    }

    /// Renders the prompt `name` with `args`, through the handler that serves
    /// it.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("greeting", |ctx: Context, name: String| async move {
    ///     let prompt = ctx.prompts().get("greet", ("name", name)).await?;
    ///     Ok::<_, Error>(format!("{:?}", prompt.messages))
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn get<N, Args>(&self, name: N, args: Args) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        let params = GetPromptRequestParams {
            name: name.into(),
            args: args.into_args(),
            meta: None,
        };

        self.ctx.clone().get_prompt(params).await
    }

    /// Adds `prompt` to the server and tells subscribers the list changed.
    ///
    /// A prompt whose arguments its handler cannot read is refused: one added
    /// after startup has no startup check left to fail.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("enable_greet", |ctx: Context| async move {
    ///     let greet = Prompt::new("greet", |name: String| async move {
    ///         (format!("Hello, {name}"), Role::User)
    ///     });
    ///     ctx.prompts().add(greet).await
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn add(&self, prompt: Prompt) -> Result<(), Error> {
        // A prompt registered up front gets this same check at startup. One
        // added while the server runs has no startup left to fail, so it is
        // refused here rather than published in a shape no peer could
        // successfully use.
        if let Some(conflict) = prompt.arg_name_conflict() {
            return Err(Error::new(ErrorCode::InternalError, conflict));
        }

        let options = &self.ctx.options;
        options.prompts.insert(prompt.name.clone(), prompt).await?;

        if options.is_prompts_list_changed_supported() {
            self.ctx
                .send_notification(crate::types::prompt::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes the prompt `name`, telling subscribers the list changed if
    /// there was one, and answers with it.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(feature = "server")] {
    /// use neva::prelude::*;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    /// app.map_tool("disable_greet", |ctx: Context| async move {
    ///     ctx.prompts().remove("greet").await.map(|p| p.is_some().to_string())
    /// });
    /// # }
    /// # }
    /// ```
    pub async fn remove(&self, name: impl Into<String>) -> Result<Option<Prompt>, Error> {
        let options = &self.ctx.options;
        let removed = options.prompts.remove(&name.into()).await?;

        if removed.is_some() && options.is_prompts_list_changed_supported() {
            self.ctx
                .send_notification(crate::types::prompt::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }
}
