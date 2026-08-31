//! What a handler reaches for: the tools, prompts and resources this server
//! serves, and the calls that run them.
//!
//! Two audiences share one surface. A handler *reads* the registry
//! (`tools`, `find_tool`) and *mutates* it (`add_tool`, `remove_resource`) --
//! a mutation emits the matching `list_changed` to every subscriber, which is
//! why these take `&mut self` even where nothing local changes. The
//! `pub(crate)` calls at the end are the other audience: the dispatch layer
//! entering a registered handler.

use super::*;

impl Context {
    /// Returns a list of all available tools
    pub async fn tools(&self) -> Vec<Tool> {
        self.options.tools.values().await
    }

    /// Finds a tool by `name`
    pub async fn find_tool(&self, name: &str) -> Option<Tool> {
        self.options.tools.get(name).await
    }

    /// Returns a list of tools by name.
    /// If some tools requested in `names` are missing, they won't be in the result list.
    pub async fn find_tools(&self, names: impl IntoIterator<Item = &str>) -> Vec<Tool> {
        futures_util::future::join_all(names.into_iter().map(|name| self.options.tools.get(name)))
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Initiates a tool call once a [`ToolUse`] request received from assistant
    /// withing a sampling window.
    ///
    /// For multiple [`ToolUse`] requests, use the [`Context::use_tools`] method.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "legacy-spec"))] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context, city: String) -> Result<(), Error> {
    ///     let args = ("city", city);
    ///     let weather = ctx.use_tool(ToolUse::new("get_weather", args)).await;
    ///
    ///     // do something with the weather result
    ///
    /// # Ok(())
    /// }
    ///
    /// #[tool]
    /// async fn get_weather(city: String) -> String {
    ///     // ...
    ///
    ///     format!("Sunny in {city}")
    /// }
    /// # }
    /// ```
    pub async fn use_tool(&self, tool: ToolUse) -> ToolResult {
        let id = tool.id.clone();
        let res = self.clone().call_tool(tool.into()).await;

        match res {
            Ok(res) => ToolResult::new(id, res),
            Err(err) => ToolResult::error(id, err),
        }
    }

    /// Initiates a parallel tool calls for multiple [`ToolUse`] requests.
    ///
    /// For a single [`ToolUse`] use the [`Context::use_tool`] method.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context) -> Result<(), Error> {
    ///     let weather = ctx.use_tools([
    ///         ToolUse::new("get_weather", ("city", "London")),
    ///         ToolUse::new("get_weather", ("city", "Paris"))
    ///     ]).await;
    ///     
    ///     // do something with the weather result
    ///
    /// # Ok(())
    /// }
    /// # }
    /// ```
    pub async fn use_tools<I>(&self, tools: I) -> Vec<ToolResult>
    where
        I: IntoIterator<Item = ToolUse>,
    {
        futures_util::future::join_all(tools.into_iter().map(|t| self.use_tool(t))).await
    }

    /// Gets the prompt by name
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "legacy-spec"))] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context, city: String) -> Result<(), Error> {
    ///     let prompt = ctx.prompt("get_weather", ("city", city)).await?;
    ///
    ///     // do something with the prompt
    ///
    /// # Ok(())
    /// }
    ///
    /// #[prompt]
    /// async fn get_weather(city: String) -> PromptMessage {
    ///     PromptMessage::user()
    ///         .with(format!("What's the weather in {city}"))
    /// }
    /// # }
    /// ```
    pub async fn prompt<N, Args>(&self, name: N, args: Args) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        let params = GetPromptRequestParams {
            name: name.into(),
            args: args.into_args(),
            meta: None,
        };

        self.clone().get_prompt(params).await
    }

    /// Reads a resource content
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "legacy-spec"))] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn summarize_document(ctx: Context, doc_uri: Uri) -> Result<(), Error> {
    ///     let doc = ctx.resource(doc_uri).await?;
    ///
    ///     // do something with the doc
    ///
    /// # Ok(())
    /// }
    ///
    /// #[resource(uri = "file://{name}")]
    /// async fn get_doc(name: String) -> TextResourceContents {
    ///     // read the doc
    ///
    /// # TextResourceContents::new("", "")
    /// }
    /// # }
    /// ```
    pub async fn resource(&self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        let uri = uri.into();
        let params = ReadResourceRequestParams::from(uri);

        self.clone().read_resource(params).await
    }

    /// Adds a new resource and notifies clients
    pub async fn add_resource(&mut self, res: impl Into<Resource>) -> Result<(), Error> {
        let res: Resource = res.into();
        self.options.resources.insert(res.name.clone(), res).await?;

        if self.options.is_resource_list_changed_supported() {
            self.send_notification(crate::types::resource::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes a resource and notifies clients
    pub async fn remove_resource(
        &mut self,
        uri: impl Into<Uri>,
    ) -> Result<Option<Resource>, Error> {
        let removed = self.options.resources.remove(&uri.into()).await?;

        if removed.is_some() && self.options.is_resource_list_changed_supported() {
            self.send_notification(crate::types::resource::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }

    /// Sends a notification that the resource with the `uri` has been updated
    #[cfg(feature = "legacy-spec")]
    pub async fn resource_updated(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        if !self.options.is_resource_subscription_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support sending resource/updated notifications",
            ));
        }

        let uri = uri.into();
        if self.is_subscribed(&uri) {
            let params = serde_json::to_value(SubscribeRequestParams::from(uri)).ok();
            self.send_notification(crate::types::resource::commands::UPDATED, params)
                .await
        } else {
            Ok(())
        }
    }

    /// Sends a notification that the resource with the `uri` has been updated
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
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn resource_updated(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        if !self.options.is_resource_subscription_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support sending resource/updated notifications",
            ));
        }

        let params = serde_json::to_value(SubscribeRequestParams::from(uri.into())).ok();
        self.send_notification(crate::types::resource::commands::UPDATED, params)
            .await
    }

    /// Adds a subscription to the resource with the [`Uri`]
    ///
    /// Legacy only: under MCP 2026-07-28 a per-resource subscription is a URI
    /// in the `subscriptions/listen` filter, established by the client and
    /// scoped to that stream, so there is nothing for the server to add.
    #[cfg(feature = "legacy-spec")]
    pub fn subscribe_to_resource(&mut self, uri: impl Into<Uri>) {
        self.options.resource_subscriptions.insert(uri.into());
    }

    /// Removes a subscription to the resource with the [`Uri`]
    ///
    /// Legacy only; see [`Self::subscribe_to_resource`].
    #[cfg(feature = "legacy-spec")]
    pub fn unsubscribe_from_resource(&mut self, uri: &Uri) {
        self.options.resource_subscriptions.remove(uri);
    }

    /// Returns `true` if there is a subscription to changes of the resource with the [`Uri`]
    #[cfg(feature = "legacy-spec")]
    pub fn is_subscribed(&self, uri: &Uri) -> bool {
        self.options.resource_subscriptions.contains(uri)
    }

    /// Returns `true` if any live `subscriptions/listen` stream watches the
    /// resource with the [`Uri`].
    ///
    /// **Node-local.** A subscription lives in the process holding its socket
    /// open, so this answers for *this instance only*. In a horizontally
    /// scaled deployment a `false` here means "nobody on this instance", not
    /// "nobody anywhere" -- so do not use it to decide whether to emit a
    /// notification. [`Self::resource_updated`] deliberately does not:
    /// notifications are published unconditionally and routed by the
    /// subscription filters they reach, wherever those live. Use this only
    /// where a node-local answer is what you actually want, such as skipping
    /// expensive local work no one on this instance is streaming.
    ///
    /// # Examples
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn touch(ctx: Context) -> Result<(), Error> {
    ///     if ctx.is_subscribed(&"res://config".into()) {
    ///         // somebody is listening for this resource
    ///     }
    /// # Ok(())
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn is_subscribed(&self, uri: &Uri) -> bool {
        self.options.subscriptions().is_resource_subscribed(uri)
    }

    /// Adds a new prompt and notifies clients
    pub async fn add_prompt(&mut self, prompt: Prompt) -> Result<(), Error> {
        // A prompt registered up front gets this same check at startup. One
        // added while the server runs has no startup left to fail, so it is
        // refused here rather than published in a shape no peer could
        // successfully use.
        if let Some(conflict) = prompt.arg_name_conflict() {
            return Err(Error::new(ErrorCode::InternalError, conflict));
        }

        self.options
            .prompts
            .insert(prompt.name.clone(), prompt)
            .await?;

        if self.options.is_prompts_list_changed_supported() {
            self.send_notification(crate::types::prompt::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes a prompt and notifies clients
    pub async fn remove_prompt(
        &mut self,
        name: impl Into<String>,
    ) -> Result<Option<Prompt>, Error> {
        let removed = self.options.prompts.remove(&name.into()).await?;

        if removed.is_some() && self.options.is_prompts_list_changed_supported() {
            self.send_notification(crate::types::prompt::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }

    /// Adds a new prompt and notifies clients
    pub async fn add_tool(&mut self, tool: Tool) -> Result<(), Error> {
        // See `add_prompt`: a tool added after startup has no startup check
        // left to fail, so a schema its handler cannot read is refused here.
        if let Some(conflict) = tool.arg_name_conflict() {
            return Err(Error::new(ErrorCode::InternalError, conflict));
        }

        self.options.tools.insert(tool.name.clone(), tool).await?;

        if self.options.is_tools_list_changed_supported() {
            self.send_notification(crate::types::tool::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes a tool and notifies clients
    pub async fn remove_tool(&mut self, name: impl Into<String>) -> Result<Option<Tool>, Error> {
        let removed = self.options.tools.remove(&name.into()).await?;

        if removed.is_some() && self.options.is_tools_list_changed_supported() {
            self.send_notification(crate::types::tool::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }

    #[inline]
    pub(crate) async fn read_resource(
        self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, Error> {
        let opt = self.options.clone();
        match opt.read_resource(&params.uri) {
            Some((handler, args)) => {
                #[cfg(feature = "http-server")]
                {
                    let template = opt.resources_templates.get(&handler.template).await;
                    self.validate_claims(
                        template.as_ref().and_then(|t| t.roles.as_deref()),
                        template.as_ref().and_then(|t| t.permissions.as_deref()),
                    )
                }?;
                #[cfg_attr(not(feature = "apps"), allow(unused_mut))]
                let mut result = handler
                    .call(params.with_args(args).with_context(self).into())
                    .await?;

                #[cfg(feature = "apps")]
                opt.apply_template_ui(&handler.template, &mut result).await;

                Ok(result)
            }
            // The spec's SHOULD: name the URI that was not found in
            // `error.data.uri`. A caller that fanned several reads onto one
            // connection otherwise cannot tell which of them this refers to
            // without matching on the request id, and an intermediary logging
            // the error has nothing to log.
            _ => Err(Error::from(ErrorCode::RESOURCE_NOT_FOUND)
                .with_data(serde_json::json!({ "uri": params.uri.to_string() }))),
        }
    }

    #[inline]
    pub(crate) async fn get_prompt(
        self,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, Error> {
        match self.options.get_prompt(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Prompt not found")),
            Some(prompt) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(prompt.roles.as_deref(), prompt.permissions.as_deref())?;
                prompt.call(params.with_context(self)).await
            }
        }
    }

    #[inline]
    pub(crate) async fn call_tool(
        self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResponse, Error> {
        match self.options.get_tool(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found")),
            Some(tool) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(tool.roles.as_deref(), tool.permissions.as_deref())?;
                tool.call(params.with_context(self)).await
            }
        }
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod missing_resource_error_tests {
    use crate::error::ErrorCode;

    #[test]
    fn missing_resource_uses_spec_version_code() {
        // The constant the emitters use must match the spec.
        #[cfg(not(feature = "legacy-spec"))]
        assert_eq!(i32::from(ErrorCode::RESOURCE_NOT_FOUND), -32602);
        #[cfg(feature = "legacy-spec")]
        assert_eq!(i32::from(ErrorCode::RESOURCE_NOT_FOUND), -32002);
    }
}

/// A tool or prompt registered up front is checked for argument conflicts at
/// startup. One added while the server is running has no startup left to fail,
/// so the same check has to run at insertion -- otherwise it is published in a
/// shape no peer could successfully call.
#[cfg(all(test, feature = "http-server"))]
mod runtime_registration_tests {
    use super::*;
    use crate::types::{Prompt, Role, Tool};

    fn ctx() -> Context {
        Context {
            session_id: None,
            headers: HeaderMap::new(),
            claims: None,
            pending: RequestQueue::new(Duration::from_secs(5)),
            sender: TransportProtoSender::None,
            // Runtime state: the collections must accept insertions, which is
            // the whole point of the paths under test.
            options: McpOptions::default().into_runtime(),
            timeout: Duration::from_secs(5),
            #[cfg(not(feature = "legacy-spec"))]
            exec: ExecMode::None,
            #[cfg(not(feature = "legacy-spec"))]
            client_capabilities: Default::default(),
            #[cfg(feature = "di")]
            scope: None,
        }
    }

    fn name_schema() -> crate::types::ToolInputSchema {
        const JSON: &str = r#"{"type":"object","properties":{"name":{"type":"string"}}}"#;
        #[cfg(feature = "legacy-spec")]
        {
            crate::types::tool::ToolSchema::from_json_str(JSON)
        }
        #[cfg(not(feature = "legacy-spec"))]
        {
            crate::types::schema_2020::InputSchema::from_json_str(JSON).unwrap_or_default()
        }
    }

    #[tokio::test]
    async fn it_refuses_a_tool_whose_schema_its_handler_cannot_read() {
        let mut tool = Tool::new("greet", |name: String| async move { name });
        tool.with_input_schema(|_| name_schema());

        let err = ctx().add_tool(tool).await.expect_err("must be refused");

        assert!(
            err.to_string().contains("publishes an inputSchema without"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn it_refuses_a_prompt_that_publishes_too_few_args() {
        let mut prompt = Prompt::new("analyze", |topic: String, tone: String| async move {
            (format!("{topic}{tone}"), Role::User)
        });
        prompt.with_args(["topic"]);

        let err = ctx().add_prompt(prompt).await.expect_err("must be refused");

        assert!(
            err.to_string().contains("publishes 1 argument(s)"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn it_accepts_a_consistent_tool() {
        let mut tool = Tool::new("greet", |name: String| async move { name });
        tool.with_input_schema(|_| name_schema())
            .with_arg_names(["name"]);

        ctx().add_tool(tool).await.expect("must be accepted");
    }
}
