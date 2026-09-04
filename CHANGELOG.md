# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## 0.5.7

### Fixed

#### Build
* **The crate compiles on Windows again** (#127). `unsafe_code` is forbidden
  workspace-wide, and `transport/stdio/windows.rs` -- the Win32 job-object
  backend behind the stdio client -- is built entirely out of raw API calls.
  The two met when 0.2.1 added `[lints] workspace = true` to `neva/Cargo.toml`:
  before that the crate did not inherit the workspace lint table, so the
  `forbid` was declared but never applied. Every release since has failed to
  build on Windows under any feature set that enables `client`, with
  `implementation of an unsafe trait` and `usage of an unsafe block`.

  The lint is now `deny` workspace-wide, with a module-scoped `expect` on the
  one module that needs it. `get_main_thread_id` also gained the explicit
  `unsafe` block that edition 2024 wants inside an `unsafe fn`, which would
  otherwise have failed the `-D warnings` gate.

  CI gained a build matrix over Linux, macOS and Windows. `handshake` picks its
  spawn backend by target, and only the Linux one was ever compiled: the Win32
  backend not at all, and the plain `Command::spawn` fallback that every other
  target uses only by whoever happened to build locally.

#### Transport
* **A stdio client returns an error instead of panicking when the MCP server
  cannot be spawned** (#125). `StdIoClient::handshake` unwrapped the spawn with
  `.expect(..)`, so a typo in the command, a binary that was never built, or a
  server removed from `PATH` -- ordinary user input, not a bug -- took the
  process down inside the library. `Transport::start` is now fallible, `connect`
  propagates with `?`, and the error names the command that failed rather than
  reporting a bare `No such file or directory (os error 2)` that does not say
  which server it was.

  Not covered on Windows. `windows::Job::new` rewrites the command to
  `cmd /c <command>` unless it already contains `"cmd"`, so the spawn itself
  always succeeds there and a missing binary instead surfaces as a non-zero
  child exit after the handshake has already returned. The regression test is
  gated off Windows for that reason.

## 0.5.6

### Added

#### MCP Apps
* **The MCP Apps extension (`io.modelcontextprotocol/ui`), behind the new
  `apps` feature** (#87). A tool points at a `ui://` HTML resource the host
  renders in a sandboxed iframe
  ([SEP-1865](https://github.com/modelcontextprotocol/ext-apps)). neva
  implements the data plane -- the `_meta.ui` blocks on tools and resources; the
  `ui/*` host-to-iframe messages are out of scope.

  Server: `McpOptions::with_apps()`, `App::add_ui_resource` and
  `App::map_ui_resource`, `Tool::with_ui` / `with_visibility`, `AppsExtension`.
  A `ui://` read is always served as `text/html;profile=mcp-app` and carries its
  template's `_meta.ui`; it stays out of `resources/list` unless
  `AppsExtension::with_listed_resources()`. Startup warns about a tool naming a
  `ui://` resource nothing serves.

  Macros: `#[tool(ui = "ui://...", visibility = ["app"])]` and
  `#[resource(ui_meta = "...")]`. Bad scheme, unknown visibility scope, wrong
  MIME type and misspelled `ui_meta` keys are compile errors.

  Client: `McpOptions::with_apps()` / `with_app_mime_types(..)` advertise the
  extension on `initialize`; `Tool::ui()` (which also reads the deprecated flat
  `_meta["ui/resourceUri"]`), `Tool::is_model_visible()` and
  `ResourceContents::ui()` read it back.

  Not advertised to a server speaking MCP 2026-07-28, which has no handshake
  (#122); `add_ui_resource` carries no role or permission requirement (#123).
  See `examples/apps`.

### Changed

* **`#[tool]`, `#[resource]`, `#[resources]`, `#[prompt]` and `#[handler]`
  reject an unknown attribute** instead of ignoring it -- a misspelled
  `visibility` published an app-only tool to the agent.

* **`ResourceContents`'s accessors are available to a client build**: `uri`,
  `text`, `blob`, `json`, `mime`, `title`, `annotations`. The builders stay
  server-side.

* **`ClientCapabilities::extensions` is no longer gated on the protocol
  generation**, so a legacy `initialize` can carry it. Additive; its counterpart
  on `ServerCapabilities` stays 2026-07-28-only.

## 0.5.5

### Changed (breaking)

#### Authorization
* **`AuthorizationHandler` is written with plain `async fn`s.** Both of its
  methods returned `neva::shared::BoxFuture`, so every implementation of the
  interactive OAuth step opened with `Box::pin(async move { ... })` -- a fact
  about how the configuration keeps the handler (behind `Arc<dyn ..>`, which a
  method returning `impl Future` cannot be made into), not about the seam an
  embedder implements. The boxing moved to an internal `dyn` bridge, the way
  `AssertionProvider` and `NotificationBus` already worked.

  To migrate, drop the wrapper: `fn redirect_uri(&self) -> BoxFuture<'_,
  Result<String, Error>>` becomes `async fn redirect_uri(&self) ->
  Result<String, Error>`, and the same for `authorize`; the futures still have
  to be `Send`, which an `async fn` holding nothing thread-bound across an
  `.await` already satisfies. Users of the default `LoopbackHandler` have
  nothing to change.

#### MRTR
* **`RequestStateStore` is written with plain `async fn`s**, the same change on
  the same reasoning: `get`, `put` and the `reserve` that serialises identical
  final-round retries all returned `BoxFuture`. So `fn get<'a>(&'a self, tag:
  &'a str) -> BoxFuture<'a, Option<Response>>` becomes `async fn get(&self,
  tag: &str) -> Option<Response>`, and likewise for the other two; `reserve`
  keeps its no-op default. Users of the default `InMemoryStateStore` have
  nothing to change.

  With both traits converted, `neva::shared::BoxFuture` is no longer part of
  any trait neva asks you to implement. It stays public -- the middleware
  pipeline's `Next` is a `dyn Fn` returning one -- and remains a plain alias
  for `Pin<Box<dyn Future<Output = T> + Send + 'a>>`.

#### Streamable HTTP
* **`HttpEngine::tracked_event` takes an `EventId` instead of a `u64`** (#108).
  An SSE event id is now a cursor within one stream rather than within a
  session, so it names the stream as well as the position -- see the fix below
  for why. Engines write it out as they wrote the sequence number:
  `id.to_string()` renders `<stream>:<seq>`. `EventId` is re-exported from
  `neva::prelude`, with `stream()` / `seq()` for an engine that wants the
  halves; the migration is the signature and nothing else.

### Fixed

#### Shutdown
* **`App::run` no longer returns while the transport writers are still
  draining** (#116). The shutdown drain that landed in 0.5.4 gives a
  subscription's graceful-close result room to be produced and queued; this is
  the last leg, getting it written. Cancelling the transport token did two
  things at once: the writers began draining what was queued, and `run`'s own
  loop broke on that same signal and returned. Nothing joined the first to the
  second, and the drain runs in detached tasks -- so under `App::run_blocking`,
  which drops its runtime the moment `run` returns, a writer that had not
  finished was aborted mid-drain and the client saw the abrupt close the drain
  exists to prevent.

  A transport now hands back a completion signal alongside its cancellation
  token: everything that may still write holds it until it is done, and `run`
  waits for that before returning. `App::with_shutdown_drain(..)` bounds the
  whole teardown rather than each phase of it -- the wait for the subscriptions
  to answer and the wait for the writers share one budget. What is still
  writing when it runs out is stopped rather than left on a runtime that
  outlives the server, so `Duration::ZERO` is the abrupt close it says it is. A
  server with nothing queued pays nothing for any of it, and a transport that
  failed under the server has its writers stopped rather than left behind.

* **The Volga engine stops on the transport's token** (#116). Its `run` took
  the token and used it only to report its own failures, so the listener came
  down on Volga's own signal handling and nothing else: a server stopped
  through an `App::with_shutdown()` handle returned from `run` with the port
  still bound and serving, until whatever owned the runtime dropped it. The
  token now drives Volga's graceful shutdown, which is also what makes the
  drain signal above mean anything for HTTP. `HttpEngine::run` always
  documented this contract -- an engine that ignores the token now costs its
  server the shutdown budget on every stop.

#### Streamable HTTP
* **A session hosts as many SSE streams as the client opens** (#108, legacy
  profile: MCP 2026-07-28 has neither sessions nor the standalone `GET`
  stream). The spec lets a client "remain connected to multiple SSE streams
  simultaneously"; neva held one sender per session, so a second `GET`
  overwrote the first, and the displaced stream ended on a bare EOF with
  nothing to tell it apart from the server closing it on purpose.

  A session now holds a map of streams, each with its own sender, cursor and
  replay buffer, and every event id names the stream it belongs to
  (`<stream>:<seq>`) -- ids "assigned by servers on a per-stream basis", as the
  spec asks. That is what makes the rest enforceable: a `Last-Event-ID` `GET`
  resumes the stream that id names and is replayed that stream's backlog alone,
  never "messages that would have been delivered on a different stream"; an id
  naming a stream the session does not hold is answered `404` rather than
  served from whatever stream is at hand.

  A second concurrent `GET` is accepted as a second stream and the first is
  left open. Server-initiated traffic (log notifications included) rides one
  stream -- the spec's MUST NOT -- and follows the newest live one, which is
  also what keeps a plain reconnect working when the client's connection died
  before the server noticed. With nothing live the role stays put, so the
  ordinary reconnect takes that stream back and is replayed what it missed. A
  session is capped at eight streams, spending the cap on live ones (a
  disconnected stream is dropped to make room before a `GET` is refused
  with `429`).

  An id in the old per-session shape (`<seq>`, no stream) is still read as the
  standalone stream's cursor while the session has only that one, so a client
  reconnecting across a server upgrade resumes rather than starts over. neva's
  own client is unaffected either way: it echoes back whatever id it was handed.

## 0.5.4

### Added

#### Authorization
* **DPoP sender-constrained tokens**, behind the new **`client-oauth-dpop`**
  feature (#110). A bearer token is a password: whoever steals it may spend it.
  A DPoP-bound one ([RFC 9449](https://www.rfc-editor.org/rfc/rfc9449)) is
  worth nothing without the key, because every request carries a proof signed
  over its own method and URL and over the token itself. Both nonce rounds are
  answered -- the token endpoint's (section 8) and the resource's (section 9),
  the second costing one repeat of the request rather than a re-authorization,
  since the token and the key were never in question.

  `OAuthClientConfig::with_dpop(key)` binds every token this client obtains to
  a key of the caller's choosing -- `Dpop::generate()` for a throwaway one per
  session, `Dpop::from_pem` for a lasting one -- and refuses an authorization
  server that answers with an unbound token. `with_dpop_auto()` mints an
  `ES256` key the first time a server asks, by challenging with the `DPoP`
  scheme or by advertising `dpop_signing_alg_values_supported`; that is the
  setting for a client talking to servers it does not control, since it never
  turns a working bearer flow into a refusal.

  One behavior to know: a DPoP connection does not follow HTTP redirects. A
  proof covers one method and one URL, nothing can re-sign it mid-chain, and
  neither retry recovers from a hop that carried the wrong one, so a `3xx` is
  surfaced as itself. Bearer connections are unaffected.

  Off by default and never self-enabling: SEP-1932 is unmerged and DPoP appears
  nowhere in the 2026-07-28 text, so the conformance suite scores `auth/dpop`
  and `auth/dpop-nonce` as extensions. Both are green on both profiles.

* **The OAuth grants that authenticate the client itself** (#109). neva's
  client implemented the authorization-code flow only, so a deployment with no
  user in front of a browser had nothing to run. Three profiles now do:
  `OAuthClientConfig::with_client_credentials()` (RFC 6749 section 4.4, the
  `io.modelcontextprotocol/oauth-client-credentials` extension),
  `with_jwt_bearer(..)` over the new `AssertionProvider` seam (RFC 7523
  section 2.1, workload identity federation), and `with_identity_assertion(..)`
  for the enterprise-managed profile, where `IdentityAssertion` trades an ID
  token at the identity provider for the RFC 8693 grant the resource's
  authorization server accepts.

  Everything ahead of the token request is unchanged -- the `401`, discovery,
  the RFC 8707 resource indicator -- and the browser round is simply not there,
  so the `AuthorizationHandler` is never called and no redirect listener is
  bound. Two behaviors worth knowing: a refusal ends the call, since the
  client presented the only credential it has and neither resends it nor
  reaches for another grant; and renewal is the grant run again, proactively,
  because RFC 6749 section 4.4.3 issues no refresh token to renew with.

* **`private_key_jwt` client authentication**, behind the new
  **`client-oauth-jwt`** feature (#109). The client signs a short-lived
  assertion with its own key instead of presenting a shared secret, which the
  client-credentials extension RECOMMENDS over a secret. Set with
  `OAuthClientConfig::with_private_key_jwt(..)`; opt-in because it is the only
  part of the OAuth client needing a JWS backend, and enabled by `client-full`.

  A Client ID Metadata Document may be paired with a key, which is what lets a
  client with no pre-registration authenticate at all
  ([CIMD draft section 6.2](https://www.ietf.org/archive/id/draft-ietf-oauth-client-id-metadata-document-00.html#section-6.2)).
  `client_metadata_document` then publishes the verifying key too -- embedded
  from `PrivateKeyJwt::with_public_jwk`, or referenced through the new
  `with_jwks_uri`, exactly one of the two per RFC 7591 section 2. Pairing a
  document with a client *secret* stays refused: it is resolved by whichever
  authorization server meets the URL, so there is nobody to have shared one
  with.

#### Shutdown
* **`App::with_shutdown()` / `App::with_shutdown_signal(..)` stop a server
  without an OS signal** (#103). Shutdown used to be signal-driven only, which
  left neva awkward to embed in a service that owns its own lifecycle and
  impossible to test: a test could only `handle.abort()` the server task, which
  skips every graceful path by construction. A `ShutdownHandle` composes with
  the signal handler rather than replacing it, so a server built this way still
  stops on Ctrl+C.

### Fixed

#### Authorization
* **A client secret is now presented the way the authorization server says it
  accepts it.** The method was always HTTP Basic, which volga 0.9.8 refuses
  outright against a server whose `token_endpoint_auth_methods_supported` does
  not list it. Basic remains the preference, and the fallback for a server that
  advertises nothing -- RFC 6749 section 2.3.1 requires servers to support it
  -- with `client_secret_post` where that is all the server takes.

* **A dynamic registration whose response named no
  `token_endpoint_auth_method` is now read against the server's own
  metadata.** RFC 7591 section 2 fills that silence with
  `client_secret_basic`, which a server advertising only `none` has already
  said it does not accept. Two documents from one server cannot both be right,
  and the one describing the token endpoint decides; left alone, such a flow
  registered successfully and then failed at the token request.

#### Subscriptions
* **A server shutting down now answers its live `subscriptions/listen` requests
  before closing their streams** (#103). The spec says a server ending a
  subscription on its own initiative SHOULD send the empty result first, so a
  client can tell an orderly end from a dropped connection. neva constructed
  that result but rarely delivered it: one cancellation token drove both the
  subscription and the transport, so the result raced a writer that had already
  broken out of its loop on the very same signal, and clients saw
  `SubscriptionEnd::Abrupt` where `Graceful` was owed.

  Shutdown is two-phase now. The signal ends the subscriptions, waits until the
  registry is empty and no message is still inside the middleware pipeline --
  together that means every result has reached the outbound channel -- and only
  then tears the transport down; the writers drain what is queued before they
  exit. `App::with_shutdown_drain(..)` caps the wait (2 seconds by default),
  and it is skipped outright when no subscription is open, so a server that
  never uses them shuts down exactly as fast as it did before.

## 0.5.3

### Added

#### Subscriptions
* **`App::with_notification_bus(..)` fans subscription notifications out across
  instances** (#104). A `subscriptions/listen` stream is a socket held open by
  one process, and the stateless transport pins nothing to an instance, so the
  subscriber and the request that mutates the server routinely land on
  different ones and the notification is silently lost. The new
  [`NotificationBus`](https://docs.rs/neva/latest/neva/trait.NotificationBus.html)
  carries them across: each instance publishes what it produces and delivers
  what it receives to the streams it holds. The subscriber table stays
  node-local -- half of every entry is a handle to a socket on one node, so a
  shared registry could not deliver anyway. Implementing a bus is a plain
  `async fn publish(&self, BusNotification)` plus a `subscribe` returning any
  `Stream`; it must not echo-suppress, since local delivery goes through that
  same stream. neva ships the trait, shared implementations (Redis pub/sub,
  NATS) live outside the crate, as for `RequestStateStore`.

  Nothing changes without one: there is no bus by default and notifications go
  straight to this instance's subscribers. A multi-instance stateless
  deployment now configures three things rather than two --
  `with_request_state_secret`, `with_request_state_store`, and this.

#### Authorization
* **Client ID Metadata Documents (CIMD).**
  `OAuthClientConfig::with_client_id_document(url)` identifies the client by an
  https URL the authorization server dereferences, so a client and server with
  no prior relationship need no registration request at all. The URL is checked
  against the spec's scheme and path rules when the client is built, and
  pairing it with a client secret is refused -- a document describes a public
  client. `client_metadata_document([redirect_uris])` builds the JSON to host
  there from the same code that would have registered, so the two cannot drift.
* **All three registration mechanisms, in the spec's priority order**: a
  pre-registered `client_id` first, then a metadata document where the server
  advertises `client_id_metadata_document_supported`, then Dynamic Client
  Registration, which the 2026-07-28 spec deprecates. Nothing changes for a
  client that configures no document. A server offering none of the three is
  refused before the browser opens, naming `with_client_id` as the way out.
* **`OAuthClientConfig::with_issuer`** names the authorization server the
  configured credentials belong to. A pre-registered `client_id` meeting a
  different issuer now fails, naming both, instead of being presented to a
  server that never issued it.

  **Custom `TokenStore` implementations:** the key is now
  `{issuer}|{client}|{resource}` -- the whole identity a credential belongs to
  -- rather than the resource alone, with any part the configuration does not
  name left empty, so two servers (or two clients sharing one durable store)
  never share a slot. Entries written by an earlier version are not found under
  the new key and are left in place; the affected sessions re-authorize once.
* **`App::with_request_state_audience`** binds MRTR `requestState` to this
  service's identity. The sealed state was bound to its request and principal
  but not to the service, so where several share one
  `with_request_state_secret`, a state minted by one was a state the others
  accepted. A mismatch is `InvalidParams`, and the check runs both ways: a
  state naming an audience is refused by a server that configures none.

  **Wire:** an audience-bound state is sealed under its own version (`v2.`
  rather than `v1.`), so a binary predating the option refuses it instead of
  dropping the member it does not know -- which would leave the binding
  unenforced by exactly the instance still to be upgraded. A deployment that
  configures no audience keeps minting `v1`; both versions decode.
* `ClientMetadata` re-exported from `neva::auth::oauth`.
* **A stored refresh token is only offered to the authorization server that
  minted it.** It is a bearer credential for its token endpoint, and the server
  a flow discovers is vouched for by the resource alone -- exactly what an
  attacker controlling the resource rewrites. The after-restart refresh added
  in 0.5.2 therefore now requires
  [`with_issuer`](https://docs.rs/neva/latest/neva/auth/oauth/struct.OAuthClientConfig.html#method.with_issuer)
  and reads the token back under it; without one the session re-authorizes
  interactively. Pointing `with_issuer` at a new server does not carry the old
  server's token over, and dynamically registered clients never reuse one.

### Changed

#### Subscriptions
* **`Context::resource_updated` no longer pre-checks `is_subscribed`.** It
  publishes unconditionally and lets the subscription filters route it, which
  is what they already did. The pre-check could only answer for the instance
  running the handler, so under a `NotificationBus` it would skip an update a
  subscriber elsewhere was waiting for. `Context::is_subscribed` is unchanged,
  and is now documented as node-local: use it to skip expensive local work, not
  to decide whether to notify.

#### Authorization
* **A redirect anywhere in `127.0.0.0/8` now registers a native client.**
  RFC 8252 section 7.3 gives a native client the whole loopback range, but the
  client matched the literal `127.0.0.1`, so a handler bound to `127.0.0.2`
  declared itself a `web` client -- which an OIDC-strict authorization server
  refuses for a plain-http redirect URI. `localhost` and `[::1]` are unchanged.
* The OAuth client's URL and query handling now goes through
  [`url`](https://docs.rs/url), an optional dependency gated on `client-oauth`
  and already compiled for every `client-*` build via `reqwest`. It catches
  what a hand-rolled split does not -- an out-of-range port above all, which
  `http::Uri` reports as no port at all.

### Fixed

#### HTTP transport and sessions
* **`bind("::1:3000")` now gets DNS-rebinding protection.** `std` takes the last
  colon of an unbracketed IPv6 bind string as the port separator, so that
  address really does listen on `[::1]:3000` -- but the default policy read the
  string whole, where it parses as the *different*, non-loopback address
  `::1:3000`. A server on loopback therefore defaulted to `allow_any_origin`,
  with the `Origin`/`Host` checks the spec makes a MUST for local servers
  switched off. Bind strings are now read the way `std` reads them;
  `[::1]:3000`, `127.0.0.1:3000` and `localhost:3000` were never affected.
* An `Origin` header carrying userinfo is no longer matched against the
  allowlist by the name in front of the `@`: `https://app.example.com:8443@evil.com`
  has the host `evil.com`. Hardening rather than a reachable bypass -- `Origin`
  is browser-set and a browser cannot be made to send this.

## 0.5.2

### Added
* **DNS-rebinding protection.** The HTTP server validates `Origin` and `Host`:
  on a loopback bind it refuses non-loopback names with `403` before reading the
  body. `HttpServer::with_allowed_origins([...])` names more;
  `allow_any_origin()` turns the gate off. An entry naming a scheme
  (`https://app.example.com`) is an origin and matched as one, so trusting an
  application does not trust what else its host serves; a bare host holds
  neither scheme nor port against the request. `Host` is matched by name either
  way.
* **`Context::client_capabilities()`** reports what the caller declared in this
  request's `_meta`, so a handler can branch before asking for an input kind it
  would be refused for (`MissingRequiredClientCapability`). Elicitation is
  reported down to the mode, via the new `ElicitationModes`.
* **`Option<T>` tool and prompt arguments**, via `ToolArg` (returned by
  `ToolHandler::args`) and `PromptArgument::named(name, required)`. A tool whose
  arguments are all optional publishes no `required` key.
* **`Tool::with_arg_names([...])`** names a bare closure's arguments, renaming
  the published schema and the extraction names together; `map_tool!` /
  `map_prompt!` read those names off the closure itself.
* **`App::run` fails at startup** when a tool or prompt and its handler disagree
  about arguments. `Context::add_tool` / `add_prompt` run the same check.
* `ArgNames` and `FromHandlerArgs` in `neva::types`.
* **A dropped `POST` response stream is resumed once**, with a `GET` carrying
  `Last-Event-ID` after the pause the server asked for. Legacy profile only, and
  only when the server named an id to resume from.
* **The SSE `retry:` field sets the reconnection delay**, instead of a fixed
  three seconds.
* **Protected Resource Metadata is looked for at the origin too** when the
  RFC 9728 path-based location misses; the document found there is validated
  against the origin rather than the endpoint.
* **The `scope` a `WWW-Authenticate` challenge names is what gets requested.**
  Order: configured scopes, the challenge, `scopes_supported`, none at all.
* **A `403 insufficient_scope` re-authorizes**, asking for the union of the
  existing grant and what the challenge demands (SEP-2350). A `403` without that
  challenge is untouched.
* **An advertised RFC 9207 `iss` is enforced** -- the flag was read out of the
  metadata document's unmodelled fields and so always came out false.

### Changed
* **A server may answer the handshake with a different protocol version.** Only
  a version outside `PROTOCOL_VERSIONS` now ends the connection.
* **`Client::disconnect` sends nothing.** The param-less
  `notifications/cancelled` it used to send fails the spec's own schema.
* **An MRTR round carries every input the handler asked for**, in one
  `InputRequiredResult`, when the handler holds its `?` until it has asked for
  everything.
* **An `inputResponses` entry that does not fit is dropped, not rejected.**
  Unsolicited, stale or state-less answers cost a round instead of failing the
  call with `-32602`. An answer of the wrong kind is still an error, now a
  JSON-RPC one rather than an in-band tool error.
* `Prompt::with_args` also sets the extraction names, so the two cannot drift.

### Changed (breaking)
* `App::map_tool` / `Tool::new` take
  `Args: FromHandlerArgs<CallToolRequestParams>` and `App::map_prompt` /
  `Prompt::new` take `Args: FromHandlerArgs<GetPromptRequestParams>`, replacing
  the `TryFrom<...>` bounds. Handlers are unaffected; a hand-written
  `impl TryFrom` needs porting.
* `ToolHandler::args` returns `Vec<ToolArg>` instead of
  `Option<HashMap<String, SchemaProperty>>` -- ordered, so the *n*-th entry is
  the *n*-th argument slot.
* `HandlerParams::Tool` and `HandlerParams::Prompt` carry the primitive's
  `ArgNames` alongside the params.
* `PropertyType` gains an `Integer` variant and `"integer"` no longer
  deserializes into `Number`; a match needs the new arm.
* The schema structs in `neva::types::schema` gain an `extra` field, so an
  exhaustive struct literal needs it (or `..Default::default()`). `EnumOption`
  gives up `Eq` with it -- an arbitrary JSON value is not `Eq`; `PartialEq`
  stays.
* **Wire:** a tool registered from a bare closure advertises `arg0`, `arg1`, ...
  instead of the former type names. `#[tool]` tools are unaffected.

### Fixed

#### Authorization
* **`insufficient_scope` is read off the challenge's `error` parameter**, not by
  searching the whole `WWW-Authenticate` value for the string.
* **A challenge that names no `scope` is still a step-up** -- the attribute is
  optional in RFC 6750.
* **The applicable Bearer challenge is found wherever the server put it** --
  behind another scheme, in a second `WWW-Authenticate` value, or later in the
  same one, all of which RFC 9110 allows. Among several the one naming
  `insufficient_scope` is acted on: it carries the `scope` the request was short
  of, and reading any other leaves the step-up asking for the grant it had.
* **A `403` on the standalone SSE `GET` re-authorizes** like one on a `POST`
  (legacy profile).
* **A step-up that lost the race reuses the winner's token**, but only when the
  grant on record covers what was demanded. A challenge that named no scope
  gives nothing to check that against, and a changed token proves nothing -- a
  refresh rotates one without widening it -- so that case runs the flow.
* **A stored refresh token survives a restart**: the refresh is retried once the
  client and metadata have been rebuilt, on the way to the interactive flow.
  Only with a configured `client_id` -- a refresh token belongs to the client it
  was issued to.
* **What the session believes it was granted follows the token.** It records
  what the response *granted*, not what was asked for, so a granted subset no
  longer reads as a token that merely expired; one that omits `scope` (RFC 6749
  5.1) has the grant carried over, one that narrows it updates the record. All
  of it reaches the token store, so a step-up after a restart widens the stored
  grant rather than replacing it.
* **A challenge naming a scope outside `with_scopes` fails, naming it**, rather
  than running a flow that cannot obtain it.
* **Only a `404` opens the Protected Resource Metadata origin fallback** -- a
  malformed body or a mismatched `resource` is the path-based document
  answering, and its answer is authoritative.
* **The RFC 8707 resource indicator is the one the accepted metadata declares**,
  not the endpoint URL.
* **A resumption asks for a token when its turn comes**, not carrying the one
  the original `POST` used: the server names the wait, and it can outlast that
  token. A `401` there re-authorizes once and tries again, as the `POST` and the
  standalone `GET` already did, instead of losing the answer.

#### HTTP transport and sessions
* **SSE responses carry `X-Content-Type-Options: nosniff`.** Without it Firefox
  buffers the stream to sniff its type and a `fetch()` reader sees nothing.
* **Each stream carries its own `Last-Event-ID` cursor and `retry:` delay.**
  Both lived on the session, so the `POST` and standalone `GET` streams sent
  each other back to positions they had never reached, and whichever frame
  landed last set the other's reconnection time.
* **A resumed response stream is released once it has answered**, instead of
  being read forever -- one leaked connection per truncated reply. What is owed
  is tracked per request, so a batch resumes only what is still unanswered.
* **An SSE `retry: 0` means reconnect immediately**; zero was dropped as if
  nothing had been stated.
* **A server offering no standalone SSE stream no longer ends the session**
  (legacy profile): `404` and `405` release the pending `initialize` and the
  session carries on over POST alone.
* **A `404` after a stream that worked means the session is gone** and ends the
  connection, instead of carrying on dead (legacy profile).
* **A terminated session answers `404` on `POST`, `GET` and `DELETE`** (legacy
  profile), the POST body carrying a JSON-RPC error addressed to the request's
  own id. `initialize` is exempt; a live id counts as activity against the
  idle sweep.
* **A stale session answers a notification with the status alone**, not a
  `null`-id JSON-RPC error that matches nothing on the other side.
* **A notification reaches the session stream the moment it is emitted** (legacy
  profile), so a handler's response can no longer overtake the progress it just
  reported. Per-session delivery replaces one shared 100-slot queue, and
  `layer()` no longer spawns.
* **A full notification queue no longer logs itself to death** (legacy profile)
  -- the warning fed straight back into the layer until the stack ran out.
* **The POST no longer carries `Content-Type` twice**, which drew `415` from
  strict receivers.

#### MRTR and parameter headers
* **`inputResponses` / `requestState` travel on the params, not in `_meta`**,
  where the spec puts them -- against any other implementation the retry looked
  like a fresh call. `_meta` is still read as a fallback;
  `Request::input_responses()` and `Request::state()` read either.
* **The idempotency digest covers the answers a round resolved to**, not the raw
  `inputResponses`. An unasked key could otherwise miss the cache and re-run the
  final handler's `on_commit` effects.
* **The MRTR field check applies to MRTR methods only**; on a method registered
  with `App::map_handler` those names are the handler's own params.
* **A `requestState` / `inputResponses` of the wrong JSON type is
  `InvalidParams`**, not silently absent. An explicit `null` counts as stating
  the field wrongly -- the spec makes both optional by *absence*.
* **A declared elicitation mode is honored.** `elicitation` was one flag, so a
  client declaring `{"form": {}}` was sent `url` requests it had said nothing
  about and the round stalled. Named modes are a list of what the client can do,
  and `MissingRequiredClientCapability` names the mode; naming none rules none
  out, which is what a bare `{}` has always meant.
* **`x-mcp-header` registrations expire with the listing that carried them**
  (SEP-2243 with SEP-2549's clock): usable for the listing's `ttlMs`, and an
  absent `ttlMs` reads as `0`. A `HeaderMismatch` (`-32020`) then has the client
  re-list and retry once; that listing is good for the retry whatever its TTL,
  for the refused tool and that one exchange only. The re-listing runs to the
  end -- a restarted traversal clears what the last one recorded, so a page it
  never reaches loses its annotations and its record of the dropped tools.

#### Schemas and arguments
* **A schema is published the way it was declared.** `Schema` and (legacy
  profile) `ToolSchema` keep every unmodelled keyword verbatim in a flattened
  `extra`: `default` (SEP-1034), `pattern`, `examples`, and the `$schema`,
  `$defs`, `$ref`, `additionalProperties`, `allOf`/`anyOf` and
  `if`/`then`/`else` that SEP-2106 requires to survive untouched -- below the
  root as well, in an enum form's `items` and each `anyOf` option under it.
* **A tool property keeps its own keywords too** (legacy profile): `enum`,
  `format`, `$ref` and `minimum` were dropped, and a property stating no `type`
  no longer acquires `"object"`.
* **A field declared `integer` rejects `1.5`.** The check judges the value, not
  how it was written, so `1.0` still passes.
* **Tool and prompt arguments are extracted by name, not by position.** Failures
  name the argument, an omitted argument arrives as `null` instead of erroring,
  and `Meta<_>` / `Context` / `Dc<_>` consume no argument slot wherever they sit
  -- classified from the *resolved* type, so type aliases are recognised.
* **Tools registered from a closure publish one property per argument**, instead
  of keying them by type name and collapsing `|a: i32, b: i32|` into one.
* **A tool call with no arguments omits `arguments`** rather than sending
  `null`, which a validating peer rejects.

#### Wire and protocol
* **Per-request `clientCapabilities` are read as the spec's optional objects**,
  not booleans. Every request from a conformant client -- MCP Inspector among
  them -- failed with `-32602 invalid type: map, expected a boolean`. Both
  shapes are accepted on the way in.
* **Elicitation params are written as the spec's union**, not a tagged enum:
  `{"Form": {...}}` hid `message` and `requestedSchema` from every peer but
  neva. Affects both protocol profiles.
* **An elicitation `mode` that is present has to be a string**; `null` was read
  as "no mode" and delivered as a well-formed form.
* **A `resources` capability that omits `subscribe` parses**, instead of failing
  the handshake it arrives with.
* **An unimplemented method answers `404` over HTTP**, not `200` with `-32601`.
* **A body protocol version disagreeing with the `MCP-Protocol-Version` header
  is a `HeaderMismatch` (`-32020`)**, not `UnsupportedProtocolVersion` -- "retry
  with a version from this list" does not fix a header and a body that disagree.
* **`resources/read` names the unknown URI in `error.data.uri`** (a spec
  SHOULD).

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
