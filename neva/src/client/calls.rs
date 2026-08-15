//! The MCP methods a client calls on a server.
//!
//! Listing is paginated behind the scenes: the `list_*` methods walk the cursor
//! and hand back whole collections. `call_tool` carries the extra machinery --
//! `Mcp-Param-*` header mirroring is derived from the tool annotations this
//! client last listed, so a call can be rejected for headers built from a stale
//! listing and has to recover by re-listing and retrying once.

use super::*;

impl Client {
    /// Sends a command to the MCP server
    ///
    /// # Example
    /// ```no_run
    /// use neva::prelude::*;
    ///
    /// #[derive(serde::Serialize)]
    /// struct MyCommandParams {
    ///     param: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let params = MyCommandParams { param: "Hello MCP!".to_string() };
    ///     let tools = client.command("my-command", Some(params)).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[inline]
    pub async fn command<T: Serialize>(
        &mut self,
        command: impl Into<String>,
        params: Option<T>,
    ) -> Result<Response, Error> {
        let id = self.generate_id()?;
        let request = Request::new(Some(id), command, params);
        self.send_request(request).await
    }

    /// Requests a list of tools that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of tools if the MCP server provides pagination
    ///     let tools = client.list_tools(None).await?;
    ///     
    ///     // Fetch the next page of tools is any   
    ///     let tools = client.list_tools(tools.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list_tools(&mut self, cursor: Option<Cursor>) -> Result<ListToolsResult, Error> {
        self.list_tools_inner(
            cursor,
            #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
            None,
        )
        .await
    }

    /// [`Self::list_tools`], plus the name of the tool this listing was fetched
    /// to retry -- whose registration becomes usable once regardless of its
    /// TTL. Only that one: every other tool on the page is an ordinary
    /// registration, and handing it the same exception would let a later call
    /// mirror from a listing it never refreshed.
    ///
    /// See [`Self::retry_after_header_mismatch`].
    pub(super) async fn list_tools_inner(
        &mut self,
        cursor: Option<Cursor>,
        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))] grace: Option<&str>,
    ) -> Result<ListToolsResult, Error> {
        // A cursor-less call starts the listing over, so it replaces what the
        // previous traversal registered rather than merging into it.
        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
        let fresh = cursor.is_none();
        let params = ListToolsRequestParams { cursor };

        #[allow(unused_mut)]
        let mut result: ListToolsResult = self
            .command(crate::types::tool::commands::LIST, Some(params))
            .await?
            .into_result()?;

        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
        self.register_param_headers(&mut result, fresh, grace);

        Ok(result)
    }

    /// Runs a batched `tools/list` response through the same registry update a
    /// direct [`Self::list_tools`] performs, rewriting the response in place so
    /// the caller never sees a tool the client refuses to call.
    ///
    /// A batched listing is always a fresh traversal: [`BatchBuilder`] enqueues
    /// it without a cursor. A response that does not parse as a listing is left
    /// alone -- it is the caller's to interpret, and it registers nothing.
    #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
    pub(super) fn register_batched_tools(&mut self, resp: &mut Response) {
        let Response::Ok(ok) = resp else { return };
        let Ok(mut result) = serde_json::from_value::<ListToolsResult>(ok.result.clone()) else {
            return;
        };

        self.register_param_headers(&mut result, true, None);

        if let Ok(value) = serde_json::to_value(&result) {
            ok.result = value;
        }
    }

    /// Records each tool's `x-mcp-header` annotations and drops any tool whose
    /// annotations are invalid.
    ///
    /// The spec makes rejection per-tool on purpose: one malformed definition
    /// must not take the whole listing down, and must not be callable either --
    /// so the offending tool is removed from the result the caller sees.
    ///
    /// A refreshed listing replaces what the previous one registered, including
    /// replacing it with nothing: a server that drops an annotation -- or drops
    /// the whole tool -- must stop the client from mirroring that argument into
    /// a header, which a leftover registration would keep doing even though the
    /// current listing no longer designates it.
    ///
    /// `fresh` marks the first page of a traversal, which clears the registry;
    /// later pages accumulate onto it, since a tool absent from page two has
    /// not been withdrawn, only listed elsewhere.
    ///
    /// The name of a rejected tool is remembered as well, so that hiding it
    /// from the listing is not all that hiding it does -- see
    /// [`Self::blocked_tool_error`].
    #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
    pub(super) fn register_param_headers(
        &mut self,
        result: &mut ListToolsResult,
        fresh: bool,
        grace: Option<&str>,
    ) {
        use crate::shared::param_headers;

        if fresh {
            self.options.param_headers.clear();
            self.options.rejected_tools.clear();
        }

        // How long this listing may be mirrored from. The spec makes `ttlMs`
        // mandatory and reads an absent one as `0` -- immediately stale -- so
        // every registration is stamped with the listing that produced it.
        let ttl_ms = result.ttl_ms;

        result.tools.retain(|tool| {
            self.options.param_headers.remove(&*tool.name);
            self.options.rejected_tools.remove(&*tool.name);
            let schema = match serde_json::to_value(&tool.input_schema) {
                Ok(schema) => schema,
                Err(_) => return true,
            };

            match param_headers::collect(&schema) {
                Ok(headers) => {
                    if !headers.is_empty() {
                        self.options.param_headers.insert(
                            tool.name.to_string(),
                            param_headers::Registration::new(
                                headers,
                                ttl_ms,
                                grace == Some(&*tool.name),
                            ),
                        );
                    }
                    true
                }
                Err(_err) => {
                    #[cfg(feature = "tracing")]
                    tracing::warn!(logger = "neva", "Dropping tool `{}`: {_err}", tool.name);
                    self.options.rejected_tools.insert(tool.name.to_string());
                    false
                }
            }
        });
    }

    /// Refuses a `tools/call` naming a tool the current listing withdrew for a
    /// malformed `x-mcp-header` declaration.
    ///
    /// Dropping such a tool from `tools/list` is what the spec asks for, but on
    /// its own it only hides the name: a caller holding one from somewhere else
    /// -- hard-coded, cached, read off a log -- still reaches `call_tool`, and
    /// since the declaration never parsed there are no annotations to mirror,
    /// so the call would travel with none of the `Mcp-Param-*` headers it asked
    /// for. An intermediary would see a call it cannot route or police, which
    /// is the one outcome the annotation exists to prevent -- so the call is
    /// refused instead of quietly sent unannotated.
    ///
    /// Only tools this client has seen rejected are known; one it never listed
    /// cannot be recognized.
    ///
    /// Sits on the send seam, so every request pays for it -- an empty set is
    /// checked first precisely so that the requests it is not about (and, in a
    /// healthy connection, all of them) stop at a single branch.
    #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
    #[inline]
    pub(super) fn blocked_tool_error(&self, req: &Request) -> Option<Error> {
        if self.options.rejected_tools.is_empty()
            || req.method.as_str() != crate::types::tool::commands::CALL
        {
            return None;
        }

        let name = req.params.as_ref()?.get("name")?.as_str()?;
        if !self.options.rejected_tools.contains(name) {
            return None;
        }

        Some(Error::new(
            ErrorCode::InvalidParams,
            format!(
                "Tool `{name}` was rejected for an invalid `x-mcp-header` declaration and cannot be called"
            ),
        ))
    }

    /// Requests a list of resources that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of resources if the MCP server provides pagination
    ///     let resources = client.list_resources(None).await?;
    ///     
    ///     // Fetch the next page of resources is any   
    ///     let resources = client.list_resources(resources.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list_resources(
        &mut self,
        cursor: Option<Cursor>,
    ) -> Result<ListResourcesResult, Error> {
        let params = ListResourcesRequestParams { cursor };
        self.command(crate::types::resource::commands::LIST, Some(params))
            .await?
            .into_result()
    }

    /// Requests a list of resource templates that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of resource templates if the MCP server provides pagination
    ///     let templates = client.list_resource_templates(None).await?;
    ///     
    ///     // Fetch the next page of resource templates is any   
    ///     let templates = client.list_resource_templates(templates.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list_resource_templates(
        &mut self,
        cursor: Option<Cursor>,
    ) -> Result<ListResourceTemplatesResult, Error> {
        let params = ListResourceTemplatesRequestParams { cursor };
        self.command(
            crate::types::resource::commands::TEMPLATES_LIST,
            Some(params),
        )
        .await?
        .into_result()
    }

    /// Requests a list of prompts that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of prompts if the MCP server provides pagination
    ///     let prompts = client.list_prompts(None).await?;
    ///     
    ///     // Fetch the next page of prompts templates is any   
    ///     let prompts = client.list_prompts(prompts.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list_prompts(
        &mut self,
        cursor: Option<Cursor>,
    ) -> Result<ListPromptsResult, Error> {
        let params = ListPromptsRequestParams { cursor };
        self.command(crate::types::prompt::commands::LIST, Some(params))
            .await?
            .into_result()
    }

    /// Calls a tool that MCP server supports
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let args = [("message", "Hello MCP!")]; // or let args = ("message", "Hello MCP!");
    ///     let result = client.call_tool("echo", args).await?;
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    ///
    /// # Structured output
    /// ```no_run
    /// use neva::prelude::*;
    ///
    /// #[json_schema(de)]
    /// struct Weather {
    ///     conditions: String,
    ///     temperature: f32,
    ///     humidity: f32,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let tools = client.list_tools(None).await?;
    ///
    ///     // Get the tool by name
    ///     let tool: &Tool = tools.get("weather-forecast")
    ///         .expect("Weather forecast tool not found");
    ///
    ///     let args = ("location", "London");
    ///     let result = client.call_tool("weather-forecast", args).await?;
    ///
    ///     // Validate the output structure and deserialize the result
    ///     let weather: Weather = tool
    ///         .validate(&result)
    ///         .and_then(|res| res.as_json())?;
    ///     
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn call_tool<N, Args>(
        &mut self,
        name: N,
        args: Args,
    ) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs,
    {
        let params = CallToolRequestParams {
            name: name.into(),
            meta: None,
            args: args.into_args(),
            #[cfg(feature = "tasks")]
            task: None,
        };

        self.call_tool_raw(params).await?.into_result()
    }

    /// Calls a task-augmented tool that MCP server supports
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let args = [("message", "Hello MCP!")]; // or let args = ("message", "Hello MCP!");
    ///     let result = client.call_tool_as_task("echo", args, None).await?;
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    ///
    /// # Structured output
    /// ```no_run
    /// use neva::prelude::*;
    ///
    /// #[json_schema(de)]
    /// struct Weather {
    ///     conditions: String,
    ///     temperature: f32,
    ///     humidity: f32,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let tools = client.list_tools(None).await?;
    ///
    ///     // Get the tool by name
    ///     let tool: &Tool = tools.get("weather-forecast")
    ///         .expect("Weather forecast tool not found");
    ///
    ///     let args = ("location", "London");
    ///     let result = client.call_tool_as_task("weather-forecast", args, None).await?;
    ///
    ///     // Validate the output structure and deserialize the result
    ///     let weather: Weather = tool
    ///         .validate(&result)
    ///         .and_then(|res| res.as_json())?;
    ///     
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "tasks")]
    pub async fn call_tool_as_task<N, Args>(
        &mut self,
        name: N,
        args: Args,
        ttl: Option<usize>,
    ) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs,
    {
        let builder = self.task();
        let builder = if let Some(t) = ttl {
            builder.with_ttl(t)
        } else {
            builder
        };

        builder.call_tool(name, args).await
    }

    /// Calls a tool
    #[inline]
    pub async fn call_tool_raw(
        &mut self,
        params: CallToolRequestParams,
    ) -> Result<Response, Error> {
        let id = self.generate_id()?;

        // Held back for the SEP-2243 retry: `with_meta` consumes the params,
        // and the call cannot be reconstructed from its own answer. A handful
        // of small allocations next to the round trip they may save.
        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
        let for_retry = params.clone();

        let request = Request::new(
            Some(id.clone()),
            crate::types::tool::commands::CALL,
            Some(params.with_meta(RequestParamsMeta::new(&id))),
        );

        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
        {
            let resp = self.send_request(request).await?;
            self.retry_after_header_mismatch(resp, for_retry).await
        }
        #[cfg(not(all(feature = "http-client", not(feature = "legacy-spec"))))]
        self.send_request(request).await
    }

    /// The second half of SEP-2243's stale-schema rule: re-list, then retry.
    ///
    /// Omitting `Mcp-Param-*` for a stale listing is what the client owes; a
    /// server that *requires* those headers answers the omission with
    /// `HeaderMismatch` (`-32020`). The spec's remedy is to fetch the current
    /// `inputSchema` and send the call again -- which is the whole reason
    /// omitting is safe: a caller never has to know its cached listing aged
    /// out.
    ///
    /// Exactly one retry, and only for `-32020`. A server that answers the
    /// fresh attempt the same way is saying something the listing cannot fix,
    /// and repeating would turn that into a loop the caller cannot see.
    ///
    /// The refresh follows `nextCursor` to the end of the listing, not merely
    /// until the refused tool turns up: a traversal that restarts clears what
    /// the previous one registered, so every page it does not reach is left
    /// with nothing -- no annotations for the tools on it, and no record of the
    /// ones that were dropped for a malformed declaration.
    #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
    pub(super) async fn retry_after_header_mismatch(
        &mut self,
        resp: Response,
        params: CallToolRequestParams,
    ) -> Result<Response, Error> {
        let Response::Err(ref err) = resp else {
            return Ok(resp);
        };
        if err.error.code != ErrorCode::HeaderMismatch {
            return Ok(resp);
        }

        // The whole listing, not just up to the refused tool. A cursor-less
        // call starts the traversal over and clears what the last one recorded,
        // so stopping early would leave every later page unregistered against a
        // registry that no longer holds their old entries: their calls would go
        // out without the headers they need, and a tool dropped for a malformed
        // annotation would stop being blocked -- which is the one outcome
        // dropping it exists to prevent.
        //
        // Fetched with grace: this listing is the server's current answer, and
        // the retry below is what it was fetched for. Judging it by its own TTL
        // instead would make the remedy impossible against the `ttlMs: 0` that
        // an absent `ttlMs` also means -- the re-fetch would be stale on
        // arrival, the retry would omit the headers again, and the call could
        // never succeed.
        //
        // A listing this client cannot obtain leaves the original answer as the
        // truthful one: it says the headers were wrong, and they still are.
        let name = params.name.clone();
        let mut cursor = None;
        let mut refreshed = false;
        // A server that keeps handing out cursors would otherwise walk this
        // recovery forever, and nothing above it can see that happening.
        for _ in 0..MAX_REFRESH_PAGES {
            let Ok(page) = self.list_tools_inner(cursor, Some(&name)).await else {
                return Ok(resp);
            };
            refreshed |= page.tools.iter().any(|tool| *tool.name == *name);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }

        // The traversal ran out -- the listing no longer carries this tool, or
        // it is paged further out than the cap reaches. Either way there is
        // nothing to retry *with*: the refresh started over and cleared the
        // registration, so a second attempt would go out exactly as bare as the
        // first and answer a different question. The original `HeaderMismatch`
        // is the useful answer and it stands.
        if !refreshed {
            return Ok(resp);
        }

        let id = self.generate_id()?;
        let retry = Request::new(
            Some(id.clone()),
            crate::types::tool::commands::CALL,
            Some(params.with_meta(RequestParamsMeta::new(&id))),
        );

        self.send_request(retry).await
    }

    /// Requests resource contents from MCP server
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let resource = client.read_resource("res://res_1").await?;
    ///     // Do something with the resource
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn read_resource(
        &mut self,
        uri: impl Into<Uri>,
    ) -> Result<ReadResourceResult, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::resource::commands::READ,
            Some(ReadResourceRequestParams {
                uri: uri.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                #[cfg(feature = "server")]
                args: None,
            }),
        );

        self.send_request(request).await?.into_result()
    }

    /// Gets a prompt that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let args = [
    ///         ("temperature", "50"),
    ///         ("style", "anything")
    ///     ];
    ///     let prompt = client.get_prompt("complex_prompt", args).await?;
    ///     // Do something with the prompt
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn get_prompt<N, Args>(
        &mut self,
        name: N,
        args: Args,
    ) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs,
    {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::prompt::commands::GET,
            Some(GetPromptRequestParams {
                name: name.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                args: args.into_args(),
            }),
        );

        self.send_request(request).await?.into_result()
    }
}

