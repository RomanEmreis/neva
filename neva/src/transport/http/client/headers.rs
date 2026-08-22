//! What this transport reads out of, and writes into, HTTP headers.
//!
//! Two unrelated jobs that happen to share a medium. Going out, a `tools/call`
//! mirrors designated arguments into `Mcp-Param-*` headers so an intermediary
//! can route on them without parsing the body -- and mirrors nothing at all
//! from a tool listing that has gone stale, since the annotations that named
//! them may no longer be the server's. Coming back, a `401`/`403` carries a
//! `WWW-Authenticate` challenge that says which authorization server to talk to
//! and which scope was insufficient.

// Only the `Mcp-Param-*` half needs anything from the transport; the challenge
// parsers below work off `reqwest`'s own header types.
#[cfg(not(feature = "legacy-spec"))]
use super::{McpSession, Message};

#[cfg(not(feature = "legacy-spec"))]
pub(super) fn routing_hints(msg: &Message) -> Option<(&str, Option<String>)> {
    match msg {
        Message::Request(r) => Some((r.method.as_str(), name_param(r))),
        Message::Notification(n) => Some((n.method.as_str(), None)),
        Message::Batch(_) | Message::Response(_) => None,
    }
}

/// The `Mcp-Name` value for `req`, already header-encoded.
///
/// The spec requires the header on `tools/call`, `resources/read` and
/// `prompts/get` (sourced from `params.name` / `params.uri`); the Tasks
/// extension adds `params.taskId` on its own methods so an intermediary can
/// route every call for a task to the instance holding its state.
#[cfg(not(feature = "legacy-spec"))]
pub(super) fn name_param(req: &crate::types::Request) -> Option<String> {
    #[cfg(feature = "tasks")]
    {
        use crate::types::task::commands as tasks;
        if matches!(
            req.method.as_str(),
            tasks::GET | tasks::UPDATE | tasks::CANCEL
        ) {
            let raw = req.params.as_ref()?.as_object()?.get("taskId")?.as_str()?;
            return Some(crate::transport::http::encode_header_value(raw));
        }
    }

    let field = match req.method.as_str() {
        crate::types::tool::commands::CALL | crate::types::prompt::commands::GET => "name",
        crate::types::resource::commands::READ => "uri",
        _ => return None,
    };

    let raw = req.params.as_ref()?.as_object()?.get(field)?.as_str()?;

    Some(crate::transport::http::encode_header_value(raw))
}

/// The `Mcp-Param-*` headers a `tools/call` mirrors, per the called tool's
/// registered `x-mcp-header` annotations.
///
/// A batch mirrors nothing, for the same reason it carries no `Mcp-Method` or
/// `Mcp-Name`: one set of headers cannot describe several calls, and two
/// batched calls of the same tool would fight over one header name. Batching an
/// annotated call therefore hides it from header-based routing -- the servers
/// on the other end skip the matching check rather than reject it -- so a
/// caller that needs an intermediary to see a call should send it on its own.
#[cfg(not(feature = "legacy-spec"))]
pub(super) fn param_headers(
    msg: &Message,
    registry: &crate::shared::param_headers::Registry,
) -> Vec<(String, String)> {
    let Message::Request(req) = msg else {
        return Vec::new();
    };
    if req.method != crate::types::tool::commands::CALL {
        return Vec::new();
    }
    let Some(params) = req.params.as_ref().and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
        return Vec::new();
    };
    let Some(entry) = registry.get(name) else {
        return Vec::new();
    };
    // Nothing is mirrored from a listing that has gone stale: the schema that
    // declared these annotations may no longer be the server's.
    let Some(headers) = entry.usable() else {
        return Vec::new();
    };

    let args = params.get("arguments").cloned().unwrap_or_default();
    crate::shared::param_headers::extract(headers, &args)
}

/// The `Mcp-Param-*` headers a request mirrors -- read once per exchange.
///
/// Once, because reading can *spend* something. A listing fetched to recover
/// from a `HeaderMismatch` is good for exactly one call, and an exchange builds
/// its `POST` more than once whenever a managed-OAuth `401` sends it back
/// through authorization. Reading again there would find the grace gone and the
/// listing stale, so the retry -- the very call the recovery was for -- would go
/// out without the headers the server refused it for, and be refused again.
#[cfg(not(feature = "legacy-spec"))]
pub(super) fn mirrored_param_headers(
    session: &McpSession,
    req: &Message,
    registry: &crate::shared::param_headers::Registry,
) -> Vec<(String, String)> {
    // A legacy peer never negotiated these, so asking would spend a grace on a
    // request that is not going to carry them.
    if session.is_legacy() {
        return Vec::new();
    }

    param_headers(req, registry)
}

