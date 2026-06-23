//! Cache hints for list-results under MCP 2026-07-28.

use serde::{Deserialize, Serialize};

/// Suggested scope at which a list-result may be cached.
///
/// Carried alongside `ttl_ms` on list-result structs (tools, prompts,
/// resources, resource templates) under `proto-2026-07-28-rc`. The
/// server announces the scope; the client (or any caching middleware)
/// decides whether to honor it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CacheScope {
    /// Cache is valid for the lifetime of the MCP session.
    Session,
    /// Cache is valid for the lifetime of the underlying transport connection.
    Connection,
    /// Cache is valid for the lifetime of this specific client.
    Client,
}

#[cfg(test)]
mod tests {
    use super::CacheScope;
    use serde_json::json;

    #[test]
    fn roundtrips_each_variant() {
        for (v, s) in [
            (CacheScope::Session, "session"),
            (CacheScope::Connection, "connection"),
            (CacheScope::Client, "client"),
        ] {
            let j = serde_json::to_value(v).unwrap();
            assert_eq!(j, json!(s));
            let back: CacheScope = serde_json::from_value(j).unwrap();
            assert_eq!(back, v);
        }
    }
}
