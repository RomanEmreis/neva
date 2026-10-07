//! The MCP methods a client calls on a server, and the machinery behind them.
//!
//! The methods themselves live in [`super::api`], one namespace per method
//! prefix; the flat ones here are their deprecated spellings. What stays is
//! `command` and the `tools/call` machinery: `Mcp-Param-*` header mirroring is
//! derived from the tool annotations this client last listed, so a call can be
//! rejected for headers built from a stale listing and has to recover by
//! re-listing and retrying once.

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
        &self,
        command: impl Into<String>,
        params: Option<T>,
    ) -> Result<Response, Error> {
        let id = self.generate_id()?;
        let request = Request::new(Some(id), command, params);
        self.send_request(request).await
    }

    /// Requests a list of tools that MCP server provides
    #[deprecated(since = "0.7.0", note = "use `client.tools().list(cursor)`")]
    #[inline]
    pub async fn list_tools(&self, cursor: Option<Cursor>) -> Result<ListToolsResult, Error> {
        self.tools().list(cursor).await
    }

    /// [`Tools::list`](super::api::Tools::list), plus the tool this listing was
    /// fetched to retry and the id of that retry -- which may mirror the tool's
    /// annotations regardless of the listing's TTL. Only that request: every
    /// other call, of this tool or any other on the page, is held to the TTL,
    /// and handing them the same exception would let them mirror from a
    /// listing nothing refreshed on their behalf.
    ///
    /// See [`Self::retry_after_header_mismatch`].
    pub(super) async fn list_tools_inner(
        &self,
        cursor: Option<Cursor>,
        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))] grace: Option<(
            &str,
            &RequestId,
        )>,
    ) -> Result<ListToolsResult, Error> {
        // A cursor-less call starts the listing over: if it ends in one page,
        // what it does not carry has been withdrawn.
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
    /// direct [`Tools::list`](super::api::Tools::list) performs, rewriting the
    /// response in place so the caller never sees a tool the client refuses to
    /// call.
    ///
    /// A batched listing is always a fresh traversal: [`BatchBuilder`] enqueues
    /// it without a cursor. A response that does not parse as a listing is left
    /// alone -- it is the caller's to interpret, and it registers nothing.
    #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
    pub(super) fn register_batched_tools(&self, resp: &mut Response) {
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
    /// Each tool on the page replaces its own registration in place, including
    /// replacing it with nothing: a server that drops an annotation must stop
    /// the client from mirroring that argument into a header. Nothing is
    /// cleared ahead of the pages, so a tool keeps the registration its last
    /// listing gave it until its own page arrives -- over a shared client
    /// another task can call it in between, and a cleared registration would
    /// send that call without the headers it needs.
    ///
    /// A tool the server withdrew is forgotten when a listing complete in one
    /// page no longer carries it. One withdrawn from a listing that spans
    /// pages keeps its registration, and with it mirrors only until the TTL of
    /// the listing that declared it runs out.
    ///
    /// The name of a rejected tool is remembered as well, so that hiding it
    /// from the listing is not all that hiding it does -- see
    /// [`Self::blocked_tool_error`]. That record is not cleared with the
    /// registry. A block is lifted only on evidence: a page that lists the tool
    /// with a declaration that parses, or a listing complete in one page that
    /// no longer carries it. A traversal merely starting is no evidence -- the
    /// malformed tool may sit on a later page -- and over a shared client
    /// another task can call it in between, sent without the headers its
    /// declaration asked for. A tool withdrawn from a listing that spans pages
    /// therefore stays refused, under the reason the last listing that carried
    /// it gave, until one of the two arrives.
    #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
    pub(super) fn register_param_headers(
        &self,
        result: &mut ListToolsResult,
        fresh: bool,
        grace: Option<(&str, &RequestId)>,
    ) {
        use crate::shared::param_headers;

        // The whole listing in one page: what it does not carry, the server
        // has withdrawn.
        let complete = fresh && result.next_cursor.is_none();
        let mut rejected_here = Vec::new();

        // How long this listing may be mirrored from. The spec makes `ttlMs`
        // mandatory and reads an absent one as `0` -- immediately stale -- so
        // every registration is stamped with the listing that produced it.
        let ttl_ms = result.ttl_ms;
        let registry = &self.options.param_headers.tools;

        result.tools.retain(|tool| {
            // Replaced or removed in one step, never removed and put back: the
            // gap between the two would be a moment for a call to go out bare.
            let schema = match serde_json::to_value(&tool.input_schema) {
                Ok(schema) => schema,
                Err(_) => {
                    registry.remove(&*tool.name);
                    self.options.rejected_tools.remove(&*tool.name);
                    return true;
                }
            };

            match param_headers::collect(&schema) {
                Ok(headers) => {
                    // Lifted here and not before the parse: a block taken away
                    // and put back would leave a moment for a call to pass.
                    self.options.rejected_tools.remove(&*tool.name);
                    if headers.is_empty() {
                        registry.remove(&*tool.name);
                    } else {
                        if let Some((_, retry)) = grace.filter(|(name, _)| *name == &*tool.name) {
                            self.options
                                .param_headers
                                .retries
                                .insert(retry.clone(), headers.clone());
                        }
                        registry.insert(
                            tool.name.to_string(),
                            param_headers::Registration::new(headers, ttl_ms),
                        );
                    }
                    true
                }
                Err(_err) => {
                    #[cfg(feature = "tracing")]
                    tracing::warn!(logger = "neva", "Dropping tool `{}`: {_err}", tool.name);
                    registry.remove(&*tool.name);
                    self.options.rejected_tools.insert(tool.name.to_string());
                    if complete {
                        rejected_here.push(tool.name.to_string());
                    }
                    false
                }
            }
        });

        // A retry's own exception is left alone: it belongs to a call still in
        // flight, which a listing someone else fetched must not strand.
        if complete {
            let listed: std::collections::HashSet<&str> =
                result.tools.iter().map(|tool| &*tool.name).collect();
            registry.retain(|name, _| listed.contains(name.as_str()));
            self.options
                .rejected_tools
                .retain(|name| rejected_here.contains(name));
        }
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
    #[deprecated(since = "0.7.0", note = "use `client.resources().list(cursor)`")]
    #[inline]
    pub async fn list_resources(
        &self,
        cursor: Option<Cursor>,
    ) -> Result<ListResourcesResult, Error> {
        self.resources().list(cursor).await
    }

    /// Requests a list of resource templates that MCP server provides
    #[deprecated(since = "0.7.0", note = "use `client.resources().templates(cursor)`")]
    #[inline]
    pub async fn list_resource_templates(
        &self,
        cursor: Option<Cursor>,
    ) -> Result<ListResourceTemplatesResult, Error> {
        self.resources().templates(cursor).await
    }

    /// Requests a list of prompts that MCP server provides
    #[deprecated(since = "0.7.0", note = "use `client.prompts().list(cursor)`")]
    #[inline]
    pub async fn list_prompts(&self, cursor: Option<Cursor>) -> Result<ListPromptsResult, Error> {
        self.prompts().list(cursor).await
    }

    /// Calls a tool that MCP server supports
    #[deprecated(since = "0.7.0", note = "use `client.tools().call(name, args)`")]
    #[inline]
    pub async fn call_tool<N, Args>(&self, name: N, args: Args) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs,
    {
        self.tools().call(name, args).await
    }

    /// Calls a tool as a task and waits for its result
    #[cfg(feature = "tasks")]
    #[deprecated(
        since = "0.7.0",
        note = "use `client.tools().as_task().with_ttl(ttl).call(name, args)`"
    )]
    pub async fn call_tool_as_task<N, Args>(
        &self,
        name: N,
        args: Args,
        ttl: Option<usize>,
    ) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs,
    {
        let builder = self.tools().as_task();
        let builder = match ttl {
            Some(ttl) => builder.with_ttl(ttl),
            None => builder,
        };

        builder.call(name, args).await
    }

    /// Calls a tool
    #[deprecated(since = "0.7.0", note = "use `client.tools().call_raw(params)`")]
    #[inline]
    pub async fn call_tool_raw(&self, params: CallToolRequestParams) -> Result<Response, Error> {
        self.tools().call_raw(params).await
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
    /// The refresh follows `nextCursor` until the refused tool turns up. A
    /// traversal starting over clears nothing, so the pages it does not reach
    /// keep what their last listing registered.
    #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
    pub(super) async fn retry_after_header_mismatch(
        &self,
        resp: Response,
        mut params: CallToolRequestParams,
    ) -> Result<Response, Error> {
        let Response::Err(ref err) = resp else {
            return Ok(resp);
        };
        if err.error.code != ErrorCode::HeaderMismatch {
            return Ok(resp);
        }

        // Fetched for the retry below, by its id: this listing is the server's
        // current answer, and that request is what it was fetched for. Judging
        // it by its own TTL instead would make the remedy impossible against
        // the `ttlMs: 0` that an absent `ttlMs` also means -- the re-fetch
        // would be stale on arrival, the retry would omit the headers again,
        // and the call could never succeed. The id is fixed first so the
        // exception can name the one request it is for; the guard takes it
        // away again however the retry ends.
        //
        // A listing this client cannot obtain leaves the original answer as the
        // truthful one: it says the headers were wrong, and they still are.
        let name = params.name.as_str();
        let id = self.generate_id()?;
        let _grace = self.options.param_headers.retry_grace(&id);
        let mut cursor = None;
        let mut refreshed = false;
        // A server that keeps handing out cursors would otherwise walk this
        // recovery forever, and nothing above it can see that happening.
        for _ in 0..api::MAX_LIST_PAGES {
            let Ok(page) = self.list_tools_inner(cursor, Some((name, &id))).await else {
                return Ok(resp);
            };
            if page.tools.iter().any(|tool| *tool.name == *name) {
                refreshed = true;
                break;
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }

        // The traversal ran out -- the listing no longer carries this tool, or
        // it is paged further out than the cap reaches. Either way there is
        // nothing to retry *with*: no current schema came back for it, so a
        // second attempt would go out exactly as the first did and answer a
        // different question. The original `HeaderMismatch` is the useful
        // answer and it stands.
        if !refreshed {
            return Ok(resp);
        }

        // The caller's `_meta` again, under this request's progress token.
        params.track_progress(&id);
        let retry = Request::new(
            Some(id.clone()),
            crate::types::tool::commands::CALL,
            Some(params),
        );

        self.send_request(retry).await
    }

    /// Requests resource contents from MCP server
    #[deprecated(since = "0.7.0", note = "use `client.resources().read(uri)`")]
    #[inline]
    pub async fn read_resource(&self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        self.resources().read(uri).await
    }

    /// Gets a prompt that MCP server provides
    #[deprecated(since = "0.7.0", note = "use `client.prompts().get(name, args)`")]
    #[inline]
    pub async fn get_prompt<N, Args>(&self, name: N, args: Args) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs,
    {
        self.prompts().get(name, args).await
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
        let client = Client::new();

        let mut first = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut first, true, None);
        assert!(client.options.param_headers.tools.contains_key("search"));

        // The tool is gone from the refreshed listing -- a later direct
        // `call_tool("search", ..)` must not keep mirroring its argument.
        let mut second = listing(serde_json::json!([plain("other")]));
        client.register_param_headers(&mut second, true, None);
        assert!(client.options.param_headers.tools.is_empty());
    }

    /// SEP-2243 has a client omit `Mcp-Param-*` while its cached `inputSchema`
    /// is stale, and SEP-2549 supplies the clock: `ttlMs: 0` -- which is also
    /// what an absent `ttlMs` means -- is stale on arrival. The annotation is
    /// still *recorded*: it says what the tool declared, and a fresh listing is
    /// what makes it sendable again.
    #[test]
    fn a_stale_listing_mirrors_nothing() {
        let client = Client::new();

        let mut immediate = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut immediate, true, None);
        {
            let entry = client
                .options
                .param_headers
                .tools
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
            .tools
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
    /// would be uncallable. The listing fetched *for* a retry is good for that
    /// retry -- named by its request id -- and for nothing else.
    #[test]
    fn a_listing_fetched_for_a_retry_is_good_for_that_retry() {
        let client = Client::new();
        let retry = RequestId::Number(7);

        let mut refetched = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut refetched, true, Some(("search", &retry)));

        let registry = &client.options.param_headers;
        assert_eq!(
            registry.retries.get(&retry).map(|headers| headers.len()),
            Some(1),
            "the retry this listing was fetched for must carry the headers"
        );
        assert!(
            registry
                .tools
                .get("search")
                .expect("registered")
                .usable()
                .is_none(),
            "and only that one: the listing is still stale for every other call"
        );
    }

    /// The exception belongs to the call that earned it. A refresh triggered by
    /// one tool re-registers every tool on the page, and handing them all the
    /// same exception would let the next call to a *different* tool mirror from
    /// a listing that was stale on arrival and that nothing refreshed on its
    /// behalf.
    #[test]
    fn the_retry_grace_does_not_spill_onto_other_tools() {
        let client = Client::new();
        let retry = RequestId::Number(7);

        let mut refetched = listing(serde_json::json!([
            annotated("search"),
            annotated("translate")
        ]));
        client.register_param_headers(&mut refetched, true, Some(("search", &retry)));

        let registry = &client.options.param_headers;
        assert_eq!(
            registry.retries.len(),
            1,
            "one exception, for the refused call's retry"
        );
        assert!(
            registry
                .tools
                .get("translate")
                .expect("registered")
                .usable()
                .is_none(),
            "a tool that was merely on the same page mirrors nothing"
        );
    }

    /// Another caller's listing, landing between a refused call's refresh and
    /// its retry, starts the registry over -- and must leave the retry its
    /// exception, or the retry goes out bare and is refused again.
    #[test]
    fn a_listing_started_over_keeps_a_pending_retry_exception() {
        let client = Client::new();
        let retry = RequestId::Number(7);

        let mut refetched = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut refetched, true, Some(("search", &retry)));

        let mut unrelated = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut unrelated, true, None);

        assert!(
            client.options.param_headers.retries.contains_key(&retry),
            "the retry still carries its headers"
        );
    }

    #[test]
    fn a_dropped_annotation_is_forgotten_on_the_same_tool() {
        let client = Client::new();

        let mut first = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut first, true, None);

        let mut second = listing(serde_json::json!([plain("search")]));
        client.register_param_headers(&mut second, true, None);
        assert!(client.options.param_headers.tools.is_empty());
    }

    /// Where a listing came from does not change what it binds: a batched
    /// `tools/list` registers and filters exactly as a direct one does.
    #[test]
    fn a_batched_listing_registers_and_filters() {
        let client = Client::new();

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

        assert!(client.options.param_headers.tools.contains_key("search"));
        assert!(!client.options.param_headers.tools.contains_key("broken"));

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
        let client = Client::new();
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
        assert!(client.options.param_headers.tools.is_empty());
    }

    #[test]
    fn later_pages_accumulate_onto_the_traversal() {
        let client = Client::new();

        let mut page1 = listing(serde_json::json!([annotated("search")]));
        client.register_param_headers(&mut page1, true, None);

        // A tool absent from page two was not withdrawn, only listed earlier.
        let mut page2 = listing(serde_json::json!([annotated("lookup")]));
        client.register_param_headers(&mut page2, false, None);

        assert!(client.options.param_headers.tools.contains_key("search"));
        assert!(client.options.param_headers.tools.contains_key("lookup"));
    }

    /// A traversal starting over must leave the tools on its later pages as
    /// their last listing registered them: over a shared client another task
    /// can call one before its page arrives, and a cleared registration would
    /// send that call without its headers.
    #[test]
    fn a_restarted_traversal_keeps_later_pages_registered() {
        let client = Client::new();
        let page = |tools: serde_json::Value, next: Option<Cursor>| -> ListToolsResult {
            serde_json::from_value(serde_json::json!({
                "tools": tools,
                "ttlMs": 60_000,
                "nextCursor": next.map(|cursor| serde_json::to_value(cursor).expect("a cursor")),
            }))
            .expect("valid listing")
        };
        let id = RequestId::Number(1);
        let args = serde_json::json!({ "region": "us-west1" });

        let mut first = page(serde_json::json!([annotated("search")]), Some(Cursor(10)));
        client.register_param_headers(&mut first, true, None);
        let mut second = page(serde_json::json!([annotated("lookup")]), None);
        client.register_param_headers(&mut second, false, None);

        let mut restarted = page(serde_json::json!([annotated("search")]), Some(Cursor(10)));
        client.register_param_headers(&mut restarted, true, None);

        assert_eq!(
            client
                .options
                .param_headers
                .mirrored(&id, "lookup", &args)
                .len(),
            1,
            "the second page has not arrived yet, so its tool still mirrors"
        );
    }

    #[test]
    fn an_invalid_definition_drops_the_tool_and_its_registration() {
        let client = Client::new();

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
        assert!(client.options.param_headers.tools.is_empty());
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
        let client = Client::new();

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

    /// A traversal starting over is no evidence that a block can go: the
    /// malformed tool may sit on a later page, and over a shared client another
    /// task can call it before the refresh gets there. Only a page that lists
    /// it well-formed lifts the block.
    #[test]
    fn a_partial_refresh_keeps_the_block() {
        let client = Client::new();
        let continued = |tools: serde_json::Value| -> ListToolsResult {
            serde_json::from_value(serde_json::json!({
                "tools": tools,
                "nextCursor": serde_json::to_value(Cursor(10)).expect("a cursor"),
            }))
            .expect("valid listing")
        };
        let broken = serde_json::json!({
            "name": "broken",
            "inputSchema": {
                "type": "object",
                "properties": { "p": { "type": "array", "items": { "x-mcp-header": "P" } } }
            }
        });

        let mut first = continued(serde_json::json!([plain("other"), broken]));
        client.register_param_headers(&mut first, true, None);
        assert!(client.blocked_tool_error(&call("broken")).is_some());

        // A new traversal's first page, without the malformed tool on it.
        let mut restarted = continued(serde_json::json!([plain("other")]));
        client.register_param_headers(&mut restarted, true, None);
        assert!(
            client.blocked_tool_error(&call("broken")).is_some(),
            "the rest of the listing has not arrived, so the block stands"
        );

        // A later page of it, where the server now declares the tool properly.
        let mut fixed = listing(serde_json::json!([annotated("broken")]));
        client.register_param_headers(&mut fixed, false, None);
        assert!(client.blocked_tool_error(&call("broken")).is_none());
    }

    /// The block follows the listing: a definition the server fixed -- or
    /// withdrew altogether -- is no longer the one being refused.
    #[test]
    fn a_fresh_listing_lifts_the_block() {
        let client = Client::new();

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
