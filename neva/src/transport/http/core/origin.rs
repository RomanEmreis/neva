//! DNS-rebinding protection: which `Origin` / `Host` a request may claim.
//!
//! A server bound to loopback is reachable by any page the user's browser
//! happens to load: the attacker points `evil.example.com` at `127.0.0.1`, the
//! browser dutifully connects, and without a check the server answers. The
//! request looks local because it *is* local -- what gives it away is the name
//! it was addressed by, which is why the spec makes validating these headers a
//! MUST for local servers.
//!
//! The two headers say different things and both are checked. `Origin` is set
//! by the browser and cannot be forged by page script, so it is the reliable
//! signal when there is one; `Host` is what the request was addressed to, which
//! is what a non-browser caller reaching a rebound name carries. A request with
//! neither is not from a browser and is left alone.

use crate::error::{Error, ErrorCode};
use http::HeaderMap;
use std::sync::Arc;

/// The `Host` values a server accepts, and by extension the origins.
#[derive(Debug, Clone, Default)]
pub(crate) enum OriginPolicy {
    /// Accept only loopback names, on any port.
    ///
    /// The default for a server bound to a loopback address -- exactly the case
    /// the spec makes this a MUST for, and the case where the answer is knowable
    /// without asking the application.
    #[default]
    Loopback,

    /// Accept the listed hosts, plus the loopback ones.
    ///
    /// Entries are matched against the hostname only; a port in an entry is
    /// ignored, since the port a request arrives on says nothing about who sent
    /// it.
    Allowlist(Arc<[Box<str>]>),

    /// Accept anything.
    ///
    /// The default when the server is not bound to loopback: neva cannot know
    /// which names a deployment is legitimately reached by -- behind a proxy the
    /// `Host` is whatever that proxy passes through -- and refusing every one it
    /// cannot verify would break the deployment rather than protect it. Also
    /// what [`HttpServer::allow_any_origin`](crate::transport::http::HttpServer::allow_any_origin)
    /// selects.
    Any,
}

impl OriginPolicy {
    /// The policy a server bound to `addr` gets when the application states
    /// none: enforcing on loopback, permissive anywhere else.
    pub(crate) fn for_addr(addr: &str) -> Self {
        if is_loopback_host(host_of(addr)) {
            Self::Loopback
        } else {
            Self::Any
        }
    }

    /// Why this request's `Origin` / `Host` is not one this server answers to,
    /// if it is not.
    ///
    /// `Origin` is checked first and on its own: a browser that sends one has
    /// told the truth about which page is calling, so a bad origin is a bad
    /// request whatever the `Host` says.
    pub(crate) fn rejection(&self, headers: &HeaderMap) -> Option<Error> {
        if matches!(self, Self::Any) {
            return None;
        }

        let refuse = |header: &str, value: &str| {
            Some(Error::new(
                ErrorCode::InvalidRequest,
                format!("Request rejected: {header} {value:?} is not allowed by this server"),
            ))
        };

        if let Some(origin) = header(headers, "origin") {
            // `null` is the opaque origin a sandboxed frame or a `file://` page
            // sends. It names nothing, so it can never be on the list.
            let host = origin_host(origin).unwrap_or("null");
            if !self.allows(host) {
                return refuse("Origin", origin);
            }
        }

        if let Some(value) = header(headers, "host")
            && !self.allows(host_of(value))
        {
            return refuse("Host", value);
        }

        None
    }

    /// Whether `host` (a hostname, no port) is one this policy accepts.
    fn allows(&self, host: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Loopback => is_loopback_host(host),
            Self::Allowlist(allowed) => {
                is_loopback_host(host)
                    || allowed
                        .iter()
                        .any(|entry| host_of(entry).eq_ignore_ascii_case(host))
            }
        }
    }
}

/// The header value as a string, if it is present and readable.
fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The host part of `scheme://host[:port]`.
fn origin_host(origin: &str) -> Option<&str> {
    let rest = origin.split_once("://")?.1;
    Some(host_of(rest))
}

/// Strips the port from `host[:port]`, keeping a bracketed IPv6 literal whole.
///
/// `[::1]:3000` -> `[::1]`, `localhost:3000` -> `localhost`, and a bare IPv6
/// address with no brackets (which is not valid in a `Host`, but may reach us
/// from a bind string like `::1:3000`) is left alone rather than cut at its
/// first colon.
fn host_of(value: &str) -> &str {
    let value = value.trim();
    if let Some(end) = value.find(']') {
        return &value[..=end];
    }
    match value.split_once(':') {
        // More than one colon and no brackets: an unbracketed IPv6 literal.
        Some(_) if value.matches(':').count() > 1 => value,
        Some((host, _)) => host,
        None => value,
    }
}

