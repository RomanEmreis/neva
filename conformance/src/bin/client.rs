//! Client fixture for the official MCP conformance suite.
//!
//! The suite starts its own server, then spawns this binary with the server URL
//! as the final argument and the scenario name in `MCP_CONFORMANCE_SCENARIO`.
//! Each scenario expects a specific exchange; anything the scenario does not
//! name is left alone, so the default flow (connect, list, call) covers most of
//! them.
//!
//! ```no_rust
//! npx @modelcontextprotocol/conformance client \
//!     --command "$(pwd)/target/debug/conformance-client" --suite core
//! ```

use neva::auth::oauth::{
    AuthorizationHandler, CallbackParams, IdentityAssertion, JwsAlgorithm, OAuthClientConfig,
    PrivateKeyJwt,
};
use neva::prelude::*;
use neva::shared::BoxFuture;
use std::time::Duration;

/// Splits the URL the suite hands us into the `host:port` and path halves the
/// client builder takes separately.
fn split_url(url: &str) -> Result<(String, String), Error> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidParams,
                format!("server URL must be http(s): {url}"),
            )
        })?;
    Ok(match rest.split_once('/') {
        Some((addr, path)) => (addr.to_owned(), format!("/{path}")),
        None => (rest.to_owned(), "/mcp".to_owned()),
    })
}

/// The answer a client's form would submit if the user accepted it untouched:
/// every field at the `default` its schema declares.
///
/// A field without one gets a placeholder, so a form that declares no defaults
/// is still answered rather than left empty.
fn prefilled_answer(params: &ElicitRequestParams) -> serde_json::Value {
    let Some(form) = params.as_form() else {
        return serde_json::json!({});
    };

    let filled = form
        .schema
        .properties
        .iter()
        .map(|(name, schema)| {
            let declared = serde_json::to_value(schema).unwrap_or_default();
            let value = declared
                .get("default")
                .cloned()
                .unwrap_or_else(|| placeholder_for(&declared));
            (name.clone(), value)
        })
        .collect::<serde_json::Map<_, _>>();

    serde_json::Value::Object(filled)
}

/// Something type-appropriate for a field whose schema declares no default.
fn placeholder_for(schema: &serde_json::Value) -> serde_json::Value {
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("integer") | Some("number") => serde_json::json!(1),
        Some("boolean") => serde_json::json!(true),
        Some("array") => serde_json::json!([]),
        // A string, or an enum -- whose first allowed value is the only answer
        // a fixture can give without guessing.
        _ => schema
            .get("enum")
            .and_then(|e| e.as_array())
            .and_then(|values| values.first().cloned())
            .unwrap_or_else(|| serde_json::json!("conformance")),
    }
}

