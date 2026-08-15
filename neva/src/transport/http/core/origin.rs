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

    /// Accept the listed origins, plus the loopback ones.
    ///
    /// An entry that states a scheme (`https://app.example.com`) is an origin
    /// and is matched as one: scheme, host and port all have to agree, with a
    /// missing port meaning the scheme's default. An entry that states none
    /// (`app.example.com`) is a host, and matches that host on any scheme and
    /// any port -- narrowed to one port if the entry names one.
    ///
    /// `Host` is matched by hostname either way. It says where the request
    /// landed rather than who sent it, it carries no scheme, and behind a proxy
    /// its port is the proxy's business.
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
        if binds_to_loopback(addr) {
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

        if let Some(origin) = header(headers, "origin")
            && !self.allows_origin(origin)
        {
            return refuse("Origin", origin);
        }

        if let Some(value) = header(headers, "host")
            && !self.allows_host(host_of(value))
        {
            return refuse("Host", value);
        }

        None
    }

    /// Whether `origin` -- a whole `scheme://host[:port]` -- is one this policy
    /// answers to.
    ///
    /// Judged as an origin rather than as the hostname inside it, because that
    /// is what it is: `https://app.example.com` and `http://app.example.com:8080`
    /// are two security origins that happen to share a name, and a page on the
    /// second is not the application the first one is. Reducing both to
    /// `app.example.com` would let whatever else that host serves -- a staging
    /// build, a user-content app, anything on a spare port -- issue
    /// state-changing requests here.
    fn allows_origin(&self, origin: &str) -> bool {
        if matches!(self, Self::Any) {
            return true;
        }

        // `null` is the opaque origin a sandboxed frame or a `file://` page
        // sends, and anything else unparseable names nothing either. Neither can
        // be on a list of origins.
        let Some((scheme, host, port)) = split_origin(origin) else {
            return false;
        };

        // A loopback origin is a page the user is genuinely running locally, on
        // whatever port it chose. Rebinding is about names that resolve *back*
        // to this machine from outside, which this is not.
        if is_loopback_host(host) {
            return true;
        }

        match self {
            Self::Any | Self::Loopback => false,
            Self::Allowlist(allowed) => allowed
                .iter()
                .any(|entry| entry_allows_origin(entry, scheme, host, port)),
        }
    }

    /// Whether `host` (a hostname, no port) is one this policy answers to.
    fn allows_host(&self, host: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Loopback => is_loopback_host(host),
            Self::Allowlist(allowed) => {
                is_loopback_host(host)
                    || allowed
                        .iter()
                        .any(|entry| entry_host(entry).eq_ignore_ascii_case(host))
            }
        }
    }
}

/// The host an allowlist entry names, in whichever form it was written.
///
/// The scheme has to come off first: `host_of` cuts at the first colon, so a
/// full origin would answer `https` -- a name no `Host` ever carries, which
/// would refuse every request made from the very origin that was allowed.
fn entry_host(entry: &str) -> &str {
    host_of(entry.split_once("://").map_or(entry, |(_, rest)| rest))
}

/// Whether an allowlist entry covers the origin `scheme://host[:port]`.
///
/// An entry naming a scheme is an origin and has to agree on all three parts.
/// An entry naming none is a host, and the caller has said nothing about scheme
/// or port -- so neither is held against the request, beyond a port the entry
/// does state.
fn entry_allows_origin(entry: &str, scheme: &str, host: &str, port: Option<&str>) -> bool {
    match entry.split_once("://") {
        Some((entry_scheme, rest)) => {
            entry_scheme.eq_ignore_ascii_case(scheme)
                && host_of(rest).eq_ignore_ascii_case(host)
                && stated_port(entry_scheme, port_of(rest)) == stated_port(scheme, port)
        }
        None => {
            // Against the *effective* port: a browser sends
            // `https://app.example.com` with the `:443` left implicit, and an
            // entry that spelled it out means the same place.
            host_of(entry).eq_ignore_ascii_case(host)
                && port_of(entry)
                    .is_none_or(|entry_port| Some(entry_port) == stated_port(scheme, port))
        }
    }
}

/// The port an origin is really on: the one it states, or its scheme's default.
///
/// `https://x` and `https://x:443` are the same origin, and a list that spelled
/// one must not miss the other.
fn stated_port<'a>(scheme: &str, port: Option<&'a str>) -> Option<&'a str> {
    if port.is_some() {
        return port;
    }

    // `eq_ignore_ascii_case` rather than lowercasing: this runs per allowlist
    // entry per request, and the rest of the file compares schemes and hosts
    // the same way -- without building a `String` to throw away.
    if scheme.eq_ignore_ascii_case("http") {
        Some("80")
    } else if scheme.eq_ignore_ascii_case("https") {
        Some("443")
    } else {
        None
    }
}