/// Whether `host` names the local machine.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        // The whole 127.0.0.0/8 block, not just 127.0.0.1: every address in it
        // routes to this machine, so every one of them is a way in.
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                http::HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn loopback_policy_accepts_local_names_on_any_port() {
        let policy = OriginPolicy::Loopback;
        for host in [
            "localhost:3000",
            "127.0.0.1:8080",
            "[::1]:3000",
            "127.0.0.5",
        ] {
            assert!(
                policy
                    .rejection(&headers(&[
                        ("host", host),
                        ("origin", &format!("http://{host}"))
                    ]))
                    .is_none(),
                "`{host}` must be accepted"
            );
        }
    }

    #[test]
    fn loopback_policy_rejects_a_rebound_name() {
        let policy = OriginPolicy::Loopback;
        let rejected = policy.rejection(&headers(&[
            ("host", "evil.example.com"),
            ("origin", "http://evil.example.com"),
        ]));
        assert!(rejected.is_some(), "a rebound name must be rejected");

        // The Host alone is enough: a non-browser caller sends no Origin.
        assert!(
            policy
                .rejection(&headers(&[("host", "evil.example.com")]))
                .is_some()
        );
        // And so is the Origin alone.
        assert!(
            policy
                .rejection(&headers(&[
                    ("host", "127.0.0.1:3000"),
                    ("origin", "https://evil.example.com")
                ]))
                .is_some()
        );
    }

    #[test]
    fn a_request_naming_nothing_is_left_alone() {
        // curl, an SDK, anything not a browser: no Origin, and often no Host
        // worth checking. There is no rebinding without a name to rebind.
        assert!(
            OriginPolicy::Loopback
                .rejection(&HeaderMap::new())
                .is_none()
        );
    }

    #[test]
    fn the_opaque_origin_is_not_a_local_one() {
        // `Origin: null` is what a sandboxed frame sends. It matches no entry.
        assert!(
            OriginPolicy::Loopback
                .rejection(&headers(&[("origin", "null")]))
                .is_some()
        );
    }

    #[test]
    fn an_allowlist_extends_loopback_rather_than_replacing_it() {
        let policy = OriginPolicy::Allowlist(Arc::from([Box::from("app.example.com")]));

        assert!(
            policy
                .rejection(&headers(&[("origin", "https://app.example.com")]))
                .is_none()
        );
        // A port on the request is not part of the match.
        assert!(
            policy
                .rejection(&headers(&[("host", "app.example.com:8443")]))
                .is_none()
        );
        // Loopback still works, so a local dev client keeps connecting.
        assert!(
            policy
                .rejection(&headers(&[("host", "127.0.0.1:3000")]))
                .is_none()
        );
        assert!(
            policy
                .rejection(&headers(&[("origin", "https://evil.example.com")]))
                .is_some()
        );
    }

    #[test]
    fn any_policy_checks_nothing() {
        assert!(
            OriginPolicy::Any
                .rejection(&headers(&[
                    ("host", "evil.example.com"),
                    ("origin", "http://evil.example.com")
                ]))
                .is_none()
        );
    }

    #[test]
    fn the_default_policy_follows_the_bind_address() {
        assert!(matches!(
            OriginPolicy::for_addr("127.0.0.1:3000"),
            OriginPolicy::Loopback
        ));
        assert!(matches!(
            OriginPolicy::for_addr("localhost:3000"),
            OriginPolicy::Loopback
        ));
        assert!(matches!(
            OriginPolicy::for_addr("[::1]:3000"),
            OriginPolicy::Loopback
        ));
        // Reachable from elsewhere: neva cannot know the names it is legitimately
        // called by, so it does not guess.
        assert!(matches!(
            OriginPolicy::for_addr("0.0.0.0:3000"),
            OriginPolicy::Any
        ));
        assert!(matches!(
            OriginPolicy::for_addr("192.168.1.5:3000"),
            OriginPolicy::Any
        ));
    }

    #[test]
    fn host_of_keeps_ipv6_literals_whole() {
        assert_eq!(host_of("[::1]:3000"), "[::1]");
        assert_eq!(host_of("localhost:3000"), "localhost");
        assert_eq!(host_of("localhost"), "localhost");
        assert_eq!(host_of("::1"), "::1");
    }
}