/// Scenario-specific data the suite passes in `MCP_CONFORMANCE_CONTEXT`.
fn context() -> serde_json::Value {
    std::env::var("MCP_CONFORMANCE_CONTEXT")
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// One call the scenario dictates, name and arguments both.
///
/// A scenario that judges what a call puts on the wire cannot let the fixture
/// invent the arguments, so it hands them over in the context and expects them
/// back verbatim.
#[derive(serde::Deserialize)]
struct DictatedCall {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

/// The calls named in `context.toolCalls`, or none if the scenario named any.
fn dictated_calls() -> Vec<DictatedCall> {
    serde_json::from_value(context()["toolCalls"].clone()).unwrap_or_default()
}

/// Where the mock issuer is told to send the authorization response.
///
/// Nothing listens there: [`RedirectReader`] reads the redirect off the
/// response instead of following it, so this only has to be a URL the issuer
/// will accept and register.
const CALLBACK_URI: &str = "http://127.0.0.1:8919/callback";

/// This client's Client ID Metadata Document, standing in for one a real
/// deployment would host.
///
/// The suite hands this over in no scenario context -- `auth/basic-cimd`
/// hardcodes the URL it expects to see as the `client_id` -- so the fixture
/// has to know its own, exactly as a shipped client knows where its document
/// is published. What the scenario judges is whether the client *uses* it when
/// the authorization server advertises `client_id_metadata_document_supported`,
/// and that decision is neva's: every other authorization scenario runs
/// against a server that advertises no such thing, and registers dynamically.
const CLIENT_ID_DOCUMENT: &str = "https://conformance-test.local/client-metadata.json";

/// The credentials a scenario issued out of band, in its context.
///
/// A scenario that hands any of these over is testing a client that was
/// configured with them: registering dynamically instead would be answering a
/// different question. Which ones it hands over is also what says *which
/// grant* it is testing -- a private key or a workload JWT belongs to no other
/// flow -- so the grant is picked from the context rather than hardcoded, the
/// way a real client is configured by its deployer.
struct Credentials {
    client_id: Option<String>,
    client_secret: Option<String>,
    /// PKCS#8 signing key for `private_key_jwt` client authentication.
    private_key_pem: Option<String>,
    signing_algorithm: Option<String>,
    /// A workload JWT to present as an RFC 7523 authorization grant.
    workload_jwt: Option<String>,
    /// The enterprise profile: an ID token, and where to trade it.
    idp_issuer: Option<String>,
    idp_token_endpoint: Option<String>,
    idp_id_token: Option<String>,
    idp_client_id: Option<String>,
    /// Whether the scenario name says this is the client-credentials grant.
    ///
    /// `auth/pre-registration` and `auth/client-credentials-basic` hand over
    /// the identical context -- an id and a secret -- and differ only in which
    /// grant the client is expected to run. Nothing on the wire distinguishes
    /// them before the token request, so the scenario name stands in for the
    /// deployer who would have configured it.
    client_credentials: bool,
}

impl Credentials {
    fn from_context(scenario: &str) -> Self {
        let ctx = context();
        let field = |name: &str| ctx[name].as_str().map(str::to_owned);
        Self {
            client_id: field("client_id"),
            client_secret: field("client_secret"),
            private_key_pem: field("private_key_pem"),
            signing_algorithm: field("signing_algorithm"),
            workload_jwt: field("valid_jwt"),
            idp_issuer: field("idp_issuer"),
            idp_token_endpoint: field("idp_token_endpoint"),
            idp_id_token: field("idp_id_token"),
            idp_client_id: field("idp_client_id"),
            client_credentials: scenario.starts_with("auth/client-credentials"),
        }
    }

    fn apply(&self, mut oauth: OAuthClientConfig) -> Result<OAuthClientConfig, Error> {
        let Some(id) = &self.client_id else {
            // Nothing issued out of band, so the document is what identifies
            // this client -- where the server resolves one, and otherwise it
            // registers. That order is the spec's, and the two are
            // alternatives: configuring both is refused.
            return Ok(oauth.with_client_id_document(CLIENT_ID_DOCUMENT));
        };

        oauth = oauth.with_client_id(id.clone());

        // A signing key and a secret are alternatives, so the key wins where
        // the scenario handed one over -- which is the case the extension
        // RECOMMENDS anyway.
        if let Some(pem) = &self.private_key_pem {
            let algorithm = self.signing_algorithm.as_deref().unwrap_or("ES256");
            // `JwsAlgorithm`'s variant names are the registered `alg` values
            // verbatim, so its derived `Deserialize` is the parser. There is
            // no `FromStr` to reach for.
            let algorithm: JwsAlgorithm = serde_json::from_value(serde_json::Value::String(
                algorithm.to_owned(),
            ))
            .map_err(|err| {
                Error::new(
                    ErrorCode::InvalidParams,
                    format!("unsupported signing algorithm `{algorithm}`: {err}"),
                )
            })?;

            let key = PrivateKeyJwt::from_pem(pem.as_bytes(), algorithm).map_err(|err| {
                Error::new(
                    ErrorCode::InvalidParams,
                    format!("the scenario's signing key is unusable: {err}"),
                )
            })?;

            return Ok(oauth.with_private_key_jwt(key).with_client_credentials());
        }

        if let Some(secret) = &self.client_secret {
            oauth = oauth.with_client_secret(secret.clone());
        }

        // The enterprise profile: sign-on already happened, and the ID token
        // it produced is what buys a token at the MCP server's authorization
        // server -- traded at the IdP first.
        if let (Some(issuer), Some(id_token)) = (&self.idp_issuer, &self.idp_id_token) {
            // The registration this client signed the user in under, which is
            // not the one it holds at the MCP server's authorization server.
            let idp_client_id = self.idp_client_id.as_deref().ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidParams,
                    "the scenario handed over an IdP id token but no `idp_client_id`",
                )
            })?;

            let mut assertion =
                IdentityAssertion::new(issuer.clone(), idp_client_id, id_token.clone())
                    .require_https(false);
            // The scenario names the endpoint, which is what a client that
            // signed the user in there would already hold. The mock IdP
            // publishes an OpenID configuration too, but not a complete
            // RFC 8414 record -- it is an identity provider, not a resource's
            // authorization server.
            if let Some(endpoint) = &self.idp_token_endpoint {
                assertion = assertion.with_token_endpoint(endpoint.clone());
            }

            return Ok(oauth.with_identity_assertion(assertion));
        }

        // Workload identity federation: the platform already minted the
        // credential, and it is the grant.
        if let Some(jwt) = &self.workload_jwt {
            return Ok(oauth.with_jwt_bearer(jwt.clone()));
        }

        if self.client_credentials {
            oauth = oauth.with_client_credentials();
        }

        Ok(oauth)
    }
}

