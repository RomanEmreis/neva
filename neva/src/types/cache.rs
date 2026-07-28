//! Cacheable results under MCP 2026-07-28.

use serde::{Deserialize, Serialize};

/// Default TTL neva announces on a cacheable result: `0`, i.e. immediately
/// stale.
///
/// The spec makes `ttlMs` mandatory, so a server that has expressed no opinion
/// still has to say something. `0` is the reading that cannot be wrong -- the
/// client may re-fetch every time -- whereas any positive default would invite
/// clients to serve stale data the server never sanctioned.
pub(crate) const DEFAULT_TTL_MS: u64 = 0;

/// Intended scope of a cached response, analogous to HTTP `Cache-Control:
/// public` vs `private`.
///
/// Carried with `ttlMs` on the results the spec marks cacheable
/// ([`DiscoverResult`](crate::types::DiscoverResult) and
/// [`ReadResourceResult`](crate::types::ReadResourceResult)). Both are
/// mandatory members of the result, not optional hints.
///
/// # Examples
///
/// ```
/// use neva::types::CacheScope;
///
/// // The safe default: never shared across authorization contexts.
/// assert_eq!(CacheScope::default(), CacheScope::Private);
/// ```
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CacheScope {
    /// The response carries no user-specific data: any client or intermediary
    /// may cache it and serve it across authorization contexts.
    Public,

    /// The response may be reused only within the same authorization context.
    /// Caches must not be shared across contexts -- a different access token
    /// requires a different cache.
    #[default]
    Private,
}

#[cfg(test)]
mod tests {
    use super::CacheScope;
    use serde_json::json;

    #[test]
    fn roundtrips_each_variant() {
        for (v, s) in [
            (CacheScope::Public, "public"),
            (CacheScope::Private, "private"),
        ] {
            let j = serde_json::to_value(v).unwrap();
            assert_eq!(j, json!(s));
            let back: CacheScope = serde_json::from_value(j).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn defaults_to_private() {
        // Defaulting to `public` would let an intermediary serve one user's
        // result to another; that mistake must not be the silent one.
        assert_eq!(CacheScope::default(), CacheScope::Private);
    }
}
