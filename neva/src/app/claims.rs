//! Who is calling: the claims of the caller's access token.

use crate::shared::OneOrMany;
use serde::{Deserialize, Deserializer};
use std::any::Any;
use std::fmt::Debug;

/// The claims of the caller's access token, decoded and validated by the
/// HTTP engine before the request reaches the server.
///
/// A handler reads them with [`Context::claims`](crate::Context::claims),
/// and the `roles` / `permissions` gates on tools, prompts and resources check
/// them before a handler runs. Every method has a default, so a claims type
/// answers only for the claims it carries.
///
/// [`DefaultClaims`] covers the registered JWT claims and the ones RFC 9068
/// adds for access tokens. An engine that decodes a type of its own implements
/// this trait for it, and a handler gets the concrete type back with
/// [`downcast_ref`](#method.downcast_ref).
///
/// `Debug` is required so that a `Request` holding an `Arc<dyn Claims>` can
/// derive it, and `Any` is what makes the downcast possible. Neither costs an
/// implementor anything: every `'static` type is `Any` already.
///
/// # Engine contract
///
/// An `HttpEngine` adapter that decodes bearer tokens wraps the claims in
/// `Arc<dyn Claims>` and inserts them into the inbound request's extensions
/// before calling the `dispatch_post` helper:
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use neva::auth::Claims;
///
/// // in the engine's POST route, after decoding the bearer token:
/// let claims: Arc<dyn Claims> = Arc::new(my_decoded_claims);
/// neutral_req.extensions_mut().insert(claims);
/// ```
///
/// # Examples
///
/// ```
/// use neva::auth::Claims;
///
/// #[derive(Debug)]
/// struct MyClaims {
///     sub: String,
///     role: String,
/// }
///
/// impl Claims for MyClaims {
///     fn subject(&self) -> Option<&str> {
///         Some(&self.sub)
///     }
///
///     fn role(&self) -> Option<&str> {
///         Some(&self.role)
///     }
/// }
/// ```
pub trait Claims: Any + Debug + Send + Sync + 'static {
    /// The `sub` claim: who the token was issued for.
    ///
    /// A subject is unique only within its [`issuer`](Self::issuer)
    /// (RFC 7519 section 4.1.2), so data kept per caller is keyed by both.
    /// Under MCP 2026-07-28 the two bind a `requestState` to the caller it
    /// was minted for.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     sub: Some("alice".into()),
    ///     ..Default::default()
    /// };
    /// assert_eq!(claims.subject(), Some("alice"));
    /// ```
    fn subject(&self) -> Option<&str> {
        None
    }

    /// The `iss` claim: who issued the token.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     iss: Some("https://auth.example.com".into()),
    ///     ..Default::default()
    /// };
    /// assert_eq!(claims.issuer(), Some("https://auth.example.com"));
    /// ```
    fn issuer(&self) -> Option<&str> {
        None
    }

    /// The `aud` claim: who the token is meant for.
    ///
    /// A token names one audience or several (RFC 7519 section 4.1.3); this
    /// is empty when it names none.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     aud: vec!["https://mcp.example.com/mcp".into()],
    ///     ..Default::default()
    /// };
    /// assert_eq!(claims.audience(), ["https://mcp.example.com/mcp"]);
    /// ```
    fn audience(&self) -> &[String] {
        &[]
    }

    /// The `client_id` claim (RFC 9068 section 2.2): the OAuth client acting
    /// for the subject, e.g. the agent that made the call.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     client_id: Some("my-agent".into()),
    ///     ..Default::default()
    /// };
    /// assert_eq!(claims.client_id(), Some("my-agent"));
    /// ```
    fn client_id(&self) -> Option<&str> {
        None
    }

    /// The `scope` claim (RFC 9068 section 2.2.3): the scopes granted, one
    /// per element; empty when the token carries none.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     scope: vec!["memory:read".into(), "memory:write".into()],
    ///     ..Default::default()
    /// };
    /// assert!(claims.scopes().iter().any(|s| s == "memory:write"));
    /// ```
    fn scopes(&self) -> &[String] {
        &[]
    }

    /// Single role for this subject, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     role: Some("admin".into()),
    ///     ..Default::default()
    /// };
    /// assert_eq!(claims.role(), Some("admin"));
    /// ```
    fn role(&self) -> Option<&str> {
        None
    }

    /// Multiple roles for this subject, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     roles: Some(vec!["admin".into()]),
    ///     ..Default::default()
    /// };
    /// assert_eq!(claims.roles(), Some(&["admin".to_string()][..]));
    /// ```
    fn roles(&self) -> Option<&[String]> {
        None
    }

    /// Permission set for this subject, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// let claims = DefaultClaims {
    ///     permissions: Some(vec!["read".into()]),
    ///     ..Default::default()
    /// };
    /// assert_eq!(claims.permissions(), Some(&["read".to_string()][..]));
    /// ```
    fn permissions(&self) -> Option<&[String]> {
        None
    }
}

