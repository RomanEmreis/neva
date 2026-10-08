//! What the dispatch layer enters a registered handler through, plus the
//! deprecated flat spellings of [`super::api`].
//!
//! A handler reaches the tools, prompts and resources this server serves
//! through [`Context::tools`], [`Context::resources`] and [`Context::prompts`].
//! The `pub(crate)` calls at the end are the other audience: the dispatch layer
//! entering a registered handler.

use super::*;

impl Context {
    /// Finds a tool by `name`
    #[deprecated(since = "0.7.0", note = "use `ctx.tools().find(name)`")]
    #[inline]
    pub async fn find_tool(&self, name: &str) -> Option<Tool> {
        self.tools().find(name).await
    }

    /// Returns a list of tools by name.
    #[deprecated(since = "0.7.0", note = "use `ctx.tools().find_many(names)`")]
    #[inline]
    pub async fn find_tools(&self, names: impl IntoIterator<Item = &str>) -> Vec<Tool> {
        self.tools().find_many(names).await
    }

    /// Initiates a tool call once a [`ToolUse`] request received from assistant
    /// withing a sampling window.
    #[deprecated(since = "0.7.0", note = "use `ctx.tools().call(tool)`")]
    #[inline]
    pub async fn use_tool(&self, tool: ToolUse) -> ToolResult {
        self.tools().call(tool).await
    }

    /// Initiates a parallel tool calls for multiple [`ToolUse`] requests.
    #[deprecated(since = "0.7.0", note = "use `ctx.tools().call_all(tools)`")]
    #[inline]
    pub async fn use_tools<I>(&self, tools: I) -> Vec<ToolResult>
    where
        I: IntoIterator<Item = ToolUse>,
    {
        self.tools().call_all(tools).await
    }

    /// Adds a new tool and notifies clients
    #[deprecated(since = "0.7.0", note = "use `ctx.tools().add(tool)`")]
    #[inline]
    pub async fn add_tool(&self, tool: Tool) -> Result<(), Error> {
        self.tools().add(tool).await
    }

    /// Removes a tool and notifies clients
    #[deprecated(since = "0.7.0", note = "use `ctx.tools().remove(name)`")]
    #[inline]
    pub async fn remove_tool(&self, name: impl Into<String>) -> Result<Option<Tool>, Error> {
        self.tools().remove(name).await
    }

