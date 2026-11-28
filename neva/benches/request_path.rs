//! What a `tools/call` costs on its way through the server.
//!
//! Two payloads, as in <https://github.com/RomanEmreis/neva/issues/137>: a
//! "small" call of two scalar arguments and a "medium" one of about 15 KB. Both
//! carry the `_meta` a 2026-07-28 client sends with every request.
//!
//! - `parse`: the body read as a [`Message`], what every transport does first.
//! - `meta`: one full read of the request's `_meta`.
//! - `call`: the whole round trip through [`TestClient`]: the client builds and
//!   writes the request, the server parses, dispatches, extracts and answers,
//!   the client reads the reply. The pipe is the stdio transport's, in memory.
//!
//! ```text
//! cargo bench -p neva --features testing --bench request_path
//! ```

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use neva::App;
use neva::testing::TestClient;
use neva::types::{Message, Request};
use serde::Deserialize;
use serde_json::{Value, json};
use std::hint::black_box;

/// One element of the medium payload.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "read by the deserializer, not by the bench")]
struct Doc {
    id: u32,
    title: String,
    tags: Vec<String>,
    score: f64,
}

/// The arguments of the two calls, by name.
fn payloads() -> [(&'static str, Value); 2] {
    let docs = (0..200)
        .map(|i| {
            json!({
                "id": i,
                "title": format!("title {i}"),
                "tags": ["a", "b", "c"],
                "score": f64::from(i) / 3.0,
            })
        })
        .collect::<Vec<_>>();

    [
        ("small", json!({ "query": "rust mcp", "limit": 10 })),
        ("medium", json!({ "docs": docs })),
    ]
}

/// A `tools/call` body as a 2026-07-28 client writes it.
fn body(args: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "tools/call",
        "params": {
            "name": "search",
            "arguments": args,
            "_meta": {
                "progressToken": 42,
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {
                    "elicitation": {}, "sampling": {}, "roots": {}
                },
                "io.modelcontextprotocol/clientInfo": {
                    "name": "some-client", "version": "1.2.3"
                }
            }
        }
    }))
    .expect("a JSON body")
}

fn parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for (label, args) in payloads() {
        let body = body(&args);
        group.bench_with_input(BenchmarkId::from_parameter(label), &body, |b, body| {
            b.iter(|| serde_json::from_slice::<Message>(black_box(body)).expect("a message"));
        });
    }
    group.finish();
}

fn meta(c: &mut Criterion) {
    let mut group = c.benchmark_group("meta");
    for (label, args) in payloads() {
        let req: Request = serde_json::from_slice(&body(&args)).expect("a request");
        group.bench_with_input(BenchmarkId::from_parameter(label), &req, |b, req| {
            b.iter(|| black_box(req).meta());
        });
    }
    group.finish();
}

fn call(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");

    let mut app = App::new();
    app.map_tool("search", |query: String, limit: i64| async move {
        format!("{query}: {limit}")
    })
    .with_arg_names(["query", "limit"]);
    app.map_tool("index", |docs: Vec<Doc>| async move { docs.len() })
        .with_arg_names(["docs"]);

    let client = rt.block_on(TestClient::connect(app)).expect("connect");

    let mut group = c.benchmark_group("call");
    for (label, args) in payloads() {
        let tool = if label == "small" { "search" } else { "index" };
        group.bench_with_input(BenchmarkId::from_parameter(label), &args, |b, args| {
            b.iter(|| {
                rt.block_on(client.tools().call(tool, args.clone()))
                    .expect("a tool result")
            });
        });
    }
    group.finish();

    rt.block_on(client.shutdown()).expect("shutdown");
}

criterion_group!(benches, parse, meta, call);
criterion_main!(benches);
