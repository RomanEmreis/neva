# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## Unreleased

### Added
* Engine-neutral OAuth 2.1 resource-server primitives behind the new
  `server-oauth` feature (included in `server-full`), built on
  `volga-oauth-core` — protocol types only, no Volga framework dependency,
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
    `WWW-Authenticate: Bearer resource_metadata="…"` challenge.
  * The default Volga engine mounts the document on its well-known path
    automatically (publicly reachable — auth enforcement is scoped to the
    MCP endpoint group) and, when bearer auth is configured, advertises it
    as `resource_metadata` on Volga's own 401 challenges.
  * Protocol types re-exported under `neva::auth::oauth`
    (`ProtectedResourceMetadata`, `BearerChallenge`, `OAuthError`, …).
* **OAuth 2.1/OIDC issuer mode for the default Volga engine** (#69):
  `with_auth(|auth| auth.with_oauth(|oauth| oauth.with_issuer(…)))` replaces
  the static decoding key with issuer-discovered JWKS validation (RFC 8414
  discovery with OIDC fallback, key rotation, refresh cooldown / max key age
  via Volga). MCP defaults applied unless overridden: the token's `aud` must
  contain the server's canonical resource URI (RFC 8707 — `aud` becomes
  required) and its `iss` must match the configured issuer. The Protected
  Resource Metadata document is derived from the issuer automatically when
  `with_oauth_metadata` was not called (#68), so discovery, challenge and
  validation work out of the box with a single builder call. New
  `AuthConfig::with_resource`/`with_resources` (RFC 8707 resource
  indicators) for overriding the audience explicitly.
* **MRTR `requestState` key rotation.** New `App::with_request_state_keys(active_kid, keys)`
  configures a keyring: new blobs are sealed under the active key id, inbound
  blobs decrypt with whichever accepted key their kid names — enabling
  zero-downtime rotation. `with_request_state_secret` remains the single-key
  shorthand (kid `"0"`). (#81)

### Fixed
* The legacy `initialize` result no longer advertises the `logging` capability
  in builds without the `tracing` feature, where the `logging/setLevel`
  handler is not registered — capability-trusting clients (e.g. newer MCP
  Inspector) would call it and hit `Method not found`.
* Per-tool/prompt/resource role and permission gates now receive the claims
  Volga's `authorize` middleware already validated (`Authenticated<…>` from
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
current default moves under a `legacy-spec` flag — a deliberate breaking
change, mirroring the spec itself.

### Added

#### MCP 2026-07-28 RC (opt-in via `proto-2026-07-28-rc`)

* **Stateless HTTP transport.** The `initialize`/`initialized` handshake is
  replaced by a single `server/discover` request returning `DiscoverResult`
  (with `Client::discover()`; `Client::init()` kept as a back-compat alias).
  No `Mcp-Session-Id` on the wire; the GET (SSE) and DELETE routes are not
  registered. Every POST carries a required `MCP-Protocol-Version` header —
  the client injects automatically; the server rejects missing/unsupported
  values with `InvalidRequest`.
* **JSON Schema 2020-12 for tools.** New `schema_2020::InputSchema` —
  `#[serde(transparent)]` newtype over `serde_json::Value` — and a per-flag
  `ToolInputSchema` alias on `Tool.input_schema`/`output_schema`. The
  `#[tool]` macro now emits full 2020-12 documents: primitive args become
  inline primitive schemas, structured `Json<T>` args derive a rich inlined
  schema when `T: JsonSchema` (graceful `{"type":"object"}` fallback
  otherwise), and the return type drives `outputSchema`.
  `input_schema`/`output_schema` string literals are validated at compile
  time on every feature configuration. `schemars` is re-exported by neva —
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
  *within a single chain* — pair non-idempotent effects with a downstream
  idempotency key.
* **MRTR final-round idempotency store.** `RequestStateStore` trait
  (`neva::app::mrtr_store`) with a default per-process `InMemoryStateStore`,
  wired via `App::with_request_state_store`. Caches the final response keyed
  by the incoming state's integrity tag + answers digest, so a lost-response
  retry returns the cached result instead of re-running the handler. Implement
  over a shared backend (e.g. Redis) for multi-instance — same constraint as
  a shared signing secret.
* **Task-augmented elicit.** Two execution substrates that never mix: a bare
  call uses MRTR re-run (`ctx.elicit(key, params)`); a task-augmented call
  genuinely suspends (`ctx.task().elicit(params)`, no replay key). New
  `ctx.task()` builder and `ctx.is_task()` switch for `TaskSupport::Optional`
  tools that elicit on both substrates. Resuming a parked task-elicit requires
  the answer to reach the instance running the task — same instance-affinity
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
    8) caps MRTR re-issue rounds — counted as retries, the initial send is
       always made on top.
* **Startup deployment warn.** When `http-server` + `tracing` are enabled and
  `with_request_state_secret` was never called, `App::run` logs at
  `tracing::warn!` so multi-instance deployments don't silently fail to
  decrypt `requestState` on cross-instance retries.

#### Spec-neutral

* `ErrorCode::RESOURCE_NOT_FOUND` — helper constant emitting `-32002` under
  the legacy spec and `-32602` (`InvalidParams`) under RC. In-tree emitters
  route through it; recommended migration path off
  `ErrorCode::ResourceNotFound`.
* `ToolSchema::from_value`, `from_schema::<T>`, `from_schemars` — legacy-side
  constructors symmetric with `InputSchema`.

### Changed

* `Tool.input_schema`/`output_schema` use the per-flag `ToolInputSchema`
  alias. `Tool::validate` extracts the schema as `serde_json::Value` before
  invoking the validator, so the same validator path covers both flavours.
* `PROTOCOL_VERSIONS` advertises `"2026-07-28"` only when the RC flag is
  enabled.
* CI matrix extended with `proto-2026-07-28-rc` × `server-full,client-full`
  under clippy, doc, and test jobs.

#### Under `proto-2026-07-28-rc` only

* **`Context::elicit`** now takes a stable `key` argument and follows MRTR
  re-run/replay. Handlers must be side-effect-free up to each elicit point
  (use `once`/`memo`/`on_commit` for effects). `Claims` gains an additive
  `subject()` accessor (default `None`) for principal-binding.
* **Tasks capability** is advertised under
  `capabilities.extensions["io.modelcontextprotocol/tasks"]` instead of the
  top-level `capabilities.tasks`. The `with_tasks` API and `tasks/*` wire
  methods are unchanged — `with_tasks` now thinly wraps the extension
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
* The typed `ToolSchema` (with `from_json_str` / `with_required`) — replaced
  by `InputSchema`.
* `McpOptions::with_mcp_version` on both server and client builders — the RC
  build is a pure 2026-07-28 peer, so the version is fixed. Version selection
  returns under the legacy flag once the RC graduates.

### Deprecated

* `ErrorCode::ResourceNotFound` — use `ErrorCode::RESOURCE_NOT_FOUND` for
  per-flag wire mapping.
* `Client::add_root`, `add_roots`, `publish_roots_changed`, `map_sampling`,
  and `McpOptions::with_logging` — removed in MCP 2026-07-28. Available under
  the legacy spec; cfg-gated out under RC.
* `ToolSchema::from_schema(schemars::Schema)` — renamed to `from_schemars`
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
  like `Context`/`Meta<_>` — injected from request context, never listed as
  an argument.

### Known limitations

* `#[tool]`'s `annotations = "…"` (and `#[prompt]`/`#[resource]` JSON-string
  attributes) still parse at runtime and panic on malformed JSON; compile-time
  validation there is a planned follow-up. `input_schema`/`output_schema`
  literals are already validated at compile time.
* `cargo check --no-default-features --features client` (without
  `--all-targets`) fails on `tokio::task::block_in_place` because the
  `client` feature alone doesn't pull `tokio/rt-multi-thread`. Pre-existing
  tokio-features issue. CI runs the `--all-targets` variant (pulls dev-deps
  → `rt-multi-thread`) and remains green.

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
* `dispatch_post` / `dispatch_delete` / `dispatch_get_sse` engine-generic route helpers — adapter handlers collapse to one-liners.
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
* Fixed JSON-RPC 2.0 protocol violation: server no longer sends a response to client notifications (§4 — notifications must never be replied to)
* Fixed `notifications/cancelled`: request cancellation now actually fires for both stdio and Streamable HTTP transports
* Fixed Streamable HTTP transport silently dropping notifications without processing them

## 0.2.7

### Added
* JSON-RPC Batch Support for client and server

### Fixed
* Fixed broken Streamable HTTP server implementation