/// The authorization step, without a user or a browser.
///
/// The mock issuer approves on sight: its `GET /authorize` answers `302` with
/// the code already on the redirect URI. So the flow is completed by making
/// that request without following the redirect and reading the query off
/// `Location` -- the same parameters a browser would have delivered to a
/// listener, minus the listener.
struct RedirectReader {
    redirect_uri: String,
}

impl RedirectReader {
    fn new(redirect_uri: impl Into<String>) -> Self {
        Self {
            redirect_uri: redirect_uri.into(),
        }
    }
}

impl AuthorizationHandler for RedirectReader {
    fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>> {
        Box::pin(async move { Ok(self.redirect_uri.clone()) })
    }

    fn authorize(&self, authorization_url: String) -> BoxFuture<'_, Result<CallbackParams, Error>> {
        Box::pin(async move {
            let http = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;

            let resp = http
                .get(&authorization_url)
                .send()
                .await
                .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;

            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InternalError,
                        format!(
                            "the authorization endpoint answered {} without a redirect",
                            resp.status()
                        ),
                    )
                })?;

            let query = location.split_once('?').map(|(_, q)| q).ok_or_else(|| {
                Error::new(
                    ErrorCode::InternalError,
                    "the authorization redirect carried no query",
                )
            })?;

            CallbackParams::from_query(query)
        })
    }
}

