# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## 0.5.2

### Added
* **DNS-rebinding protection: the HTTP server validates `Origin` and `Host`.**
  A server on loopback is reachable by any page the browser loads -- point
  `evil.example.com` at `127.0.0.1` and the browser connects; the request is
  genuinely local, and only the name it was addressed by gives it away. The
  spec makes validating these headers a MUST for local servers, and neva did
  not.
  * Bound to loopback, a server now answers only to loopback names
    (`localhost`, anything in `127.0.0.0/8`, `[::1]`) on any port, and refuses
    anything else with `403 Forbidden` before the body is read. POST, GET and
    DELETE are all gated.
  * Bound to anything else, it accepts everything as before: behind a proxy the
    `Host` is whatever that proxy forwards, so the legitimate names are not
    knowable from here.
  * A request carrying neither header is left alone -- there is no rebinding
    without a name to rebind, and non-browser callers send neither.
  * New `HttpServer::with_allowed_origins([...])` names additional hosts (and
    turns the gate on for a non-loopback bind);
    `HttpServer::allow_any_origin()` turns it off for a deployment whose name
    is validated in front of the server.

### Fixed
* **A `resources` capability that omits `subscribe` no longer fails to parse.**
  Every member of a capability object is optional, and a server that supports
  neither subscriptions nor list-change notifications advertises
  `"resources": {}`. `subscribe` was the one member without a default, so such a
  server's capabilities came back as
  `missing field \`subscribe\`` -- and since capabilities arrive with the
  handshake, the connection failed outright rather than degrading.
* **A mirrored header value carrying a horizontal tab is Base64-encoded.** The
  spec quotes RFC 9110's field-value set, which admits HTAB, and then names
  control characters as a reason to encode; HTAB is one. The readings differ
  only for a tab in the middle of a value, and encoding is the safe way to
  differ -- decoding the sentinel is mandatory for every peer, whereas a bare
  tab is exactly the byte an intermediary may fold into a space.
* **A server that hosts no standalone SSE stream no longer ends the session
  (legacy profile).** The client opens the optional `GET` stream after the
  `initialize` handshake and treated any non-2xx as fatal, cancelling the
  session -- so a conformant server that simply chose not to offer one (the
  spec has it answer `405 Method Not Allowed`) was reported as
  `Connection closed` before the first request went out. `404` and `405` are
  now read as "there is no stream here": the pending `initialize` is released
  and the session carries on over POST alone. Any other non-2xx is still
  fatal -- a `401` says the credentials the POSTs carry are wrong too.
* **The POST no longer carries `Content-Type` twice.** It was set once by the
  JSON body and once explicitly, and `.header()` appends: the header reached
  the peer as `application/json, application/json`, which matches no media type
  and drew `415 Unsupported Media Type` from strict receivers.
* **A tool call with no arguments omits `arguments` instead of sending
  `null`.** The schema types it as an optional object; `null` is not one, and a
  peer that validates rejected the call over the field it was not given.
* **A schema is published the way it was declared.** `Schema` and (on the
  legacy profile) `ToolSchema` modelled the keywords neva itself reads and
  dropped everything else, so a peer received a schema quietly different from
  the one the server wrote -- wider, in the cases that matter.
  * Both now carry an `extra` catch-all, flattened on the wire, that keeps every
    unmodelled keyword verbatim: `default` (SEP-1034), `pattern`, `examples`,
    and on a tool's `inputSchema` the `$schema`, `$defs`, `$ref`,
    `additionalProperties`, `allOf`/`anyOf` and `if`/`then`/`else` that SEP-2106
    requires to survive untouched.
  * **Breaking:** `PropertyType` gains an `Integer` variant, and `"integer"` no
    longer deserializes into `Number`. The two are different types in JSON
    Schema -- `integer` rejects `1.5` -- and sharing a variant meant a declared
    `"integer"` was published as `"number"`. A match on `PropertyType` needs the
    new arm; `Schema::integer()` builds one.
  * **Breaking:** the schema structs in `neva::types::schema` gain the `extra`
    field, so a struct literal that names every field needs it (or
    `..Default::default()`).
* **MRTR `inputResponses` / `requestState` travel on the params, not in
  `_meta`.** The spec puts both on `InputResponseRequestParams`, beside `name`
  and `arguments`; neva read and wrote them inside `_meta`, so its client and
  server agreed with each other and with no one else -- against any other
  implementation the retry looked like a fresh call and the round-trip never
  completed.
  * A request is read from the spec location first and from `_meta` only as a
    fallback, so a 0.5.x neva client keeps working against a newer server.
    Nothing this build sends carries the old location.
  * The MRTR request binding ignores both fields (as it already ignored
    `_meta`): a request does not become a different request by being answered,
    and hashing the answers in would have made every `requestState` fail
    verification on the round it was minted for.
  * New `Request::input_responses()` and `Request::state()` read whichever
    location a peer used.
* **Elicitation params are written as the spec's union, not as a tagged enum.**
  `ElicitRequestParams` derived its `Serialize`/`Deserialize`, so an
  `elicitation/create` carried `{"Form": {"message": ..., "requestedSchema":
  ...}}`. The spec types the params as a bare union -- `ElicitRequestFormParams
  | ElicitRequestURLParams` -- whose fields sit directly in `params`, so no
  conforming client found `message` or `requestedSchema` where it looks for
  them and elicitation did not work with any peer but neva. The variant is now
  chosen by the `mode` discriminator on the way in (absent or `"form"` for a
  form, `"url"` for a URL request) and written flat on the way out; a form no
  longer emits `"mode": null`, which is not one of the values the spec allows.
  Affects both protocol profiles.
* **An unimplemented method answers `404 Not Found` over HTTP.** The JSON-RPC
  code was already `-32601`, but the status was `200`. The spec pins the `404`
  so a caller can tell "this endpoint speaks MCP and has no such method" from
  "this URL is not an MCP endpoint" without parsing the body -- the JSON-RPC
  error body is what distinguishes the two. Found by the conformance suite.
* **A protocol version stated in the body that disagrees with the
  `MCP-Protocol-Version` header is a `HeaderMismatch` (`-32020`).** It was
  reported as `UnsupportedProtocolVersion` (`-32022`), because the body version
  was checked against the supported set before it was compared to the header.
  The two say different things to whoever has to fix them: `-32022` means
  "retry with a version from this list", which does not fix a header and a body
  that disagree.
* **`resources/read` for an unknown URI names it in `error.data.uri`.** A
  spec SHOULD; the error carried `data: null`, so a caller with several reads in
  flight could not tell which one it was about without matching on the request
  id.