impl dyn Claims {
    /// The concrete claims type behind this trait object, if it is `T`.
    ///
    /// What an engine decodes into a type of its own -- a tenant, a custom
    /// claim -- is read through this, past what the [`Claims`] methods
    /// expose.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::auth::{Claims, DefaultClaims};
    ///
    /// #[derive(Debug)]
    /// struct TenantClaims {
    ///     tenant: String,
    /// }
    ///
    /// impl Claims for TenantClaims {}
    ///
    /// let claims: &dyn Claims = &TenantClaims { tenant: "acme".into() };
    /// assert_eq!(
    ///     claims.downcast_ref::<TenantClaims>().map(|c| c.tenant.as_str()),
    ///     Some("acme")
    /// );
    /// assert!(claims.downcast_ref::<DefaultClaims>().is_none());
    /// ```
    #[inline]
    pub fn downcast_ref<T: Claims>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
}

/// Engine-agnostic pre-built [`Claims`] type matching the JWT standard
/// claim names. Available for every HTTP engine -- under the Volga
/// adapter it also implements `volga::auth::AuthClaims` so it can be
/// fed straight into Volga's bearer-auth pipeline.
///
/// # Examples
///
/// ```
/// use neva::auth::DefaultClaims;
///
/// let claims: DefaultClaims = serde_json::from_value(serde_json::json!({
///     "sub": "alice",
///     "iss": "https://auth.example.com",
///     "aud": ["https://mcp.example.com/mcp", "https://api.example.com"],
///     "scope": "memory:read memory:write"
/// }))
/// .unwrap();
///
/// assert_eq!(claims.aud.len(), 2);
/// assert_eq!(claims.scope, ["memory:read", "memory:write"]);
/// ```
#[derive(Default, Clone, Debug, Deserialize)]
pub struct DefaultClaims {
    /// JWT `sub` claim -- subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// JWT `iss` claim -- issuer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// JWT `aud` claim -- audience. A token carries one string or an array
    /// of them; either lands here, and an absent claim leaves it empty.
    #[serde(default, deserialize_with = "one_or_many")]
    pub aud: Vec<String>,
    /// JWT `exp` claim -- expiration time (seconds since epoch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    /// JWT `nbf` claim -- not-before time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<i64>,
    /// JWT `iat` claim -- issued-at time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<i64>,
    /// JWT `jti` claim -- token id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// `client_id` claim (RFC 9068 section 2.2) -- the OAuth client acting
    /// for the subject.
    pub client_id: Option<String>,
    /// `scope` claim (RFC 9068 section 2.2.3) -- the granted scopes. A
    /// space-delimited string in the token, split here; an array is taken
    /// as it is.
    #[serde(default, deserialize_with = "space_delimited")]
    pub scope: Vec<String>,
    /// Subject role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Subject roles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
    /// Subject permissions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
}

impl Claims for DefaultClaims {
    #[inline]
    fn subject(&self) -> Option<&str> {
        self.sub.as_deref()
    }

    #[inline]
    fn issuer(&self) -> Option<&str> {
        self.iss.as_deref()
    }

    #[inline]
    fn audience(&self) -> &[String] {
        &self.aud
    }

    #[inline]
    fn client_id(&self) -> Option<&str> {
        self.client_id.as_deref()
    }

    #[inline]
    fn scopes(&self) -> &[String] {
        &self.scope
    }

    #[inline]
    fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    #[inline]
    fn roles(&self) -> Option<&[String]> {
        self.roles.as_deref()
    }

    #[inline]
    fn permissions(&self) -> Option<&[String]> {
        self.permissions.as_deref()
    }
}