/// Two workers, not one per core.
///
/// The suite starts every client scenario at once -- two dozen of these
/// processes plus two dozen mock servers -- so a runtime sized to the machine
/// leaves a small CI runner with far more runnable threads than cores. That
/// costs `sse-retry` in particular: it measures the wall-clock gap between a
/// stream closing and the reconnect, against the 500ms the server asked for
/// with a 200ms late tolerance, and a timer that fires on a starved thread
/// spends that tolerance on scheduling. This fixture opens one connection and
/// makes a handful of calls; it has no use for a work-stealing pool.
///
/// `multi_thread` rather than `current_thread` because `Client::connect` uses
/// `block_in_place`, which the current-thread runtime refuses.
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let url = std::env::args()
        .next_back()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "server URL argument is required"))?;
    let scenario =
        std::env::var("MCP_CONFORMANCE_SCENARIO").unwrap_or_else(|_| "tools_call".into());

    tracing::info!(%scenario, %url, "conformance client starting");
    tracing::debug!(context = %context(), "scenario context");

    let (addr, endpoint) = split_url(&url)?;
    let credentials = Credentials::from_context(&scenario);
    // Applied outside the closure so a scenario handing over an unusable
    // credential fails here, naming it, rather than at the first `401`.
    let oauth = credentials.apply(
        OAuthClientConfig::default()
            // Inert until something answers `401`, which only the
            // authorization scenarios do. `require_https(false)` because
            // every mock issuer here is on loopback http.
            .require_https(false)
            // Not keyed on the scenario, unlike the grant: which credential a
            // client presents *is* on the wire here. The `auth/dpop`
            // scenarios challenge with the `DPoP` scheme and advertise
            // `dpop_signing_alg_values_supported`, and every other scenario
            // does neither -- so this is a client that can do DPoP meeting
            // servers that ask for it, which is the shape a shipped client
            // has.
            .with_dpop_auto()
            .with_handler(RedirectReader::new(CALLBACK_URI)),
    )?;
    let oauth = std::sync::Mutex::new(Some(oauth));
    let mut client = Client::new().with_options(|opt| {
        opt.with_name("neva-conformance-client")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_timeout(Duration::from_secs(20))
            .with_http(|http| {
                http.bind(&addr)
                    .with_endpoint(&endpoint)
                    .with_oauth(|default| {
                        oauth
                            .lock()
                            .ok()
                            .and_then(|mut configured| configured.take())
                            .unwrap_or(default)
                    })
            })
    });

    // Registering the handler is also what declares the capability, and MRTR
    // scenarios only get an input request if the client declared it can answer
    // one. The answer is built from the form's own schema: SEP-1034 has the
    // client prefill each field's declared `default`, which is what a real UI
    // shows the user, so a fixture that answers with something else would not
    // be exercising the requirement.
    client.map_elicitation(|params: ElicitRequestParams| async move {
        ElicitResult::accept().with_content(prefilled_answer(&params))
    });

    client.connect().await?;
    let result = run(&mut client, &scenario).await;
    client.disconnect().await?;
    result
}