* **Per-request `clientCapabilities` are read in the shape the spec defines.**
  `io.modelcontextprotocol/clientCapabilities` holds a `ClientCapabilities`
  object, in which each capability is itself an *optional object* whose presence
  is the declaration (`{"elicitation": {"form": {}}, "roots": {}}`). neva read
  the members as booleans, so every request from a spec-conformant client --
  MCP Inspector among them -- failed to parse its `_meta` and came back as
  `-32602 invalid type: map, expected a boolean`, making `tools/call`,
  `resources/read` and `prompts/get` unusable. Both shapes are now accepted on
  the way in (the boolean for older neva clients), and neva's own client writes
  the object shape.
* **Tool and prompt arguments are extracted by name, not by position.**
  * A failure now names the argument: ``missing required argument `age` `` and
    ``invalid value for argument `age`: ... `` instead of
    `Invalid param provided`.
  * An argument a peer omitted is offered to the handler as `null`, so an
    absent optional argument no longer errors on arrival.
  * `Meta<_>`, `Context` and `Dc<_>` parameters consume no argument slot, so
    they can sit anywhere in the signature without shifting the rest. The
    `#[tool]` / `#[prompt]` macros classify them from the *resolved* type, so a
    parameter reaching the signature through a type alias
    (`type Token = Meta<ProgressToken>`) is recognised too. The same goes for
    an argument's published JSON type and whether it is required: a
    `type MaybeAge = Option<i32>` parameter now publishes exactly what the
    spelled-out type does.
  * Whether an argument may be omitted is decided by its type, never by whether
    a synthetic `null` happens to deserialize into it -- otherwise a required
    `serde_json::Value` argument would silently arrive as `Null`.
* **Tools registered from a closure publish one property per argument.** The
  generated `inputSchema` keyed its properties by *type name*, so
  `|a: i32, b: i32|` advertised a single `number` property and the second
  argument had nowhere to travel in. Properties are now named per argument and
  listed in `required`. Prompt arguments likewise no longer publish
  `std::any::type_name` output.

### Added
* **`Option<T>` tool and prompt arguments.**
  * New `ToolArg` (the return type of `ToolHandler::args`) carries the
    published property together with whether a call must supply it.
  * New `PromptArgument::named(name, required)`.
  * A tool whose arguments are all optional now publishes no `required` key
    rather than an empty array.
* `Tool::with_arg_names([...])` declares the names of a handler's arguments.
  Rust does not keep a closure's parameter names, so a tool registered from a
  bare closure publishes and reads the positional `arg0`, `arg1`, ... names
  until this is called; it renames the generated schema and the extraction
  names together, so the two cannot drift.
* `map_tool!` / `map_prompt!` read the names off the closure itself:
  `map_tool!(app, "greet", |name: String, age: i32| async move { ... })`
  registers the tool with `name` and `age` and skips metadata parameters.
* `App::run` fails at startup when a tool or prompt and its handler disagree
  about the arguments -- a tool whose schema was overridden without
  `with_arg_names`, a miscounted `with_arg_names`, declared names the schema
  does not offer as properties, a `Prompt::with_args` list that does not cover
  every argument the handler takes, or the same name given to two arguments.
  None of these could ever be called
  successfully, and this reports them before serving instead of on a peer's
  first request. A schema that composes -- `$ref`, `allOf`, `oneOf`, a
  conditional branch -- may publish an argument the check cannot follow, so it
  is left alone rather than failed on a guess. `Context::add_tool` and
  `Context::add_prompt` run the same check and refuse the insertion, since a
  primitive registered while the server runs has no startup left to fail.
* `ArgNames` and `FromHandlerArgs` in `neva::types`.

### Changed
* **Breaking:** `App::map_tool` / `Tool::new` take
  `Args: FromHandlerArgs<CallToolRequestParams>` and `App::map_prompt` /
  `Prompt::new` take `Args: FromHandlerArgs<GetPromptRequestParams>`, replacing
  the `TryFrom<...Params>` bounds. Handlers themselves are unaffected; a
  hand-written `impl TryFrom<CallToolRequestParams> for MyArgs` needs porting.
* **Breaking:** `HandlerParams::Tool` and `HandlerParams::Prompt` carry the
  primitive's `ArgNames` alongside the params.
* **Breaking:** `ToolHandler::args` returns `Vec<ToolArg>` instead of
  `Option<HashMap<String, SchemaProperty>>` -- ordered, so the *n*-th entry is
  the *n*-th argument slot, and carrying each argument's `required` flag.
* **Breaking (wire):** a tool registered from a bare closure advertises
  `arg0`, `arg1`, ... instead of the former type names (`number`, `string`).
  Tools declared with `#[tool]` are unaffected -- they already published their
  parameter names, and now read by them.
* `Prompt::with_args` sets the extraction names along with the published
  argument list, so the two are one decision. `#[prompt]` needs no change.

## 0.5.1