/// Whether a `403` is the authorization server's `insufficient_scope`, and so
/// something a wider grant would fix.
///
/// RFC 6750 puts the code in the `WWW-Authenticate` challenge; a `403` without
/// one is the resource server refusing the caller, not the token.
///
/// The challenge is parsed rather than searched. `insufficient_scope` is a
/// value of the `error` parameter, and the same bytes appear in places that
/// mean the opposite of it: an `error_description` explaining the code, or a
/// scope name that merely contains it. Reading those as the error would send a
/// caller through an interactive flow -- replacing a perfectly good token -- to
/// retry a request that re-authorization was never going to fix.
///
/// The question is asked of [`bearer_challenge`], which is also what the flow
/// is handed -- so what decides a step-up and what is acted on cannot be two
/// different challenges.
#[cfg(feature = "client-oauth")]
pub(super) fn insufficient_scope(headers: &reqwest::header::HeaderMap) -> bool {
    use volga_oauth_client::OAuthErrorCode;

    bearer_challenge(headers)
        .and_then(|challenge| super::oauth::parse_challenge(&challenge))
        .is_some_and(|challenge| {
            matches!(challenge.error(), Some(OAuthErrorCode::InsufficientScope))
        })
}

/// Whether this response is the `use_dpop_nonce` refusal that asks for one
/// retry (RFC 9449 sections 8 and 9).
///
/// The `DPoP-Nonce` header alone does not say so: a server may supply a nonce
/// with any response, including a successful one, and that nonce is for the
/// *next* request rather than a demand to repeat this one. What makes it a
/// refusal is the `error` in the challenge.
///
/// *Every* challenge is asked, not the one [`bearer_challenge`] picks. A
/// resource that takes both schemes answers with both (RFC 9449 section 7.1),
/// so `Bearer error="invalid_token", DPoP error="use_dpop_nonce"` is an
/// ordinary thing to receive -- and the challenge selected for authorization
/// is the Bearer one, whose error says nothing about the nonce. Reading only
/// that would send the client through a whole authorization flow, user
/// interaction included, to answer a server that had simply not handed out its
/// nonce yet.
#[cfg(feature = "client-oauth-dpop")]
pub(super) fn use_dpop_nonce(headers: &reqwest::header::HeaderMap) -> bool {
    use volga_oauth_client::OAuthErrorCode;

    headers
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(bearer_challenges)
        .filter_map(|challenge| super::oauth::parse_challenge(&challenge))
        .any(|challenge| matches!(challenge.error(), Some(OAuthErrorCode::UseDpopNonce)))
}

/// The `WWW-Authenticate` value carrying the Bearer challenge that applies, if
/// any.
///
/// `WWW-Authenticate` may be sent more than once, and one value may carry
/// several challenges -- including several *Bearer* ones, which RFC 9110 allows
/// and a server distinguishing realms produces. Both are walked:
/// [`bearer_challenges`] takes one value apart, and this takes them all.
///
/// Among them the one naming `insufficient_scope` wins, wherever it sits. It is
/// the only error a client can answer with anything beyond authenticating again,
/// and answering it takes what that challenge carries -- the `scope` the request
/// was short of. Handing the flow whichever came first, a
/// `Bearer error="invalid_token"` say, would have it re-authorize for exactly the
/// grant it already held and spend the exchange's one retry being refused
/// identically.
///
/// Where none names the code, a `DPoP` challenge wins over a `Bearer` one, and
/// otherwise the first is as good as any. What these challenges carry is the
/// same either way -- a `resource_metadata` pointer is the server's own and
/// does not depend on which challenge it accompanies -- so the scheme is the
/// one thing that differs between them and the one thing the caller acts on:
/// it is what tells a session allowed to decide for itself to bind its tokens
/// to a key (RFC 9449 section 7.1). A resource that takes both schemes answers
/// with both, and handing over the `Bearer` one because it was written first
/// would have the client authorize for, and then present, exactly the
/// credential the resource asked it to replace -- an authorization server that
/// leaves `dpop_signing_alg_values_supported` out, as it may, then has nothing
/// left to say so either.
///
/// A challenge naming `insufficient_scope` still outranks that, and needs to:
/// the credential it refused was *accepted as a credential*, so the scheme is
/// already settled, and what is unresolved is the grant.
#[cfg(feature = "client-oauth")]
pub(super) fn bearer_challenge(headers: &reqwest::header::HeaderMap) -> Option<String> {
    use volga_oauth_client::OAuthErrorCode;

    // The challenge to fall back on, and whether it is one of the preferred
    // scheme -- which is what decides whether a later one may displace it.
    let mut fallback: Option<(String, bool)> = None;

    for value in headers
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
    {
        for challenge in bearer_challenges(value) {
            let Some(parsed) = super::oauth::parse_challenge(&challenge) else {
                continue;
            };
            if matches!(parsed.error(), Some(OAuthErrorCode::InsufficientScope)) {
                return Some(challenge);
            }

            #[cfg(feature = "client-oauth-dpop")]
            let preferred = parsed
                .scheme()
                .eq_ignore_ascii_case(volga_oauth_client::auth_scheme::DPOP);
            #[cfg(not(feature = "client-oauth-dpop"))]
            let preferred = false;

            match fallback {
                // Nothing held yet, or a plain challenge giving way to one
                // that names the scheme this client has more to offer under.
                None => fallback = Some((challenge, preferred)),
                Some((_, false)) if preferred => fallback = Some((challenge, preferred)),
                Some(_) => {}
            }
        }
    }

    fallback.map(|(challenge, _)| challenge)
}