/// Drives the exchange the named scenario asserts on.
async fn run(client: &mut Client, scenario: &str) -> Result<(), Error> {
    match scenario {
        // The harness cannot see what a client kept of a schema, so it asks for
        // it back: list the tools, then hand the observed `inputSchema` to the
        // echo tool verbatim. What arrives is what survived the round trip.
        "json-schema-2020-12-preservation" => {
            let tools = client.list_tools(None).await?;
            let observed = tools
                .tools
                .iter()
                .find(|t| t.name == "json_schema_2020_12_tool")
                .map(|t| serde_json::to_value(&t.input_schema))
                .transpose()
                .map_err(Error::from)?
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidParams,
                        "the mock server did not advertise json_schema_2020_12_tool",
                    )
                })?;

            let result = client
                .call_tool("json_schema_echo", Some([("schema", observed)]))
                .await?;
            tracing::info!(?result, "echoed the observed schema back");
        }
        // Calling this tool is what makes the server elicit; the handler
        // registered above answers with the schema's declared defaults, which
        // is the behavior under test.
        "elicitation-sep1034-client-defaults" => {
            let result = client
                .call_tool(
                    "test_client_elicitation_defaults",
                    None::<[(&str, &str); 0]>,
                )
                .await?;
            tracing::info!(?result, "elicitation tool returned");
        }
        // `x-mcp-header` mirroring is judged on the headers a call carries, so
        // the scenario dictates both the tool and its arguments. Listing first
        // is what teaches the client the annotations -- the registry it mirrors
        // from is filled by `tools/list`, so a call issued before one would
        // rightly carry no `Mcp-Param-*` header at all.
        "http-custom-headers" => {
            client.list_tools(None).await?;
            for call in dictated_calls() {
                let result = client.call_tool(&*call.name, call.arguments).await?;
                tracing::info!(tool = %call.name, ?result, "dictated call returned");
            }
        }
        // The scenario serves one well-formed tool among ten with malformed
        // annotations and watches which ones get called. Calling everything
        // that survived `tools/list` states both halves at once: the valid tool
        // is still reachable, and no invalid one is.
        "http-invalid-tool-headers" => {
            let tools = client.list_tools(None).await?;
            let names = tools
                .tools
                .iter()
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>();
            tracing::info!(?names, "tools that survived listing");
            for name in names {
                match client.call_tool(&*name, None::<[(&str, &str); 0]>).await {
                    Ok(result) => tracing::info!(%name, ?result, "tool returned"),
                    Err(err) => tracing::warn!(%name, %err, "tool failed"),
                }
            }
        }
        // The scenario answers this call with an SSE stream, writes a priming
        // frame carrying an event id and a `retry:`, then closes the stream
        // without the response -- and finishes the answer on the stream the
        // client is expected to resume. Nothing is asserted about the result
        // here; the reconnection is the whole subject, so a call that ends in
        // an error has still produced the traffic being judged.
        "sse-retry" => {
            client.list_tools(None).await?;
            match client
                .call_tool("test_reconnection", None::<[(&str, &str); 0]>)
                .await
            {
                Ok(result) => tracing::info!(?result, "resumed and completed"),
                Err(err) => tracing::warn!(%err, "reconnection tool failed"),
            }
        }
        // Step-up: this scenario grants `tools/list` on one scope and
        // `tools/call` on another, so the call is what makes the server ask for
        // more. Listing first is what puts a token in hand for the escalation
        // to widen rather than replace.
        "auth/scope-step-up" => {
            let tools = client.list_tools(None).await?;
            let name = tools
                .tools
                .first()
                .map(|t| t.name.to_string())
                .unwrap_or_else(|| "test-tool".to_string());
            match client.call_tool(&*name, None::<[(&str, &str); 0]>).await {
                Ok(result) => tracing::info!(%name, ?result, "call succeeded after step-up"),
                Err(err) => tracing::warn!(%name, %err, "call failed"),
            }
        }
        // The suite's own fixture server exposes `add_numbers`; list first so
        // the recorded traffic carries both verbs, then call it.
        "tools_call"
        | "request-metadata"
        | "http-standard-headers"
        | "json-schema-ref-no-deref" => {
            let tools = client.list_tools(None).await?;
            tracing::info!(count = tools.tools.len(), "tools listed");
            let result = client
                .call_tool(
                    "add_numbers",
                    Some([("a", serde_json::json!(2)), ("b", serde_json::json!(3))]),
                )
                .await?;
            tracing::info!(?result, "add_numbers returned");
        }
        // The MRTR scenario's mock server checks how a retry is formed: that
        // `requestState` comes back byte-exact, that it is absent when the
        // server sent none, that the retry carries a fresh JSON-RPC id, and
        // that an unrelated call in between carries neither field. Each tool
        // drives one of those, so all four are called -- and the errors are
        // logged rather than propagated, because a tool that answers with a
        // JSON-RPC error has still produced the traffic being judged.
        "sep-2322-client-request-state" => {
            for tool in [
                "test_mrtr_echo_state",
                "test_mrtr_no_state",
                "test_mrtr_unrelated",
                "test_mrtr_no_result_type",
            ] {
                match client.call_tool(tool, None::<[(&str, &str); 0]>).await {
                    Ok(result) => tracing::info!(%tool, ?result, "tool returned"),
                    Err(err) => tracing::warn!(%tool, %err, "tool failed"),
                }
            }
        }
        // Everything else: exercise the read-only surface so the scenario has
        // traffic to inspect without guessing at fixture names.
        _ => {
            let tools = client.list_tools(None).await?;
            tracing::info!(count = tools.tools.len(), "tools listed");
        }
    }
    Ok(())
}