    /// Gets the prompt by name
    #[deprecated(since = "0.7.0", note = "use `ctx.prompts().get(name, args)`")]
    #[inline]
    pub async fn prompt<N, Args>(&self, name: N, args: Args) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        self.prompts().get(name, args).await
    }

    /// Adds a new prompt and notifies clients
    #[deprecated(since = "0.7.0", note = "use `ctx.prompts().add(prompt)`")]
    #[inline]
    pub async fn add_prompt(&self, prompt: Prompt) -> Result<(), Error> {
        self.prompts().add(prompt).await
    }

    /// Removes a prompt and notifies clients
    #[deprecated(since = "0.7.0", note = "use `ctx.prompts().remove(name)`")]
    #[inline]
    pub async fn remove_prompt(&self, name: impl Into<String>) -> Result<Option<Prompt>, Error> {
        self.prompts().remove(name).await
    }

    /// Reads a resource content
    #[deprecated(since = "0.7.0", note = "use `ctx.resources().read(uri)`")]
    #[inline]
    pub async fn resource(&self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        self.resources().read(uri).await
    }

    /// Adds a new resource and notifies clients
    #[deprecated(since = "0.7.0", note = "use `ctx.resources().add(resource)`")]
    #[inline]
    pub async fn add_resource(&self, res: impl Into<Resource>) -> Result<(), Error> {
        self.resources().add(res).await
    }

    /// Removes a resource and notifies clients
    #[deprecated(since = "0.7.0", note = "use `ctx.resources().remove(uri)`")]
    #[inline]
    pub async fn remove_resource(&self, uri: impl Into<Uri>) -> Result<Option<Resource>, Error> {
        self.resources().remove(uri).await
    }

    /// Sends a notification that the resource with the `uri` has been updated
    #[deprecated(since = "0.7.0", note = "use `ctx.resources().notify_updated(uri)`")]
    #[inline]
    pub async fn resource_updated(&self, uri: impl Into<Uri>) -> Result<(), Error> {
        self.resources().notify_updated(uri).await
    }

    /// Adds a subscription to the resource with the [`Uri`]
    #[cfg(feature = "legacy-spec")]
    #[deprecated(since = "0.7.0", note = "use `ctx.resources().subscribe(uri)`")]
    #[inline]
    pub fn subscribe_to_resource(&self, uri: impl Into<Uri>) {
        self.resources().subscribe(uri)
    }

    /// Removes a subscription to the resource with the [`Uri`]
    #[cfg(feature = "legacy-spec")]
    #[deprecated(since = "0.7.0", note = "use `ctx.resources().unsubscribe(uri)`")]
    #[inline]
    pub fn unsubscribe_from_resource(&self, uri: &Uri) {
        self.resources().unsubscribe(uri)
    }

    /// Returns `true` if there is a subscription to changes of the resource with the [`Uri`]
    #[deprecated(since = "0.7.0", note = "use `ctx.resources().is_subscribed(uri)`")]
    #[inline]
    pub fn is_subscribed(&self, uri: &Uri) -> bool {
        self.resources().is_subscribed(uri)
    }

    #[inline]
    pub(crate) async fn read_resource(
        self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, Error> {
        let opt = self.options.clone();
        match opt.read_resource(&params.uri) {
            Some((handler, args)) => {
                // On the route, whatever registered it: a template's requirement
                // is copied there when the server starts, so a read never has to
                // look the template up.
                #[cfg(feature = "http-server")]
                self.validate_claims(&handler.required)?;
                #[cfg_attr(not(feature = "apps"), allow(unused_mut))]
                let mut result = handler
                    .call(params.with_args(args).with_context(self).into())
                    .await?;

                #[cfg(feature = "apps")]
                opt.apply_app_defaults(&handler.template, &mut result).await;

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
                self.validate_claims(&prompt.required)?;
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
                self.validate_claims(&tool.required)?;
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
            #[cfg(not(feature = "legacy-spec"))]
            client_extensions: None,
            #[cfg(feature = "di")]
            scope: None,
            #[cfg(feature = "svir")]
            runtime: None,
            #[cfg(feature = "svir")]
            tool: None,
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

        let err = ctx().tools().add(tool).await.expect_err("must be refused");

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

        let err = ctx()
            .prompts()
            .add(prompt)
            .await
            .expect_err("must be refused");

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

        ctx().tools().add(tool).await.expect("must be accepted");
    }
}

/// Who may read a resource. A route no template backs -- a `ui://` resource --
/// states its own requirement; every other route inherits its template's.
#[cfg(all(test, feature = "http-server"))]
mod read_resource_claims_tests {
    use super::*;
    use crate::auth::DefaultClaims;
    use crate::types::resource::template::ResourceFunc;
    use crate::types::{ResourceContents, ResourceTemplate};

    fn ctx(options: RuntimeMcpOptions, claims: Option<DefaultClaims>) -> Context {
        Context {
            session_id: None,
            headers: HeaderMap::new(),
            claims: claims.map(|claims| Arc::new(claims) as Arc<dyn crate::auth::Claims>),
            pending: RequestQueue::new(Duration::from_secs(5)),
            sender: TransportProtoSender::None,
            options,
            timeout: Duration::from_secs(5),
            #[cfg(not(feature = "legacy-spec"))]
            exec: ExecMode::None,
            #[cfg(not(feature = "legacy-spec"))]
            client_capabilities: Default::default(),
            #[cfg(not(feature = "legacy-spec"))]
            client_extensions: None,
            #[cfg(feature = "di")]
            scope: None,
            #[cfg(feature = "svir")]
            runtime: None,
            #[cfg(feature = "svir")]
            tool: None,
        }
    }

    fn role(role: &str) -> Option<DefaultClaims> {
        Some(DefaultClaims {
            role: Some(role.into()),
            ..Default::default()
        })
    }

    async fn read(
        options: &RuntimeMcpOptions,
        uri: &str,
        claims: Option<DefaultClaims>,
    ) -> Result<ReadResourceResult, Error> {
        ctx(options.clone(), claims)
            .read_resource(Uri::from(uri).into())
            .await
    }

    #[tokio::test]
    async fn a_template_route_is_judged_by_its_template() {
        let mut options = McpOptions::default();
        options
            .add_resource_template(
                ResourceTemplate::new("res://secret/{id}", "secret"),
                ResourceFunc::new(|uri: Uri| async move {
                    ResourceContents::new(uri).with_text("secret")
                }),
            )
            .with_roles(["admin"]);
        let options = options.into_runtime();

        assert!(read(&options, "res://secret/1", None).await.is_err());
        assert!(
            read(&options, "res://secret/1", role("guest"))
                .await
                .is_err()
        );
        assert!(
            read(&options, "res://secret/1", role("admin"))
                .await
                .is_ok()
        );
    }

    #[cfg(all(feature = "apps", not(feature = "legacy-spec")))]
    mod ui {
        use super::*;
        use crate::app::extension::UiResource;

        fn permission(permission: &str) -> Option<DefaultClaims> {
            Some(DefaultClaims {
                permissions: Some(vec![permission.into()]),
                ..Default::default()
            })
        }

        #[tokio::test]
        async fn a_restricted_ui_resource_refuses_a_caller_without_the_role() {
            let mut options = McpOptions::default();
            options
                .add_ui_resource(UiResource::new("ui://admin/app.html", "admin", "<html>"))
                .with_roles(["admin"]);
            let options = options.into_runtime();

            assert!(read(&options, "ui://admin/app.html", None).await.is_err());
            assert!(
                read(&options, "ui://admin/app.html", role("guest"))
                    .await
                    .is_err()
            );

            let result = read(&options, "ui://admin/app.html", role("admin"))
                .await
                .expect("the role is held");
            assert_eq!(result.contents[0].text(), Some("<html>"));
        }

        #[tokio::test]
        async fn required_permissions_are_checked_too() {
            let mut options = McpOptions::default();
            options
                .add_ui_resource(UiResource::new(
                    "ui://reports/app.html",
                    "reports",
                    "<html>",
                ))
                .with_permissions(["reports:read"]);
            let options = options.into_runtime();

            assert!(read(&options, "ui://reports/app.html", None).await.is_err());
            assert!(
                read(
                    &options,
                    "ui://reports/app.html",
                    permission("reports:write")
                )
                .await
                .is_err()
            );
            assert!(
                read(
                    &options,
                    "ui://reports/app.html",
                    permission("reports:read")
                )
                .await
                .is_ok()
            );
        }

        #[tokio::test]
        async fn an_unrestricted_ui_resource_still_answers_anyone() {
            let mut options = McpOptions::default();
            options.add_ui_resource(UiResource::new("ui://clock/app.html", "clock", "<html>"));
            let options = options.into_runtime();

            assert!(read(&options, "ui://clock/app.html", None).await.is_ok());
            assert!(
                read(&options, "ui://clock/app.html", role("guest"))
                    .await
                    .is_ok()
            );
        }

        /// The requirement lives on the route, so it must not leak onto the
        /// same-named template -- nor the template's onto the route.
        #[tokio::test]
        async fn a_ui_route_and_a_same_named_template_keep_their_own_requirements() {
            let mut options = McpOptions::default();
            options
                .add_resource_template(
                    ResourceTemplate::new("res://clock/{id}", "clock"),
                    ResourceFunc::new(|uri: Uri| async move {
                        ResourceContents::new(uri).with_text("secret")
                    }),
                )
                .with_roles(["admin"]);
            options
                .add_ui_resource(UiResource::new("ui://clock/app.html", "clock", "<html>"))
                .with_roles(["viewer"]);
            let options = options.into_runtime();

            assert!(
                read(&options, "ui://clock/app.html", role("viewer"))
                    .await
                    .is_ok()
            );
            assert!(
                read(&options, "ui://clock/app.html", role("admin"))
                    .await
                    .is_err()
            );
            assert!(
                read(&options, "res://clock/1", role("viewer"))
                    .await
                    .is_err()
            );
            assert!(read(&options, "res://clock/1", role("admin")).await.is_ok());
        }
    }
}