/// What the client will mirror into `Mcp-Param-*` headers is decided by the
/// current listing and nothing else: a tool the server no longer designates --
/// or no longer lists at all -- must stop sending its argument in a header.
#[cfg(all(test, feature = "http-client", not(feature = "legacy-spec")))]
mod param_header_registry_tests {
    use super::*;

    fn listing(tools: serde_json::Value) -> ListToolsResult {
        serde_json::from_value(serde_json::json!({ "tools": tools })).expect("valid listing")
    }

    fn listing_with_ttl(tools: serde_json::Value, ttl_ms: u64) -> ListToolsResult {
        serde_json::from_value(serde_json::json!({ "tools": tools, "ttlMs": ttl_ms }))
            .expect("valid listing")
    }

    fn annotated(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "inputSchema": {
                "type": "object",
                "properties": { "region": { "type": "string", "x-mcp-header": "Region" } }
            }
        })
    }

    fn plain(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
        })
    }

    #[test]
    fn a_fresh_listing_forgets_a_tool_it_no_longer_lists() {
        let mut client = Client::new();

        let mut first = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut first, true, None);
        assert!(client.options.param_headers.contains_key("search"));

        // The tool is gone from the refreshed listing -- a later direct
        // `call_tool("search", ..)` must not keep mirroring its argument.
        let mut second = listing(serde_json::json!([plain("other")]));
        client.register_param_headers(&mut second, true, None);
        assert!(client.options.param_headers.is_empty());
    }

    /// SEP-2243 has a client omit `Mcp-Param-*` while its cached `inputSchema`
    /// is stale, and SEP-2549 supplies the clock: `ttlMs: 0` -- which is also
    /// what an absent `ttlMs` means -- is stale on arrival. The annotation is
    /// still *recorded*: it says what the tool declared, and a fresh listing is
    /// what makes it sendable again.
    #[test]
    fn a_stale_listing_mirrors_nothing() {
        let mut client = Client::new();

        let mut immediate = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut immediate, true, None);
        {
            let entry = client
                .options
                .param_headers
                .get("search")
                .expect("registered");
            assert_eq!(
                entry.declared().len(),
                1,
                "the declaration is still what the tool said"
            );
            assert!(
                entry.usable().is_none(),
                "but nothing may be mirrored from a listing that is already stale"
            );
        }

        let mut with_room = listing_with_ttl(serde_json::json!([annotated("search")]), 60_000);
        client.register_param_headers(&mut with_room, true, None);
        let entry = client
            .options
            .param_headers
            .get("search")
            .expect("registered");
        assert_eq!(
            entry.usable().map(<[_]>::len),
            Some(1),
            "a listing with time left mirrors as before"
        );
    }

    /// SEP-2243's remedy for a refused call is to fetch the current schema and
    /// retry "with the appropriate headers". Against a server stating
    /// `ttlMs: 0` -- which is also what an absent `ttlMs` means, and what neva's
    /// own server sends by default -- that re-fetch is stale the instant it
    /// lands. Judging it by its own TTL would leave the retry omitting the
    /// headers again, so the remedy could never work and an annotated tool
    /// would be uncallable. The listing fetched *for* a retry is good for it,
    /// once.
    #[test]
    fn a_listing_fetched_for_a_retry_is_good_for_that_retry() {
        let mut client = Client::new();

        let mut refetched = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut refetched, true, Some("search"));

        let entry = client
            .options
            .param_headers
            .get("search")
            .expect("registered");
        assert_eq!(
            entry.usable().map(<[_]>::len),
            Some(1),
            "the retry this listing was fetched for must carry the headers"
        );
        assert!(
            entry.usable().is_none(),
            "and only that one: the listing is still stale for everything after"
        );
    }

    /// The grace belongs to the call that earned it. A refresh triggered by one
    /// tool re-registers every tool on the page, and handing them all the same
    /// exception would let the next call to a *different* tool mirror from a
    /// listing that was stale on arrival and that nothing refreshed on its
    /// behalf.
    #[test]
    fn the_retry_grace_does_not_spill_onto_other_tools() {
        let mut client = Client::new();

        let mut refetched = listing(serde_json::json!([
            annotated("search"),
            annotated("translate")
        ]));
        client.register_param_headers(&mut refetched, true, Some("search"));

        assert!(
            client
                .options
                .param_headers
                .get("search")
                .expect("registered")
                .usable()
                .is_some(),
            "the refused tool carries its retry"
        );
        assert!(
            client
                .options
                .param_headers
                .get("translate")
                .expect("registered")
                .usable()
                .is_none(),
            "a tool that was merely on the same page mirrors nothing"
        );
    }

    #[test]
    fn a_dropped_annotation_is_forgotten_on_the_same_tool() {
        let mut client = Client::new();

        let mut first = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut first, true, None);

        let mut second = listing(serde_json::json!([plain("search")]));
        client.register_param_headers(&mut second, true, None);
        assert!(client.options.param_headers.is_empty());
    }

    /// Where a listing came from does not change what it binds: a batched
    /// `tools/list` registers and filters exactly as a direct one does.
    #[test]
    fn a_batched_listing_registers_and_filters() {
        let mut client = Client::new();

        let mut resp = Response::success(
            RequestId::Number(1),
            serde_json::json!({
                "tools": [
                    annotated("search"),
                    {
                        "name": "broken",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "p": { "type": "array", "items": { "x-mcp-header": "P" } }
                            }
                        }
                    }
                ]
            }),
        );
        client.register_batched_tools(&mut resp);

        assert!(client.options.param_headers.contains_key("search"));
        assert!(!client.options.param_headers.contains_key("broken"));

        // The caller must not be handed a tool the client refuses to call.
        let Response::Ok(ok) = &resp else {
            panic!("a successful listing")
        };
        let tools = ok.result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "search");
    }

    /// A slot that is not a listing is the caller's to interpret.
    #[test]
    fn a_non_listing_response_is_left_alone() {
        let mut client = Client::new();
        let mut resp = Response::success(
            RequestId::Number(1),
            serde_json::json!({ "content": [{ "type": "text", "text": "hi" }] }),
        );
        let before = match &resp {
            Response::Ok(ok) => ok.result.clone(),
            Response::Err(_) => panic!("a successful response"),
        };

        client.register_batched_tools(&mut resp);

        let Response::Ok(ok) = &resp else {
            panic!("a successful response")
        };
        assert_eq!(ok.result, before);
        assert!(client.options.param_headers.is_empty());
    }

    #[test]
    fn later_pages_accumulate_onto_the_traversal() {
        let mut client = Client::new();

        let mut page1 = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut page1, true, None);

        // A tool absent from page two was not withdrawn, only listed earlier.
        let mut page2 = listing(serde_json::json!([annotated("lookup")]));
        client.register_param_headers(&mut page2, false, None);

        assert!(client.options.param_headers.contains_key("search"));
        assert!(client.options.param_headers.contains_key("lookup"));
    }

    #[test]
    fn an_invalid_definition_drops_the_tool_and_its_registration() {
        let mut client = Client::new();

        let mut first = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut first, true, None);

        // Same tool, now annotated somewhere the client cannot reach.
        let mut second = listing(serde_json::json!([{
            "name": "search",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "region": { "type": "array", "items": { "x-mcp-header": "Region" } }
                }
            }
        }]));
        client.register_param_headers(&mut second, true, None);

        assert!(second.tools.is_empty(), "a malformed tool is not callable");
        assert!(client.options.param_headers.is_empty());
    }

    fn call(name: &str) -> Request {
        Request::new(
            Some(RequestId::Number(1)),
            crate::types::tool::commands::CALL,
            Some(serde_json::json!({ "name": name, "arguments": {} })),
        )
    }

    /// Hiding the name from the listing is not enough on its own: a caller
    /// holding it from anywhere else would otherwise reach the tool with none
    /// of the headers its declaration asked for.
    #[test]
    fn a_rejected_tool_cannot_be_called_by_name() {
        let mut client = Client::new();

        let mut listed = listing(serde_json::json!([
            annotated("search"),
            {
                "name": "broken",
                "inputSchema": {
                    "type": "object",
                    "properties": { "p": { "type": "array", "items": { "x-mcp-header": "P" } } }
                }
            }
        ]));
        client.register_param_headers(&mut listed, true, None);

        let err = client
            .blocked_tool_error(&call("broken"))
            .expect("a rejected tool is refused");
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(client.blocked_tool_error(&call("search")).is_none());
        // Only `tools/call` names a tool.
        assert!(
            client
                .blocked_tool_error(&Request::new(
                    Some(RequestId::Number(1)),
                    crate::types::tool::commands::LIST,
                    Some(serde_json::json!({ "name": "broken" })),
                ))
                .is_none()
        );
    }

    /// The block follows the listing: a definition the server fixed -- or
    /// withdrew altogether -- is no longer the one being refused.
    #[test]
    fn a_fresh_listing_lifts_the_block() {
        let mut client = Client::new();

        let mut first = listing(serde_json::json!([{
            "name": "broken",
            "inputSchema": {
                "type": "object",
                "properties": { "p": { "type": "array", "items": { "x-mcp-header": "P" } } }
            }
        }]));
        client.register_param_headers(&mut first, true, None);
        assert!(client.blocked_tool_error(&call("broken")).is_some());

        let mut fixed = listing(serde_json::json!([annotated("broken")]));
        client.register_param_headers(&mut fixed, true, None);
        assert!(client.blocked_tool_error(&call("broken")).is_none());

        client.register_param_headers(&mut first, true, None);
        let mut gone = listing(serde_json::json!([plain("other")]));
        client.register_param_headers(&mut gone, true, None);
        assert!(client.blocked_tool_error(&call("broken")).is_none());
    }
}
