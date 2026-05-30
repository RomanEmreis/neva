# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## Unreleased

### Added

* New compile-time feature flag `proto-2026-07-28-rc` enabling the MCP Release Candidate spec 2026-07-28 wire format. Opt-in only; the legacy spec remains the default.
* `neva::types::schema_2020::InputSchema` — `#[serde(transparent)]` newtype around `serde_json::Value`, holding full JSON Schema 2020-12 documents verbatim. Ships with `from_value`, `from_json_str`, `from_schema::<T>()`, `from_schemars`, `as_value`, and `into_value`.
* `neva::types::ToolInputSchema` — per-flag type alias that resolves to `tool::ToolSchema` under the legacy spec and to `schema_2020::InputSchema` under `proto-2026-07-28-rc`. Use this alias in code that constructs or accepts tool schemas so the same call site compiles under either flag.
* `ToolSchema::from_value(Value) -> Result<Self, Error>` — fallible Value constructor, mirroring `InputSchema::from_value`.
* `ToolSchema::from_schema::<T: JsonSchema>()` — generic constructor symmetric with `InputSchema::from_schema::<T>()`.
* `ToolSchema::from_schemars(schemars::Schema)` — non-generic constructor renamed for symmetry with `InputSchema::from_schemars`.
* `neva::types::CacheScope` and `ttlMs` / `cacheScope` fields on the four MCP list results (tools / prompts / resources / resource templates), per the RC spec's caching hints.
* RC-only routing headers (`Mcp-Session-Id` / routing hints) injected into the HTTP POST send loop, plus a `routing_hints` helper on the client transport.
* `traceparent` / `tracestate` fields on `RequestParamsMeta` and a client-side `TraceContextProvider` hook (RC only), with matching server-side span `record`.
* `ErrorCode::RESOURCE_NOT_FOUND` constant — emits `-32002` (legacy `ResourceNotFound`) or `-32602` (`InvalidParams`) per the active spec version. All in-tree emitters of "resource not found" route through this constant so the wire code automatically follows the active flag.
* The `#[tool]` proc-macro now emits JSON Schema 2020-12 `inputSchema` / `outputSchema` under `proto-2026-07-28-rc`: primitive arguments map to inline primitive schemas, structured `Json<T>` arguments derive a rich, inlined schema when the inner `T` implements `JsonSchema` (graceful `{"type":"object"}` fallback otherwise), and the return type drives `outputSchema`. Explicit `input_schema` / `output_schema` string literals are now validated at compile time (on every feature configuration). `schemars` need not be a direct dependency of the user crate — it is re-exported by neva.
* Stateless HTTP transport (RC only): the `initialize` / `initialized` handshake is replaced by a new `server/discover` request returning `neva::types::DiscoverResult`. The client gains `Client::discover()` (with `Client::init()` kept as a back-compat alias so existing `connect()` flows keep working).
* A required `MCP-Protocol-Version` request header (RC only) on every HTTP POST. The client injects it automatically; the server rejects a missing or unsupported value with JSON-RPC `InvalidRequest`.
* Client implementation info is now carried in each request's `_meta` under `io.modelcontextprotocol/clientInfo` (RC only), merged non-destructively with any existing `_meta` (e.g. `traceparent`).

### Changed

* `Tool.input_schema` and `Tool.output_schema` now use the per-flag `ToolInputSchema` alias instead of the typed `ToolSchema` directly. Under `proto-2026-07-28-rc` these fields carry a Value-shaped `InputSchema`; under the legacy spec they continue to carry `ToolSchema`.
* `Tool::validate(&CallToolResponse)` now extracts the schema as `serde_json::Value` (via `as_value()` under RC, `serde_json::to_value(&...)` under legacy) before invoking the JSON Schema validator, so the same validator path covers both spec flavours.
* Server completion logic now walks Value-shaped schemas under RC (no compile-time field access on a typed struct).
* `PROTOCOL_VERSIONS` advertises `"2026-07-28"` only when the RC flag is enabled; the stable versions remain unconditionally listed.
* CI matrix extended with `proto-2026-07-28-rc` paired with `server-full client-full`, covered by clippy, doc, and test jobs.
* Under `proto-2026-07-28-rc` the HTTP transport is request/response only: no `Mcp-Session-Id` is emitted or required on the wire, the GET (SSE) and DELETE routes are not registered, and server-initiated notifications (progress, resource-updated, list-changed, task-status, elicitation) are inert — `Context` notification helpers become no-ops, so clients poll (`tools/list`, `resources/read`, `server/discover`) instead. The session id is still minted internally to keep the per-request correlation key collision-free.

### Deprecated

* `ErrorCode::ResourceNotFound` — use the helper constant `ErrorCode::RESOURCE_NOT_FOUND` instead. Under RC this maps to `InvalidParams` per the spec; under the legacy spec it maps to `-32002` for backwards compatibility.
* `Client::add_root`, `Client::add_roots`, `Client::publish_roots_changed`, `Client::map_sampling` — `roots/list`, `notifications/roots/list_changed`, and `sampling/createMessage` are removed in MCP 2026-07-28. The methods remain available under the legacy spec and are completely absent (cfg-gated out) under `proto-2026-07-28-rc`.
* `McpOptions::with_logging` and the server-emitted `notifications/message` / `logging/setLevel` handlers — server-side logging is removed in MCP 2026-07-28. Available under the legacy spec; absent under `proto-2026-07-28-rc`.
* `ToolSchema::from_schema(schemars::Schema)` — renamed to `from_schemars` for symmetry with `InputSchema::from_schemars`. The previous name remains available as `from_schema_legacy` with `#[deprecated]` so legacy code keeps compiling during the transition.

### Removed under `proto-2026-07-28-rc`

* `roots/list` request, `notifications/roots/list_changed` notification, and the `Root` / `Roots` types.
* `sampling/createMessage` request, the `SamplingHandler` / `SamplingTaskCapability` types, and the `sampling!` proc-macro re-export.
* `logging/setLevel` request, `notifications/message` notification, `LoggingLevel` / `LogMessage` / `SetLevelRequestParams` types, and the `NotificationFormatter` helper.
* The typed `ToolSchema` struct (and its `from_json_str` / `with_required` builder methods) — replaced by the Value-shaped `InputSchema`.

### Known limitations

* The `#[tool]` macro's `annotations = "…"` attribute (and the `#[prompt]` / `#[resource]` JSON-string attributes) still parse at runtime via `from_json_str` and panic on malformed JSON; compile-time validation there is a planned follow-up. The `#[tool]` `input_schema` / `output_schema` literals are already validated at compile time.
* `cargo check --no-default-features --features client` (without `--all-targets`) still fails on `tokio::task::block_in_place` because the `client` feature alone does not pull in `tokio/rt-multi-thread`. This is a pre-existing tokio-features issue (independent of this changeset). CI runs the `--all-targets` variant — which pulls dev-deps and therefore `rt-multi-thread` — and remains green.

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