/// `aud`: one audience or several (RFC 7519 section 4.1.3). `null` is none.
fn one_or_many<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<OneOrMany<String>>::deserialize(de)?
        .map(OneOrMany::into_vec)
        .unwrap_or_default())
}

/// `scope`: space-delimited scope tokens (RFC 6749 section 3.3), or an array
/// from an issuer that sends one. `null` is none.
fn space_delimited<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<String>, D::Error> {
    Ok(match Option::<OneOrMany<String>>::deserialize(de)? {
        None => Vec::new(),
        Some(OneOrMany::One(scope)) => scope.split_ascii_whitespace().map(Into::into).collect(),
        Some(OneOrMany::Many(scopes)) => scopes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(payload: serde_json::Value) -> DefaultClaims {
        serde_json::from_value(payload).expect("claims")
    }

    #[test]
    fn audience_is_one_string() {
        let claims = decode(json!({ "aud": "https://mcp.example.com" }));

        assert_eq!(claims.audience(), ["https://mcp.example.com"]);
    }

    #[test]
    fn audience_is_an_array() {
        // RFC 8707: a token minted for several resources names each of them.
        let claims = decode(json!({ "aud": ["https://a.example.com", "https://b.example.com"] }));

        assert_eq!(
            claims.audience(),
            ["https://a.example.com", "https://b.example.com"]
        );
    }

    #[test]
    fn absent_or_null_audience_is_empty() {
        assert!(decode(json!({})).audience().is_empty());
        assert!(decode(json!({ "aud": null })).audience().is_empty());
    }

    #[test]
    fn audience_of_another_shape_is_refused() {
        let err = serde_json::from_value::<DefaultClaims>(json!({ "aud": 42 }));

        assert!(err.is_err());
    }

    #[test]
    fn scope_is_split_on_spaces() {
        let claims = decode(json!({ "scope": "memory:read  memory:write " }));

        assert_eq!(claims.scopes(), ["memory:read", "memory:write"]);
    }

    #[test]
    fn scope_array_is_taken_as_it_is() {
        let claims = decode(json!({ "scope": ["memory:read", "memory:write"] }));

        assert_eq!(claims.scopes(), ["memory:read", "memory:write"]);
    }

    #[test]
    fn absent_or_empty_scope_is_empty() {
        assert!(decode(json!({})).scopes().is_empty());
        assert!(decode(json!({ "scope": "" })).scopes().is_empty());
        assert!(decode(json!({ "scope": null })).scopes().is_empty());
    }

    #[test]
    fn default_claims_answer_for_what_they_carry() {
        let claims = decode(json!({
            "sub": "alice",
            "iss": "https://auth.example.com",
            "client_id": "my-agent",
            "role": "admin",
            "roles": ["reader"],
            "permissions": ["read"]
        }));

        assert_eq!(claims.subject(), Some("alice"));
        assert_eq!(claims.issuer(), Some("https://auth.example.com"));
        assert_eq!(claims.client_id(), Some("my-agent"));
        assert_eq!(claims.role(), Some("admin"));
        assert_eq!(claims.roles(), Some(&["reader".to_string()][..]));
        assert_eq!(claims.permissions(), Some(&["read".to_string()][..]));
    }

    #[test]
    fn a_claims_type_answers_none_for_what_it_does_not_carry() {
        #[derive(Debug)]
        struct Bare;
        impl Claims for Bare {}

        let claims: &dyn Claims = &Bare;

        assert_eq!(claims.subject(), None);
        assert_eq!(claims.issuer(), None);
        assert!(claims.audience().is_empty());
        assert_eq!(claims.client_id(), None);
        assert!(claims.scopes().is_empty());
    }

    #[test]
    fn downcast_finds_the_concrete_type() {
        #[derive(Debug)]
        struct TenantClaims {
            tenant: &'static str,
        }
        impl Claims for TenantClaims {}

        let claims: std::sync::Arc<dyn Claims> =
            std::sync::Arc::new(TenantClaims { tenant: "acme" });

        assert_eq!(
            claims.downcast_ref::<TenantClaims>().map(|c| c.tenant),
            Some("acme")
        );
        assert!(claims.downcast_ref::<DefaultClaims>().is_none());
    }
}
