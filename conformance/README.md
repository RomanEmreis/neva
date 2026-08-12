# neva conformance harness

Fixtures driven by the official
[MCP conformance suite](https://github.com/modelcontextprotocol/conformance).
The suite is the executable form of the specification: it connects to
`conformance-server` as a client and drives it through every server scenario,
and it starts its own server and drives `conformance-client` through every
client scenario.

This crate is `publish = false` and is not a `default-member`, so nothing here
reaches crates.io or a normal `cargo build`.

## Running it locally

**Always name the version.** npm's `latest` dist-tag is still on the `0.1.x`
line, which predates both MCP 2026-07-28 and the `--requirements` flag -- a bare
`npx @modelcontextprotocol/conformance` fails with
`error: unknown option '--requirements'`. The 2026-07-28 scenarios live on the
`alpha` line only. Use the same version CI does:

```bash
export CONFORMANCE_VERSION=$(cat conformance/.conformance-version)
```

That file is the single pin -- CI reads the same one, so a local run and the
workflow can never drift onto different suites.

Then, from the repository root:

```bash
# 1. build and start the fixture server (default profile, MCP 2026-07-28)
cargo build -p neva-conformance
PORT=3000 ./target/debug/conformance-server &

# 2. server mode
npx -y "@modelcontextprotocol/conformance@${CONFORMANCE_VERSION}" server \
    --url http://127.0.0.1:3000/mcp \
    --requirements 2026-07-28 \
    --expected-failures conformance/expected-failures-2026-07-28.yaml

# 3. client mode (the suite starts its own server)
npx -y "@modelcontextprotocol/conformance@${CONFORMANCE_VERSION}" client \
    --command "$(pwd)/target/debug/conformance-client" \
    --requirements 2026-07-28 \
    --expected-failures conformance/expected-failures-2026-07-28.yaml

# 4. stop the fixture server
kill %1
```

The legacy profile is the same commands against a different build and a
different requirement set:

```bash
cargo build -p neva-conformance --features legacy-spec
PORT=3000 ./target/debug/conformance-server &
npx -y "@modelcontextprotocol/conformance@${CONFORMANCE_VERSION}" server \
    --url http://127.0.0.1:3000/mcp \
    --requirements 2025-11-25 \
    --expected-failures conformance/expected-failures-2025-11-25.yaml
```

To look at one failure, drop `--requirements` and name the scenario -- the
baseline goes too, so the checks print raw:

```bash
npx -y "@modelcontextprotocol/conformance@${CONFORMANCE_VERSION}" server \
    --url http://127.0.0.1:3000/mcp --scenario server-stateless
```

`list` shows what exists, `--requirements <revision>` shows what a revision
actually demands:

```bash
npx -y "@modelcontextprotocol/conformance@${CONFORMANCE_VERSION}" list --requirements 2026-07-28
```

## Why two profiles

neva picks its protocol generation at compile time, so one build speaks one
generation. An SDK that negotiates at runtime can cover both from a single
binary; we cannot, which is why CI runs a matrix:

| profile      | build                    | requirement set |
| ------------ | ------------------------ | --------------- |
| default      | (no extra features)      | `2026-07-28`    |
| `legacy-spec`| `--features legacy-spec` | `2025-11-25`    |

The legacy row is the only automated proof that the pre-2026 wire still works
after a refactor.

## The fixtures are a contract

Scenario checks look primitives up **by name**: `test_simple_text`,
`test://static-text`, `test_prompt_with_arguments`, the `user_name` key of an
`InputRequiredResult`. Renaming one does not make a scenario fail loudly -- it
makes it report "not testable", which is worse, because it looks like a pass in
the summary. Treat the names in `src/bin/server.rs` as fixed, and check them
against the scenario source when adding new ones.

Three of them exist purely so a structural rule can be probed:

| fixture                      | what it lets the suite check                          |
| ---------------------------- | ----------------------------------------------------- |
| `test_missing_capability`    | a capability the caller never declared is refused      |
| `test_logging_tool`          | no log notifications without `_meta.../logLevel`       |
| `test_streaming_elicitation` | the response stream carries no independent requests    |
| `test_trigger_tool_change`   | `tools/list_changed` reaches a `subscriptions/listen`  |
| `test_trigger_prompt_change` | the same for `prompts/list_changed`                    |

## The baselines

`expected-failures-*.yaml` record what each profile currently fails. The
semantics are strict in both directions:

* a failure that is **not** listed fails the build -- a regression;
* an entry that starts **passing** fails the build -- a stale entry.

So a baseline entry is a debt with an owner, not a mute button, and deleting one
is how a fix is proved. Every entry names the issue that retires it.

`server-stateless` is baselined per check (`scenario:check-id`) rather than
wholesale: it carries 30 checks and only 7 fail, and excusing the scenario would
stop enforcing the 23 that already hold.

Extension scenarios (Tasks, MCP Apps) run under `--requirements` but are not
scored, so they are reported without being baselined -- a spec revision does not
require an extension.

## Bumping the suite

`conformance/.conformance-version` is the one place the suite version is pinned;
CI reads it and so does the export above. It is pinned on purpose: the suite
grows between releases, and an unpinned `@latest` turns somebody else's new
scenario into our red build. Bump the file deliberately, re-run all four
commands above, and update the baselines in the same commit.