/// The challenges inside one `WWW-Authenticate` value that this client can
/// answer, each rendered on its own.
///
/// `BearerChallenge::parse` returns the *first* Bearer challenge in a value and
/// stops where the next scheme begins, which is the right contract for reading
/// one challenge and the wrong one for finding the applicable challenge among
/// several. Iterating header values does not help: RFC 9110 section 11.6.1 lets a
/// server put the whole list in one value, so
/// `Bearer error="invalid_token", Bearer error="insufficient_scope", scope="admin"`
/// is a single value whose second challenge is the one that matters.
///
/// Splitting is by challenge boundary, not by comma: a list element begins a new
/// challenge when its first token is not `name=value` (RFC 9110 section 11.1),
/// and commas inside a quoted string separate nothing. Parameters are left to
/// `BearerChallenge::parse`, which each rendered challenge is handed whole.
#[cfg(feature = "client-oauth")]
pub(super) fn bearer_challenges(value: &str) -> Vec<String> {
    let mut groups: Vec<Vec<&str>> = Vec::new();
    for element in list_elements(value) {
        // A parameter is `token BWS "=" BWS value` (RFC 9110 section 11.2), so
        // what tells it from a scheme is whether an `=` follows the first token
        // -- not whether whitespace precedes the `=`, which that grammar allows.
        // Reading `scope = "admin"` as a scheme would break the challenge in two
        // and leave the step-up asking for nothing.
        let rest = element.trim_start();
        let token_end = rest
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(rest.len());

        let starts_challenge = !rest[token_end..].trim_start().starts_with('=');
        if starts_challenge {
            groups.push(vec![element]);
        } else if let Some(group) = groups.last_mut() {
            group.push(element);
        }
    }

    groups
        .into_iter()
        .filter(|group| {
            group[0]
                .split_ascii_whitespace()
                .next()
                .is_some_and(is_understood_scheme)
        })
        .map(|group| group.join(", "))
        .collect()
}

/// Whether a challenge in `scheme` is one this client can answer.
///
/// `Bearer` always. `DPoP` only where the client can sign a proof: a build
/// without that support reading a DPoP challenge would discover the resource's
/// metadata, run a whole flow, and present a bearer token the server already
/// said it does not take -- so the challenge is left unanswered instead, which
/// is what an unsupported scheme means (RFC 9110 section 11.6.1).
#[cfg(feature = "client-oauth")]
fn is_understood_scheme(scheme: &str) -> bool {
    if scheme.eq_ignore_ascii_case(volga_oauth_client::auth_scheme::BEARER) {
        return true;
    }

    #[cfg(feature = "client-oauth-dpop")]
    return scheme.eq_ignore_ascii_case(volga_oauth_client::auth_scheme::DPOP);

    #[cfg(not(feature = "client-oauth-dpop"))]
    false
}

/// Splits a header value on the commas that separate list elements -- the ones
/// outside a quoted string, since a quoted `scope="a,b"` carries its own.
#[cfg(feature = "client-oauth")]
pub(super) fn list_elements(value: &str) -> Vec<&str> {
    let mut elements = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                elements.push(value[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    elements.push(value[start..].trim());
    elements.retain(|element| !element.is_empty());
    elements
}