/// The header value as a string, if it is present and readable.
fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// `scheme://host[:port]` taken apart, or `None` when it is not one -- `null`
/// among them.
fn split_origin(origin: &str) -> Option<(&str, &str, Option<&str>)> {
    let (scheme, rest) = origin.trim().split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }

    // A serialized origin is scheme, host and port and nothing else -- no
    // userinfo, no path. `host_of` cuts at the first colon, so it would read
    // `app.example.com:8443@evil.com` as the host `app.example.com`: the name
    // in front of the `@` is the credential, and the host is what follows it.
    // A browser never sends this, but a hand-rolled split is exactly where a
    // value that only looks like an origin gets to pass as one.
    if rest.contains(['@', '/']) {
        return None;
    }

    Some((scheme, host_of(rest), port_of(rest)))
}

/// The port in `host[:port]`, if it states one. An IPv6 literal's own colons do
/// not count; only what follows its closing bracket.
fn port_of(value: &str) -> Option<&str> {
    let value = value.trim();
    let rest = match value.find(']') {
        Some(end) => &value[end + 1..],
        // Unbracketed and multi-colon: an IPv6 literal, all host and no port.
        None if value.matches(':').count() > 1 => return None,
        None => value,
    };
    rest.split_once(':')
        .map(|(_, port)| port)
        .filter(|port| !port.is_empty())
}

/// Strips the port from `host[:port]`, keeping a bracketed IPv6 literal whole.
///
/// `[::1]:3000` -> `[::1]`, `localhost:3000` -> `localhost`, and a bare IPv6
/// address with no brackets is left alone rather than cut at its first colon.
///
/// This reads header grammar -- `Host`, and the authority inside an `Origin` --
/// where an unbracketed literal is all host and states no port. A bind string
/// is the other grammar, where the last colon *is* the port separator, and it
/// goes to [`binds_to_loopback`] instead; running one through the other is what
/// made `bind("::1:3000")` look like it was not on loopback.
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