### Added
* **`subscriptions/listen`** (MCP 2026-07-28, #95). The final spec replaced the
  HTTP `GET` stream *and* `resources/subscribe`/`unsubscribe` with one
  long-lived request carrying a notification filter. This is how server-push
  works again on the stateless transport, and it **retracts the note carried
  since 0.4.x that "server-initiated notifications are inert; clients poll
  instead"** -- that limitation described the RC, not the final spec.
  * New `SubscriptionFilter` (`toolsListChanged` / `promptsListChanged` /
    `resourcesListChanged` / `resourceSubscriptions`) with `intersection`,
    `is_subset_of`, `supported_by` and `matches`, plus
    `SubscriptionsListenRequestParams`, `SubscriptionsListenResult`,
    `SubscriptionsAcknowledgedNotificationParams` and `SubscriptionMeta`.
  * **Server:** `subscriptions/listen` is handled by neva itself -- there is no
    handler to write. The accepted filter is the requested one narrowed to the
    advertised capabilities, acknowledged as the first message on the stream
    (`notifications/subscriptions/acknowledged`), and every message carries
    `_meta["io.modelcontextprotocol/subscriptionId"]`. `Context::add_tool`,
    `remove_tool`, `add_prompt`, `remove_prompt`, `add_resource`,
    `remove_resource` and `resource_updated` fan out to the streams that asked
    for them; `Context::is_subscribed` now answers from the live streams. A
    subscription ends on `notifications/cancelled` (stdio's mechanism, and
    stdio's only -- over HTTP that notification travels on its own `POST` and
    proves nothing about who opened the stream), on the client closing the
    stream (HTTP's mechanism), on transport close, or on server shutdown.
  * **Client:** `Client::listen(SubscriptionFilter)` returns a `Subscription`
    once the server acknowledges, exposing `id()`, `requested()`,
    `acknowledged()`, `is_fully_honored()`, `cancel()` and
    `closed() -> SubscriptionEnd` (`Graceful` / `Cancelled` / `Abrupt`).
    Dropping the handle ends the subscription too, and so does
    `Client::disconnect`, so neither can leave the peer streaming into a client
    with no way left to stop it.
  * Notifications keep flowing to the handlers registered with
    `Client::subscribe` / `on_tools_changed` / `on_resource_changed`, so
    existing client code needs no change. What reaches them is what the
    subscription acknowledged: the acknowledgment is a promise about the whole
    stream, and anything outside it -- or ahead of it -- is dropped rather than
    dispatched to handlers that know nothing about subscriptions.
  * `Client::call_batch` rejects a batched `subscriptions/listen` with
    `InvalidRequest`. A batch slot is an ordinary request slot -- finite TTL, a
    plain `Response`, no handle -- so a subscription opened that way would have
    nothing to cancel it and would outlive the call that made it.
  * Works on both transports: over HTTP the subscription rides the listen
    `POST`'s own `text/event-stream` body (a client disconnect ends it); over
    stdio it interleaves on stdout and ends on `notifications/cancelled`.
  * `Subscription` and `SubscriptionEnd` are re-exported from `neva::prelude`
    alongside `Client`; the filter types arrive there with the rest of
    `neva::types`.
  * New `examples/subscriptions/` (server + client over HTTP).

### Changed (breaking)
* **`listChanged` and `resources.subscribe` are advertised again.** Both were
  masked off in the default build because nothing could deliver them; a listen
  stream now can. Servers that configure `with_list_changed()` /
  `with_subscribe()` start seeing those capabilities on the wire, and clients
  narrow their listen filter against exactly what a server advertises.
* **`resources/subscribe` / `resources/unsubscribe` are legacy-only.** The RPC
  pair is not deleted by the spec, it is folded into
  `SubscriptionFilter::resource_subscriptions` -- a subscription is now a URI on
  a stream rather than server-side state.
  * `Context::subscribe_to_resource` / `unsubscribe_from_resource` and
    `resource::commands::{SUBSCRIBE, UNSUBSCRIBE}` are behind `legacy-spec`;
    `UnsubscribeRequestParams` is behind `any(legacy-spec, client)`.
    `SubscribeRequestParams` stays in both profiles -- it is the `{uri}` payload
    of `notifications/resources/updated`.
  * `Client::subscribe_to_resource` / `unsubscribe_from_resource` stay compiled
    (the dual-mode fallback still reaches legacy peers) but now reject a
    2026-07-28 peer with `MethodNotFound`. Use `Client::listen` with a
    `resourceSubscriptions` entry instead.
  * Migration: replace `client.subscribe_to_resource(uri)` with
    `client.listen(SubscriptionFilter::new().with_resource(uri))`, and drop
    `ctx.subscribe_to_resource(..)` from server handlers -- the client owns the
    subscription now.

### Changed
* `ServerCapabilities` now derives `Default`, so a filter can be narrowed
  against a partially-built capability set.
* **The client's receive loop now stops when the transport is cancelled**, and
  fails every still-pending request on the way out. Over HTTP the receiver
  holds a sender clone of its own channel, so the loop never ended by itself
  and outlived the connection it served; a disconnect now ends both.
* **The streaming `POST` reply no longer requires the `tracing` feature.**
  `dispatch_post` produces the `StreamResponse::Stream` arm in any default-build
  configuration -- a subscription stream is not a logging concern. Internally
  the per-`POST` notification sink moved out of the `tracing`-gated
  `notification::fmt` into a new internal `notification::sink`.

## 0.5.0

### Changed (breaking)
* **MCP 2026-07-28 is now the default protocol generation.** The
  `proto-2026-07-28-rc` opt-in flag is gone; what it used to gate is what a
  plain `neva` build now speaks. The previous default -- MCP 2024-11-05 ..
  2025-11-25 -- moves behind a new **`legacy-spec`** feature. This is the
  deliberate breaking change flagged since 0.4.0: the spec revision itself is
  breaking, and neva follows it rather than freezing on the old wire.
  * Migration: builds that enabled `proto-2026-07-28-rc` simply drop the flag.
    Builds that relied on the old default add `features = ["legacy-spec"]`.
  * `with_mcp_version` (server side) is available again -- under `legacy-spec`,
    where version selection is meaningful. The default build pins `2026-07-28`.
  * The `roots` and `sampling` examples swapped places: the MCP 2026-07-28
    variants are now `examples/{roots,sampling}/{server,client}` and the legacy
    ones moved to `examples/{roots,sampling}/legacy/{server,client}`.
  * Note that `--all-features` enables `legacy-spec`, so it now exercises the
    legacy profile; the default profile needs an explicit feature list
    (e.g. `--features "server-full client-full"`).
* **Unified streaming-capable POST seam** for HTTP engine adapters. Streamable
  HTTP has allowed a POST reply to be either a single JSON body or an SSE
  stream since spec revision 2025-03-26; neva's engine seam only modeled the
  JSON shape. Now:
  * `SseResponse` is renamed to `StreamResponse` and its `Status` variant to
    `Complete` (it carries full JSON replies, not just error statuses). A
    deprecated `SseResponse` type alias remains for one release.
  * `handlers::dispatch_post` returns
    `StreamResponse<impl Stream<Item = E::SseEvent>>` instead of
    `E::Response`: engines handle the same two-arm match their GET route
    already has. In the default (MCP 2026-07-28) build with `tracing` the `Stream` arm
    carries request-scoped notifications followed by the response; other
    builds always produce `Complete` (no behavior change).
  * `handle_post` stays available as the JSON-only building block.
  * The default Volga adapter now goes through `dispatch_post` like every
    other engine (claims moved into `VolgaEngine::adapt_request`, per the
    `HttpEngine` contract); the axum/hyper/actix engine examples were updated
    to the two-arm match.

* **Minor final-spec items** (MCP 2026-07-28, #99).
  * **Deterministic `tools/list` order.** The spec asks servers to list tools in
    a deterministic order (it lets clients cache, and improves LLM prompt-cache
    hit rates). neva's registries were `HashMap`-backed, so the order was
    arbitrary *and* cursor pagination could skip or repeat entries across
    pages; they are `BTreeMap`-backed now, ordered by name.
  * **`x-mcp-header`.** A server may annotate a tool's `inputSchema` property to
    have the argument mirrored into an `Mcp-Param-{name}` header. Servers may
    use it; clients **must** honor it, so the client now records the
    annotations from `tools/list` and attaches the headers on `tools/call`.
    Definitions that break the spec's constraints (non-token name, duplicate,
    non-primitive type, or a property not statically reachable through
    `properties`) cause the *tool* to be dropped from the listing, so one bad
    definition cannot change what a good one sends. Streamable HTTP only --
    other transports may ignore the annotation.
  * **`Mcp-Name` on every method that requires it.** It was sent only for
    `tools/call`; the spec also requires it on `resources/read` (`params.uri`)
    and `prompts/get` (`params.name`). Values that are not safe ASCII -- and
    plain values that would be mistaken for the marker -- now travel Base64
    behind the `=?base64?...?=` sentinel.
  * **`baggage`** joins `traceparent` / `tracestate` as a reserved `_meta` key
    for OpenTelemetry propagation, on `TraceContext` and the passive recorder.
  * **`includeContext`.** `thisServer` / `allServers` (and their builders) are
    `#[deprecated]` in the default build; omit the field or use `none`.
* **Wire conformance pack** (MCP 2026-07-28, #98). Assorted field/method/code
  mismatches against the final schema, all gated so `legacy-spec` is unchanged.
  * **`_meta` key naming.** Per-request client capabilities move from the bare
    `clientCapabilities` key to `io.modelcontextprotocol/clientCapabilities`,
    and the protocol version now also rides in `_meta` as
    `io.modelcontextprotocol/protocolVersion` (it was header-only). A body that
    names a different version than the `MCP-Protocol-Version` header is
    rejected as a header mismatch.
  * **Routing headers are validated against the body.** `Mcp-Method` is
    required on every request and `Mcp-Name` on `tools/call`, `resources/read`
    and `prompts/get`; a `tools/call` must also mirror each `x-mcp-header`
    argument into `Mcp-Param-{name}`. The server now rejects a missing or
    disagreeing header with `HeaderMismatch` (`-32020`) and HTTP `400`,
    decoding the `=?base64?...?=` sentinel before comparing. Without this an
    intermediary routing or rate-limiting on those headers could be bypassed by
    a body naming a different tool. A notification is not required to carry
    `Mcp-Method`, but one that does must state its own method. Routing headers
    on a batch are rejected outright -- no single method or name describes one,
    so a batched call is neither expected to mirror its arguments nor checked
    for having done so.
  * **Required `_meta` fields are enforced.** Both keys above are mandatory on
    every request -- capabilities are declared per request precisely so a
    stateless server never infers them from earlier traffic -- and the HTTP
    server rejects a request missing either with `InvalidParams` (`-32602`) and
    HTTP `400`, per the spec. Requests inside a batch are checked one by one.
    An empty `clientCapabilities` object is a valid declaration, not an
    omission. neva's own client has always sent both. The requirement is on the
    message, not the transport, so `Request::required_meta_error` is public and
    the dispatch seam enforces it for stdio too -- only the `400` is HTTP's.
  * **Error codes.** `HeaderMismatch` (`-32020`),
    `MissingRequiredClientCapability` (`-32021`) and
    `UnsupportedProtocolVersion` (`-32022`) join `ErrorCode` at their
    spec-allocated numbers -- neva defined none of them and answered a version
    mismatch with `InvalidRequest`. These carry the `data` payloads the spec
    defines (`supported`/`requested`, `requiredCapabilities`), so `Error` grew
    `with_data`. All three answer HTTP `400`, as the spec requires.
  * **`CacheableResult`.** `ttlMs` and `cacheScope` are mandatory members
    rather than optional hints, on `DiscoverResult`, `ReadResourceResult` and
    all four list results. `CacheScope` is now `public`/`private` (it was
    `session`/`connection`/`client`), defaulting to `private`. neva always
    emits both; a peer that omits them still parses.
  * **`server/discover` reshaped.** `protocolVersion` becomes
    `supportedVersions: string[]` -- discovery advertises the whole set and the
    client picks -- and `serverInfo` leaves the result entirely. Servers now
    identify themselves in *every* result's `_meta` under
    `io.modelcontextprotocol/serverInfo`; neva stamps it at the dispatch seam
    and the client reads `Client::server_info` from there.
  * **`notifications/roots/list_changed` is removed**, and URL elicitation
    loses its `elicitationId`: with no server-initiated completion signal there
    is nothing to correlate. A server that needs to track an elicitation across
    retries encodes its own identifier in `requestState`.
  * **`ping` is removed**, along with `Client::ping` and `BatchBuilder::ping`.
  * **`notifications/elicitation/complete` is removed**, along with
    `Context::complete_elicitation`, `Client::on_elicitation_completed` and
    `ElicitationCompleteParams`. Answering the input request is the completion
    signal.
* **Tasks method realignment** (MCP 2026-07-28, #96). The final Tasks extension
  ([`modelcontextprotocol/ext-tasks`](https://github.com/modelcontextprotocol/ext-tasks))
  reshapes the whole surface, not just the method names. Under `legacy-spec`
  the 2025-11-25 surface is unchanged.
  * **Methods.** `tasks/list` and `tasks/result` are **removed**. `tasks/get`
    becomes the single polling method and returns a `DetailedTask`: the status
    plus, depending on it, the outstanding `inputRequests`, the terminal
    `result`, or the `error`. `tasks/update` is **new** -- the client answers a
    task's input requests with it, keyed to what `tasks/get` surfaced.
    `tasks/cancel` now acknowledges with an empty result (cancellation is
    cooperative, so the outcome is learned by polling).
  * **`CreateTaskResult` is flat** (`Result & Task`), carrying `resultType:
    "task"` -- the third discriminator value, joining `ResultType`. The task's
    fields sit at the top level instead of under a nested `task` object.
  * **Field renames.** `Task::ttl` serializes as `ttlMs` and `poll_interval` as
    `pollIntervalMs`. `ttl` is now `Option<usize>`, matching the schema's
    nullable "unlimited" case (it was documented as nullable but typed
    non-null in both profiles).
  * `notifications/tasks/status` is now `notifications/tasks`.
  * **Capability.** The extension capability is an empty object: advertising it
    *is* the declaration. The `cancel` / `list` / `requests` sub-tree and its
    builders (`with_cancel`, `with_list`, `with_requests`, `with_tools`,
    `with_elicitation`, `with_all`) are gone, and `with_tasks` takes no
    closure -- `opt.with_tasks()`.
  * `Mcp-Name` now carries `params.taskId` on the task methods, as the spec
    requires for routing a task's calls to the instance holding its state.
  * Client-hosted tasks are legacy-only: they existed to answer server->client
    task-augmented requests, and MCP 2026-07-28 has no server->client requests.
  * Not covered here: `subscriptions/listen`, which is how the spec has clients
    opt into `notifications/tasks`. It lands in 0.5.1, though the task category
    itself is not in the filter yet.
* **`resultType` on every result** (MCP 2026-07-28, #97). The final spec makes
  the discriminator mandatory on results, not just on MRTR continuations. neva
  emitted it only on `InputRequiredResult`; now every success result carries
  `resultType: "complete"` -- tools, prompts, resources, discover, completion,
  tasks and anything a custom handler returns.
  * Stamped centrally in `Response::success`, so it covers every `IntoResponse`
    impl including `Json<T>` and the scalar ones. An existing discriminator is
    never overwritten, which is how `input_required` survives the same funnel.
    A non-object result has nowhere to put the field and is passed through.
  * New `types::ResultType` (`Complete` / `InputRequired`) and
    `Response::result_type()`, which applies the spec's compatibility rule:
    an **absent** field reads as `Complete`, and so does any value neva does
    not recognize. The client's MRTR detection now goes through it instead of
    matching the raw JSON in two places.

### Added
* **Request-scoped logging** (MCP 2026-07-28, #93). The 2026-07-28 spec
  removes only `logging/setLevel`; it keeps `notifications/message` as a
  deprecated, request-scoped log notification. neva had compiled the whole
  logging surface out -- this brings the kept part back:
  * The desired level rides per-request on
    `_meta["io.modelcontextprotocol/logLevel"]` (`RequestParamsMeta`) instead of
    a global `setLevel` handshake. While the server handles a request, it emits
    `notifications/message` at or above that severity and suppresses the rest;
    with no requested level it emits none.
  * Both emission paths honor the level: the HTTP `MpscLayer`
    (`notification::fmt::layer()`) and the stdio `NotificationFormatter`. Every
    supported stdio setup works unchanged -- the formatter resolves the
    request-scoped level on its own, including from a formatter-only subscriber.
    A new `notification::fmt::span_context()` layer can be added alongside to
    resolve it from a typed span extension instead.
  * Client API: `McpOptions::with_log_level` (via `Client::with_options`),
    `#[deprecated]` on arrival to mirror the schema. `LoggingLevel`/`LogMessage`
    stay undecorated.
  * `logging/setLevel` and `with_logging`/`set_log_level` stay removed in the
    default build.
  * Delivery: request-scoped notifications flow on the originating request's
    response stream, per the spec. Over **stdio** they interleave on stdout.
    Over the stateless **HTTP** transport, a `POST` that opts in (carries
    `io.modelcontextprotocol/logLevel` or a `progressToken` in `_meta`) gets a
    `text/event-stream` reply carrying its `notifications/message` /
    `notifications/progress` followed by the response; other `POST`s stay a
    single JSON object. The layer routes each request's notifications to a
    per-`POST` sink (keyed by the per-`POST` session id); the client parses the
    SSE reply and dispatches notifications before resolving the request. The
    suppression rule ("no `logLevel` => no `notifications/message`") holds on
    every transport.

## 0.4.3

### Added
* **MRTR input-request kinds: sampling + roots** (`proto-2026-07-28-rc`, #85).
  The spec did not delete sampling and roots -- it removed them as
  capability-driven server->client *requests* and re-homed the ability onto
  MRTR, as input-request kinds. They return here the same way, and -- matching
  the spec's own 12-month lifecycle -- **already deprecated**:
  * `ctx.sample(key, params)` and `ctx.list_roots(key)` join `ctx.elicit` on
    the MRTR substrate, with identical re-run/replay semantics. `once` /
    `memo` / `on_commit` cover them for free -- one substrate, three kinds.
  * `CreateMessageRequestParams`/`Result` and `ListRootsResult`/`Root` are
    available under the RC again, now as input-request params/results. The
    server-push `SamplingHandler` channel stays gone: the client fulfils
    sampling from its `map_sampling` handler and roots from its configured
    list, both on the MRTR loop.
  * `ClientMrtrCapabilities` grows `sampling` and `roots` flags; the server
    gates each kind on its own flag and reports a request for an undeclared
    kind instead of stalling the round-trip. The flags are additive, so a peer
    that only sends `elicitation` still decodes.
  * Both new server APIs, the new `InputRequest::Sampling`/`Roots` variants and
    the capability flags carry `#[deprecated]`. Elicitation stays first-class.

  Existing deprecation notes on `Client::map_sampling`, `add_root(s)`,
  `McpOptions::with_roots`/`with_sampling` were reworded: they described the
  ability as *removed* in 2026-07-28, which is no longer accurate.
* RC variants of the roots and sampling examples, alongside the legacy ones:
  `examples/roots/rc/{server,client}` and
  `examples/sampling/rc/{server,client}`. Each `rc/` directory is its own
  workspace -- Cargo unifies features across members built together, so keeping
  the RC crates in the legacy workspace would switch `proto-2026-07-28-rc` on
  for the legacy crates and stop them compiling.

* `neva::shared::BoxFuture` (also in the prelude) -- the return type of
  neva's object-safe async traits, now owned by neva instead of borrowed
  from `futures_util`. Implementing
  [`AuthorizationHandler`](https://docs.rs/neva/latest/neva/auth/oauth/trait.AuthorizationHandler.html)
  or [`RequestStateStore`](https://docs.rs/neva/latest/neva/trait.RequestStateStore.html)
  no longer requires a `futures` dependency of your own, kept in lockstep
  with neva's. It is a plain alias for
  `Pin<Box<dyn Future<Output = T> + Send + 'a>>` -- the same type
  `futures_util::future::BoxFuture` denotes -- so existing implementations
  that spell out the `futures_util` path keep compiling unchanged.

### Documentation
* Documented why MRTR **seals** `requestState` with ChaCha20-Poly1305 instead
  of signing it (#82): a signed state is tamper-evident but *readable*, which
  stops being enough once `ctx.memo` writes server-computed values -- an
  upstream response, a quoted price, a downstream token -- into the state for
  the next round to replay. The AEAD tag authenticates exactly as an HMAC
  would, so confidentiality costs nothing. The consequence for callers is
  spelled out at [`types::mrtr`](https://docs.rs/neva/latest/neva/types/mrtr/)
  and at `App::with_request_state_secret`: the secret upholds confidentiality,
  not just integrity.
* Documented the MRTR idempotency story as a whole (#82): a re-run/replay
  handler executes from the top every round, and the protocol leaves the
  resulting side-effect problem to the implementation. `ctx.memo` /
  `ctx.once` / `ctx.on_commit` plus the default `RequestStateStore`
  (final-round replay protection for a lost HTTP response) mean a tool that
  charges a card can be written in the obvious way.

### Changed
* **Breaking (`proto-2026-07-28-rc` API, #85)** -- generalizing the MRTR input
  request reshapes three public items. The *wire* format is unchanged: an
  envelope is still `{ method, params }` and `method` is still the
  discriminator, so peers interoperate across the change.
  * `mrtr::ElicitationInputRequest` (and its `ElicitationCreateMethod` tag) are
    replaced by the `mrtr::InputRequest` union. Migration:
    `ElicitationInputRequest { params, .. }` -> `InputRequest::Elicitation(params)`.
  * `mrtr::InputResponses` is now `HashMap<String, serde_json::Value>` rather
    than `HashMap<String, ElicitResult>` -- the result type depends on the kind
    that was requested. Deserialize your own type out of the value.
  * `mrtr::ClientMrtrCapabilities` gained two fields, so struct-literal
    construction needs `..Default::default()`.
* Updated to `volga` / `volga-oauth-core` / `volga-oauth-client` 0.9.6,
  which ships the two upstream fixes this crate was working around:
  * The token-endpoint futures (`exchange_code` / `refresh` / `token`) are
    now `Send`, so the internal `spawn_blocking` bridge that ran them on a
    dedicated current-thread runtime is gone -- the OAuth code exchange and
    token refresh run inline on the caller's runtime, with no extra thread
    or runtime per operation. A `Send` bound assertion now guards the
    regression.
  * `application_type` is a first-class member of the registration
    document, so the loopback (native client) declaration no longer travels
    as an extension field. The wire shape is unchanged.
* Dependency updates: `serde` 1.0.229, `serde_json` 1.0.151, `tokio`
  1.53.1, `tokio-util` 0.7.19, `futures-util` 0.3.33, `jsonschema` 0.48.2,
  and `syn` 3.0 / `quote` 1.0.47 / `proc-macro2` 1.0.107 in `neva_macros`.

### Fixed
* A managed OAuth session could permanently lose its ability to refresh
  non-interactively: the cached client/metadata were taken out of the
  single-flight slot for the duration of a refresh and were not restored
  if the (now removed) bridge itself failed, so every later request fell
  back to interactive authorization. The cached state is now borrowed
  rather than moved.

## 0.4.2

### Added
* **Dual-mode client (`initialize` fallback).** Under
  `proto-2026-07-28-rc` the client now carries both handshakes: it tries
  `server/discover` and, when the server clearly doesn't speak the RC
  protocol (`MethodNotFound`, `InvalidRequest`, or a non-JSON-RPC /
  unknown-code reply), falls back to the legacy `initialize`/`initialized`
  handshake -- negotiating the newest pre-RC version (override with
  `with_mcp_version`) -- and speaks legacy for that peer at runtime:
  `Mcp-Session-Id` header, the standalone SSE GET stream, legacy
  server-push sampling/roots/logging, no MRTR and no RC routing headers.
  Network-level failures do not trigger the fallback. The switch is
  per-connection, monotonic, and decided before any other traffic; the
  server side remains compile-time pure. The legacy client machinery now
  compiles (dormant) under the RC flag for this -- the legacy build is
  unchanged. (#84)
* Client-side OAuth 2.1 authorization behind the new `client-oauth`
  feature (included in `client-full`), built on `volga-oauth-client`:
  * `HttpClient::with_oauth(...)` enables the automatic flow: a `401`
    challenge drives RFC 9728/8414 discovery (OIDC fallback), dynamic
    client registration when no `client_id` is configured (RFC 7591,
    `application_type: "native"` for loopback redirects), and the
    authorization-code + PKCE flow with the server's canonical URI as
    the RFC 8707 resource indicator; the failed request is retried once
    with the fresh token, concurrent `401`s share a single flow.
  * The callback is validated for `state` and the RFC 9207 `iss`
    parameter (required when the server advertises support) before the
    code exchange -- mix-up-attack responses abort the flow.
  * The interactive step is pluggable via `AuthorizationHandler`; the
    default `LoopbackHandler` opens the system browser and captures the
    redirect on a loopback listener. Tokens persist through the
    re-exported `TokenStore` abstraction (`InMemoryTokenStore` default).
  * Token lifecycle: an access token about to expire (30s leeway) is
    refreshed proactively before the next request, and a `401` tries the
    refresh-token grant before falling back to interactive
    authorization -- refresh-token rotation and dead-entry pruning
    included. Both paths are non-interactive and single-flight.
  * Everything exported under `neva::auth::oauth`.
* OAuth examples: `examples/oauth-server` (resource server with explicit
  RFC 9728 metadata), `examples/oauth-client` (fully automatic flow),
  `examples/oauth-with-keycloak` (end-to-end walkthrough with a
  ready-to-import realm), and `examples/oauth-hyper-engine` -- a custom
  `HttpEngine` on bare hyper serving the well-known document, the
  `WWW-Authenticate` challenge and per-tool role gates through the
  engine-neutral primitives, without Volga.
* Engine-neutral OAuth 2.1 resource-server primitives behind the new
  `server-oauth` feature (included in `server-full`), built on
  `volga-oauth-core` -- protocol types only, no Volga framework dependency,
  so they work with any `HttpEngine`:
  * `HttpServer::with_oauth_metadata(...)` configures the RFC 9728
    Protected Resource Metadata document (`OAuthResourceOptions`:
    authorization servers, scopes, canonical resource override for
    reverse proxies, full-document escape hatch). The document is
    canonicalized (RFC 8707), pre-serialized once at server start, and
    exposed to engines via `HttpContext::oauth_metadata_path()` /
    `oauth_metadata_url()`.
  * `handlers::handle_oauth_metadata` serves the well-known document;
    `handlers::handle_unauthorized` answers 401 with the
    `WWW-Authenticate: Bearer resource_metadata="..."` challenge.
  * The default Volga engine mounts the document on its well-known path
    automatically (publicly reachable -- auth enforcement is scoped to the
    MCP endpoint group) and, when bearer auth is configured, advertises it
    as `resource_metadata` on Volga's own 401 challenges.
  * Protocol types re-exported under `neva::auth::oauth`
    (`ProtectedResourceMetadata`, `BearerChallenge`, `OAuthError`, ...).
* **OAuth 2.1/OIDC issuer mode for the default Volga engine** (#69):
  `with_auth(|auth| auth.with_oauth(|oauth| oauth.with_issuer(...)))` replaces
  the static decoding key with issuer-discovered JWKS validation (RFC 8414
  discovery with OIDC fallback, key rotation, refresh cooldown / max key age
  via Volga). MCP defaults applied unless overridden: the token's `aud` must
  contain the server's canonical resource URI (RFC 8707 -- `aud` becomes
  required) and its `iss` must match the configured issuer. The Protected
  Resource Metadata document is derived from the issuer automatically when
  `with_oauth_metadata` was not called (#68), so discovery, challenge and
  validation work out of the box with a single builder call. New
  `AuthConfig::with_resource`/`with_resources` (RFC 8707 resource
  indicators) for overriding the audience explicitly.
* **MRTR `requestState` key rotation.** New `App::with_request_state_keys(active_kid, keys)`
  configures a keyring: new blobs are sealed under the active key id, inbound
  blobs decrypt with whichever accepted key their kid names -- enabling
  zero-downtime rotation. `with_request_state_secret` remains the single-key
  shorthand (kid `"0"`). (#81)

### Fixed
* A client whose transport died (e.g. the OAuth flow failed against an
  unreachable issuer) sat out the full request timeout -- and `Ctrl+C`
  appeared ignored, since the shutdown handler only cancels the
  transport token. Pending request awaits now race that token and abort
  with `Connection closed` the moment it fires, so both transport death
  and shutdown signals interrupt `connect()`/requests immediately.
* Client-only feature sets (e.g. `http-client` alone) failed to build:
  the client's notification handler uses `tokio::task::block_in_place`,
  but nothing enabled `tokio/rt-multi-thread` -- now the `client` feature
  does.
* The legacy `initialize` result no longer advertises the `logging` capability
  in builds without the `tracing` feature, where the `logging/setLevel`
  handler is not registered -- capability-trusting clients (e.g. newer MCP
  Inspector) would call it and hit `Method not found`.
* Per-tool/prompt/resource role and permission gates now receive the claims
  Volga's `authorize` middleware already validated (`Authenticated<...>` from
  the request) instead of re-decoding the `Authorization` header. With
  Volga's default `strip_token_from_request = true` the header is removed
  before the route runs, so the old re-decode lost the claims and protected
  tools rejected valid tokens.

### Changed
* **Breaking (RC wire format):** the sealed MRTR `requestState` blob is now
  `v1.{kid}.b64(nonce).b64(ciphertext+tag)` (previously
  `b64(nonce).b64(ciphertext+tag)`). The `v1.{kid}` header is bound into the
  AEAD associated data, so neither segment can be transplanted; decode rejects
  unknown versions and key ids with `InvalidParams`. In-flight states minted
  by an older release fail verification (TTL is 300s, so exposure is
  transient). (#81)

### Security
* Resolved RUSTSEC-2026-0190 (unsoundness in `anyhow::Error::downcast_mut`)
  by updating the transitive `anyhow` dependency to 1.0.103.

## 0.4.1

### Added
* Added cargo audit mandatory CI step

### Security
* Resolved RUSTSEC-2026-0185 vulnerability
* Resolved RUSTSEC-2023-0071 vulnerability

## 0.4.0

This release adds opt-in support for the **MCP 2026-07-28 Release Candidate**
spec behind the compile-time `proto-2026-07-28-rc` flag. The legacy spec
remains the default and is unchanged for users who don't opt in. Once the RC
graduates the flag will invert: the RC path becomes the default and the
current default moves under a `legacy-spec` flag -- a deliberate breaking
change, mirroring the spec itself.

### Added

#### MCP 2026-07-28 RC (opt-in via `proto-2026-07-28-rc`)

* **Stateless HTTP transport.** The `initialize`/`initialized` handshake is
  replaced by a single `server/discover` request returning `DiscoverResult`
  (with `Client::discover()`; `Client::init()` kept as a back-compat alias).
  No `Mcp-Session-Id` on the wire; the GET (SSE) and DELETE routes are not
  registered. Every POST carries a required `MCP-Protocol-Version` header --
  the client injects automatically; the server rejects missing/unsupported
  values with `InvalidRequest`.
* **JSON Schema 2020-12 for tools.** New `schema_2020::InputSchema` --
  `#[serde(transparent)]` newtype over `serde_json::Value` -- and a per-flag
  `ToolInputSchema` alias on `Tool.input_schema`/`output_schema`. The
  `#[tool]` macro now emits full 2020-12 documents: primitive args become
  inline primitive schemas, structured `Json<T>` args derive a rich inlined
  schema when `T: JsonSchema` (graceful `{"type":"object"}` fallback
  otherwise), and the return type drives `outputSchema`.
  `input_schema`/`output_schema` string literals are validated at compile
  time on every feature configuration. `schemars` is re-exported by neva --
  user crates don't need a direct dep.
* **Multi Round-Trip Requests (MRTR) for elicitation.** Handlers call
  `ctx.elicit(key, params).await?`; on a miss the framework returns an
  `InputRequiredResult` carrying an AEAD-sealed `requestState`, and the
  client re-issues with `inputResponses` until completion. State is bound to
  a TTL, the originating request, and the authenticated principal
  (`Claims::subject`). Configure the secret via
  `App::with_request_state_secret`; an ephemeral key is generated otherwise
  (multi-instance deployments **must** set a shared one).
* **Replay-aware effect helpers** on `Context`: `ctx.once(key, fut)`
  (run-at-most-once side effect), `ctx.memo(key, fut)` (computed-and-cached
  value, written into the sealed `requestState`), `ctx.on_commit(fut)` (runs
  exactly once when the handler reaches its final result). At-most-once
  *within a single chain* -- pair non-idempotent effects with a downstream
  idempotency key.
* **MRTR final-round idempotency store.** `RequestStateStore` trait
  (`neva::app::mrtr_store`) with a default per-process `InMemoryStateStore`,
  wired via `App::with_request_state_store`. Caches the final response keyed
  by the incoming state's integrity tag + answers digest, so a lost-response
  retry returns the cached result instead of re-running the handler. Implement
  over a shared backend (e.g. Redis) for multi-instance -- same constraint as
  a shared signing secret.
* **Task-augmented elicit.** Two execution substrates that never mix: a bare
  call uses MRTR re-run (`ctx.elicit(key, params)`); a task-augmented call
  genuinely suspends (`ctx.task().elicit(params)`, no replay key). New
  `ctx.task()` builder and `ctx.is_task()` switch for `TaskSupport::Optional`
  tools that elicit on both substrates. Resuming a parked task-elicit requires
  the answer to reach the instance running the task -- same instance-affinity
  tasks already have.
* **Protocol extensions framework.** `Extension` trait
  (`neva::app::extension`) registered via `App::with_extension`; extensions
  advertise a capability under their reverse-DNS id (surfaced in
  `server/discover` under `capabilities.extensions`) and register their own
  handlers. **Tasks** is the first built-in consumer (`TasksExtension`, id
  `io.modelcontextprotocol/tasks`).
* **Cache hints.** `CacheScope` enum and `ttlMs`/`cacheScope` fields on the
  four list results (tools, prompts, resources, resource templates).
* **Routing & tracing.** RC-only `Mcp-Method`/`Mcp-Name` routing headers on
  HTTP POSTs; `traceparent`/`tracestate` on `RequestParamsMeta` with a
  client-side `TraceContextProvider` hook and matching server-side span
  recording.
* **Configuration knobs.** `App::with_max_state_bytes` (default 8 KiB) caps
  encoded `requestState`; client `McpOptions::with_max_mrtr_rounds` (default
    8) caps MRTR re-issue rounds -- counted as retries, the initial send is
       always made on top.
* **Startup deployment warn.** When `http-server` + `tracing` are enabled and
  `with_request_state_secret` was never called, `App::run` logs at
  `tracing::warn!` so multi-instance deployments don't silently fail to
  decrypt `requestState` on cross-instance retries.

#### Spec-neutral

* `ErrorCode::RESOURCE_NOT_FOUND` -- helper constant emitting `-32002` under
  the legacy spec and `-32602` (`InvalidParams`) under RC. In-tree emitters
  route through it; recommended migration path off
  `ErrorCode::ResourceNotFound`.
* `ToolSchema::from_value`, `from_schema::<T>`, `from_schemars` -- legacy-side
  constructors symmetric with `InputSchema`.

### Changed

* `Tool.input_schema`/`output_schema` use the per-flag `ToolInputSchema`
  alias. `Tool::validate` extracts the schema as `serde_json::Value` before
  invoking the validator, so the same validator path covers both flavours.
* `PROTOCOL_VERSIONS` advertises `"2026-07-28"` only when the RC flag is
  enabled.
* CI matrix extended with `proto-2026-07-28-rc` x `server-full,client-full`
  under clippy, doc, and test jobs.

#### Under `proto-2026-07-28-rc` only

* **`Context::elicit`** now takes a stable `key` argument and follows MRTR
  re-run/replay. Handlers must be side-effect-free up to each elicit point
  (use `once`/`memo`/`on_commit` for effects). `Claims` gains an additive
  `subject()` accessor (default `None`) for principal-binding.
* **Tasks capability** is advertised under
  `capabilities.extensions["io.modelcontextprotocol/tasks"]` instead of the
  top-level `capabilities.tasks`. The `with_tasks` API and `tasks/*` wire
  methods are unchanged -- `with_tasks` now thinly wraps the extension
  registration. Legacy builds keep the top-level field.
* **HTTP transport is request/response only.** Server-initiated notifications
  (progress, resource-updated, list-changed, task-status, elicitation) are
  inert; `Context` notification helpers become no-ops. Clients poll
  (`tools/list`, `resources/read`, `server/discover`) instead.
* **Client request `_meta`** carries implementation info under
  `io.modelcontextprotocol/clientInfo`, merged non-destructively with any
  existing `_meta`.

### Removed under `proto-2026-07-28-rc`

* `roots/list`, `notifications/roots/list_changed`, `Root`/`Roots` types.
* `sampling/createMessage`, the `SamplingHandler`/`SamplingTaskCapability`
  types, and the `sampling!` macro re-export.
* `logging/setLevel`, `notifications/message`,
  `LoggingLevel`/`LogMessage`/`SetLevelRequestParams`, and
  `NotificationFormatter`.
* The typed `ToolSchema` (with `from_json_str` / `with_required`) -- replaced
  by `InputSchema`.
* `McpOptions::with_mcp_version` on both server and client builders -- the RC
  build is a pure 2026-07-28 peer, so the version is fixed. Version selection
  returns under the legacy flag once the RC graduates.

### Deprecated

* `ErrorCode::ResourceNotFound` -- use `ErrorCode::RESOURCE_NOT_FOUND` for
  per-flag wire mapping.
* `Client::add_root`, `add_roots`, `publish_roots_changed`, `map_sampling`,
  and `McpOptions::with_logging` -- removed in MCP 2026-07-28. Available under
  the legacy spec; cfg-gated out under RC.
* `ToolSchema::from_schema(schemars::Schema)` -- renamed to `from_schemars`
  for symmetry. Previous name kept as `from_schema_legacy` for a transition
  window.

### Security

* MRTR (RC): the server no longer accepts unbound `_meta.inputResponses`. The
  signed `requestState` records the requested key(s), and answers are
  accepted only when paired with a valid state and only for solicited,
  not-yet-resolved keys; anything else is `InvalidParams`. Previously a
  client could pre-seed or overwrite answers for a future `ctx.elicit` key
  and skip the intended `InputRequiredResult`.

### Fixed

* `Dc<T>` dependency-injection extractors now work as handler arguments for
  tools and prompts (previously resources only). They were classified as
  unknown `"object"` types and failed the `TypeCategory` bound; now treated
  like `Context`/`Meta<_>` -- injected from request context, never listed as
  an argument.

### Known limitations

* `#[tool]`'s `annotations = "..."` (and `#[prompt]`/`#[resource]` JSON-string
  attributes) still parse at runtime and panic on malformed JSON; compile-time
  validation there is a planned follow-up. `input_schema`/`output_schema`
  literals are already validated at compile time.
* `cargo check --no-default-features --features client` (without
  `--all-targets`) fails on `tokio::task::block_in_place` because the
  `client` feature alone doesn't pull `tokio/rt-multi-thread`. Pre-existing
  tokio-features issue. CI runs the `--all-targets` variant (pulls dev-deps
  -> `rt-multi-thread`) and remains green.

## 0.3.4

### Changed
* `HttpEngine::adapt_request` no longer forces engines to `.unwrap()`/`.expect()`
* `HttpEngine::adapt_response` drops the `BytesMut` round-trip for the default Streamable HTTP implementation.
* `parse_message` single-step decode + `Error::classify()`. Drops the `serde::Value` intermediate from the single-message hot path.
* Removed `'static` constraint for `HttpEngine::Request`, `HttpEngine::Response` and `HttpEngine::SseEvent` 

## 0.3.3

### Added
* **Pluggable HTTP server.** Introduced the `HttpEngine` trait so non-Volga HTTP stacks (axum, hyper, custom adapters) can plug into neva's Streamable HTTP transport. The engine declares its native request/response/SSE-event types and supplies four bridge methods plus a `run` loop; everything else (JSON-RPC framing, SSE replay & dedup, batch fast-path, pending-oneshot routing) stays in neva.
* New feature flags: `http-server` (engine-agnostic abstractions, no framework dependency) and `http-server-volga` (default Volga adapter). `server-full` enables the Volga variant for backwards compatibility.
* `dispatch_post` / `dispatch_delete` / `dispatch_get_sse` engine-generic route helpers -- adapter handlers collapse to one-liners.
* Reference engine adapters under `examples/`: `axum` (Send-friendly types, the canonical pattern), `hyper` (raw protocol layer, no router), and `actix-web` (handles actix's `!Send` request/response types and its dedicated-runtime requirements).
* `neva::auth::Claims` is now neva's engine-neutral typed-claims contract. Any HTTP engine adapter can wrap its own decoded claims in `Arc<dyn Claims>` and insert them into request extensions to enable `with_roles` / `with_permissions` gating across tools, prompts, and resources.
* CI `doc_check` job gating on `cargo doc --no-deps --all-features` with `RUSTDOCFLAGS="-D warnings"`.

### Fixed
* Broken intra-doc links and malformed code-block examples (`#[resource(...]` / `#[prompt(...]`) flagged by rustdoc.
* `cargo doc --no-deps --all-features` is now warning-free and enforced in CI.

## 0.3.2

### Added
* Lazy cleanup for expired tasks

### Changed
* JSON RPC batches are now processing in parallel
* Improved pagination for `list/tools`, `list/prompts` and `list/resources` commands

### Fixed
* Removed unnecessary heap allocation for the middleware pipeline
* Request timeout and cleanup

## 0.3.1

### Added
* SSE backpressure configuration.
* Graceful session cleanup and sweeper for stale sessions

## 0.3.0

### Added
* Improved MCP Client DX for calling task-enabled tools.
* Added `wire_code()` method that returns a safe JSON-RPC 2.0 supported code. 

## 0.2.9

### Added
* SSE `Last-Event-ID` replay

### Fixed
* Fixed a bug when optional `params` field in `Request` was expected as required.

## 0.2.8

### Fixed
* Fixed JSON-RPC 2.0 protocol violation: server no longer sends a response to client notifications (section 4 -- notifications must never be replied to)
* Fixed `notifications/cancelled`: request cancellation now actually fires for both stdio and Streamable HTTP transports
* Fixed Streamable HTTP transport silently dropping notifications without processing them

## 0.2.7

### Added
* JSON-RPC Batch Support for client and server

### Fixed
* Fixed broken Streamable HTTP server implementation