/// Whether a bind string lands on the local machine.
///
/// A bind string is a socket address, not a `Host` header, and it is read here
/// the way `std` reads it on the way to `bind` -- because that is what decides
/// which interface the server ends up on. Reading it with [`host_of`] instead
/// got `::1:3000` wrong in the direction that matters: `std` takes it as the
/// address `::1` on port 3000 (loopback), while `host_of` leaves it whole and
/// it parses as the *different*, non-loopback address `::1:3000`. The server
/// bound to loopback and the protection this module exists for was off.
///
/// What this has to agree with is the engine, since the engine is what calls
/// `bind`. Every engine neva ships follows the `std` grammar, so `std` is the
/// thing to read the string like; an engine that read it differently would put
/// the server on an interface this function did not predict.
///
/// One case it answers conservatively: a *name* that resolves to loopback
/// (`myapp.test` in `/etc/hosts`) is not recognised as one, because deciding
/// that needs a resolver and this runs while the server is being built. Such a
/// server gets [`OriginPolicy::Any`] and states its hosts with
/// [`HttpServer::with_allowed_origins`](crate::transport::http::HttpServer::with_allowed_origins).
fn binds_to_loopback(addr: &str) -> bool {
    // The bracketed forms `std` parses outright: `127.0.0.1:3000`, `[::1]:3000`.
    if let Ok(socket) = addr.parse::<std::net::SocketAddr>() {
        return socket.ip().is_loopback();
    }

    // Everything else `std` splits at the *last* colon and resolves the front
    // half -- a name (`localhost:3000`) or an unbracketed literal (`::1:3000`).
    let host = addr.rsplit_once(':').map_or(addr, |(host, _)| host);

    is_loopback_host(host)
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
    use std::net::ToSocketAddrs;

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

    /// An entry that names a scheme is an origin, and only that origin gets in.
    ///
    /// A hostname is not an origin: `https://app.example.com` and
    /// `http://app.example.com:8080` are two security principals sharing a name,
    /// and whatever else that host serves -- staging, user content, a spare port
    /// -- must not inherit the trust the application was given.
    #[test]
    fn a_listed_origin_is_matched_whole() {
        let policy = OriginPolicy::Allowlist(Arc::from([Box::from("https://app.example.com")]));

        // Both headers, because that is what a browser sends -- and a `Host`
        // never carries a scheme. Checking the `Origin` alone would miss an
        // entry that clears one gate and is refused at the other.
        let allows = |origin: &str| {
            policy
                .rejection(&headers(&[("origin", origin), ("host", "app.example.com")]))
                .is_none()
        };

        assert!(allows("https://app.example.com"));
        // The default port is the same origin spelled out.
        assert!(allows("https://app.example.com:443"));

        assert!(!allows("http://app.example.com"), "another scheme");
        assert!(!allows("http://app.example.com:8080"), "another port");
        assert!(!allows("https://app.example.com:8443"), "another port");
        assert!(!allows("https://other.example.com"), "another host");
    }

    /// An entry that names no scheme is a host, and says nothing about scheme or
    /// port -- so neither is held against the request. It is the shape the
    /// documented example used, and it keeps meaning what it meant.
    #[test]
    fn a_listed_host_covers_the_schemes_it_did_not_name() {
        let policy = OriginPolicy::Allowlist(Arc::from([Box::from("app.example.com")]));
        for origin in [
            "https://app.example.com",
            "http://app.example.com",
            "http://app.example.com:8080",
        ] {
            assert!(
                policy.rejection(&headers(&[("origin", origin)])).is_none(),
                "`{origin}` must be accepted"
            );
        }

        // Stating a port narrows it to that port, still on any scheme.
        let pinned = OriginPolicy::Allowlist(Arc::from([Box::from("app.example.com:8443")]));
        assert!(
            pinned
                .rejection(&headers(&[("origin", "https://app.example.com:8443")]))
                .is_none()
        );
        assert!(
            pinned
                .rejection(&headers(&[("origin", "https://app.example.com")]))
                .is_some()
        );

        // A pinned default port is the port the browser leaves implicit, and
        // the two spellings name one place.
        let default_port = OriginPolicy::Allowlist(Arc::from([Box::from("app.example.com:443")]));
        assert!(
            default_port
                .rejection(&headers(&[("origin", "https://app.example.com")]))
                .is_none(),
            "an implicit :443 is the :443 the entry pinned"
        );
        assert!(
            default_port
                .rejection(&headers(&[("origin", "http://app.example.com")]))
                .is_some(),
            "but plain HTTP is port 80, which the entry did not pin"
        );
        // `Host` is matched by name either way: it says where the request
        // landed, not who sent it, and behind a proxy its port is the proxy's.
        assert!(
            pinned
                .rejection(&headers(&[("host", "app.example.com:443")]))
                .is_none()
        );
    }

    #[test]
    fn an_origin_is_taken_apart_at_the_right_colons() {
        assert_eq!(
            split_origin("https://app.example.com:8443"),
            Some(("https", "app.example.com", Some("8443")))
        );
        assert_eq!(
            split_origin("http://[::1]:3000"),
            Some(("http", "[::1]", Some("3000")))
        );
        assert_eq!(split_origin("http://[::1]"), Some(("http", "[::1]", None)));
        // The opaque origin, and other things that are not origins.
        assert_eq!(split_origin("null"), None);
        assert_eq!(split_origin("app.example.com"), None);
        assert_eq!(split_origin("https://"), None);
        // Userinfo makes the name in front of the `@` the credential, not the
        // host -- so a value shaped like this is not an origin at all, and must
        // not be matched as one against `app.example.com`.
        assert_eq!(split_origin("https://app.example.com:8443@evil.com"), None);
        assert_eq!(split_origin("https://app.example.com/path"), None);
    }

    /// A value that only looks like an allowlisted origin does not get in.
    #[test]
    fn an_origin_carrying_userinfo_is_not_the_host_it_names_first() {
        let policy = OriginPolicy::Allowlist(Arc::from([Box::from("app.example.com")]));
        assert!(
            policy
                .rejection(&headers(&[(
                    "origin",
                    "https://app.example.com:8443@evil.com"
                )]))
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

    /// `std` accepts an unbracketed IPv6 bind string and takes the last colon
    /// as the port separator, so `bind("::1:3000")` really does listen on
    /// `[::1]:3000`. Read whole instead, it parses as the address `::1:3000`
    /// -- a different one, and not loopback -- which left a loopback server
    /// with rebinding protection switched off.
    #[test]
    fn an_unbracketed_ipv6_bind_address_is_still_loopback() {
        let bound = "::1:3000"
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
            .expect("`std` accepts this bind string");
        assert!(
            bound.ip().is_loopback(),
            "the premise: `{bound}` is what the server actually binds to"
        );
        assert!(matches!(
            OriginPolicy::for_addr("::1:3000"),
            OriginPolicy::Loopback
        ));

        // `[::]` is the unspecified address -- every interface, and not a
        // case for the loopback policy.
        assert!(matches!(
            OriginPolicy::for_addr("[::]:3000"),
            OriginPolicy::Any
        ));

        // `::1` with no port is not an address at all: the last colon is the
        // port separator, which leaves the host `:`, and nothing resolves
        // that. The server does not start, so the permissive answer here is
        // one nothing ever reads -- it just must not be the one that claims
        // loopback.
        assert!("::1".to_socket_addrs().is_err());
        assert!(matches!(OriginPolicy::for_addr("::1"), OriginPolicy::Any));
    }

    #[test]
    fn host_of_keeps_ipv6_literals_whole() {
        assert_eq!(host_of("[::1]:3000"), "[::1]");
        assert_eq!(host_of("localhost:3000"), "localhost");
        assert_eq!(host_of("localhost"), "localhost");
        assert_eq!(host_of("::1"), "::1");
    }
}
