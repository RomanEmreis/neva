//! Engine-neutral authorization validators.
//!
//! These helpers enforce the `with_roles` / `with_permissions` /
//! `with_scopes` rules attached to tools, prompts, and resources. They operate on neva's
//! own [`Claims`] trait so any HTTP engine -- Volga, axum, hyper, a
//! custom adapter -- can opt in by implementing the trait for its claims
//! type. The Volga adapter re-uses these same validators so behavior is
//! identical across engines.

use super::types::Claims;
use crate::app::options::RuntimeMcpOptions;
use crate::error::{Error, ErrorCode};
use crate::types::{Request, Uri};
use std::sync::Arc;
use tokio::sync::SetOnce;

const ERR_NO_CLAIMS: &str = "Claims are not provided";
const ERR_UNAUTHORIZED: &str = "Subject is not authorized to invoke this";

/// What a caller's [`Claims`] must hold to use a tool, a prompt or a resource.
///
/// One value per item, filled by that item's `with_roles` /
/// `with_permissions` / `with_scopes` and checked by [`Self::validate`]. Empty
/// means unrestricted.
#[derive(Debug, Clone, Default)]
pub(crate) struct RequiredClaims {
    /// Roles allowed in; the caller must hold at least one.
    roles: Option<Vec<String>>,
    /// Permissions allowed in; the caller must hold at least one.
    permissions: Option<Vec<String>>,
    /// Scopes the caller's token must grant; every one of them.
    scopes: Vec<String>,
}

impl RequiredClaims {
    /// Replaces the roles allowed in.
    #[inline]
    pub(crate) fn set_roles<T, I>(&mut self, roles: T)
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.roles = Some(roles.into_iter().map(Into::into).collect());
    }

    /// Replaces the permissions allowed in.
    #[inline]
    pub(crate) fn set_permissions<T, I>(&mut self, permissions: T)
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.permissions = Some(permissions.into_iter().map(Into::into).collect());
    }

    /// Replaces the scopes the caller's token must grant.
    #[inline]
    pub(crate) fn set_scopes<T, I>(&mut self, scopes: T)
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.scopes = scopes.into_iter().map(Into::into).collect();
    }

    /// The scopes the caller's token must grant; empty when there are none.
    #[inline]
    pub(crate) fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// Whether `claims` satisfy the roles, the permissions and the scopes.
    ///
    /// Checked in that order. A missing role or permission is a decision
    /// about the caller, which no new token changes, so it has to be what the
    /// caller hears about first: answering it with
    /// [`ErrorCode::InsufficientScope`] would send a client off to
    /// re-authorize for a call that is refused again afterwards.
    #[inline]
    pub(crate) fn validate(&self, claims: Option<&dyn Claims>) -> Result<(), Error> {
        validate_roles(claims, self.roles.as_deref())?;
        validate_permissions(claims, self.permissions.as_deref())?;
        validate_scopes(claims, &self.scopes)
    }
}

/// Validates the scopes an access token grants against the ones required.
///
/// Every required scope must be granted: a scope is a grant, so a call that
/// needs `memory:read` and `memory:write` needs both. That sets it apart from
/// roles and permissions, where any one is enough.
///
/// Without claims at all there is no token to widen, so that is the
/// "claims missing" error a role gate answers as well. A token short of some
/// scopes gets [`ErrorCode::InsufficientScope`], with `requiredScopes` and
/// `missingScopes` in its `data`: what the HTTP transport turns into a `403`
/// step-up challenge.
#[inline]
pub(crate) fn validate_scopes(
    claims: Option<&dyn Claims>,
    required: &[String],
) -> Result<(), Error> {
    if required.is_empty() {
        return Ok(());
    }

    let granted = claims.ok_or_else(claims_missing)?.scopes();
    let missing = required
        .iter()
        .filter(|scope| !granted.contains(scope))
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return Ok(());
    }

    let message = format!(
        "The access token does not grant the scope this requires: `{}`",
        missing
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );

    Err(
        Error::new(ErrorCode::InsufficientScope, message).with_data(serde_json::json!({
            "requiredScopes": required,
            "missingScopes": missing,
        })),
    )
}

/// The scopes an [`ErrorCode::InsufficientScope`] error says its call
/// requires: what a step-up challenge asks for.
///
/// Read from the error's `data`, which is all a reply carries once it has
/// left the handler. Empty when the error names none -- a handler that built
/// one by hand without the `data`.
pub(crate) fn required_scopes_of(err: &crate::types::ErrorDetails) -> Vec<String> {
    err.data
        .as_ref()
        .and_then(|data| data.get("requiredScopes"))
        .and_then(|scopes| scopes.as_array())
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(|scope| scope.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The tools, prompts and resources of the server, as the HTTP transport
/// reads them to check a request's scopes before it is dispatched.
///
/// The check has to come first because the shape of the reply does: a
/// request that opts into notifications is answered with `200` and an event
/// stream as soon as it is dispatched, and a batch shares one status. Either
/// way, a refusal reached in the dispatch arrives too late to be the `403`
/// a client steps up on.
///
/// Bound once the server's runtime is built, which is after the transport
/// starts: the runtime answers through the transport's sender. So the
/// transport does not run its engine until the gate is bound -- see
/// [`Self::bound`] -- and no request reaches an unbound one. A gate that is
/// never bound, as in a test that drives the handlers directly, checks
/// nothing, and the requirement the dispatch checks is the one that refuses.
#[derive(Clone, Default)]
pub(crate) struct ScopeGate {
    options: Arc<SetOnce<RuntimeMcpOptions>>,
}

impl std::fmt::Debug for ScopeGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopeGate")
            .field("bound", &self.options.get().is_some())
            .finish()
    }
}

/// A request refused for want of scope.
#[derive(Debug)]
pub(crate) struct ScopeRefusal {
    /// What the request is answered with: [`ErrorCode::InsufficientScope`].
    pub(crate) error: Error,
    /// Every scope the request's tool, prompt or resource requires: what a
    /// step-up has to ask for.
    pub(crate) scopes: Vec<String>,
}

impl ScopeGate {
    /// Binds the gate to the server's registry. Once only: a second call is
    /// a no-op.
    #[inline]
    pub(crate) fn bind(&self, options: RuntimeMcpOptions) {
        let _ = self.options.set(options);
    }

    /// Waits until the gate is bound.
    ///
    /// What the transport waits on before its engine takes a request. One let
    /// in earlier would get past the gate unchecked, and be refused in the
    /// dispatch instead: in-band, after a streamed reply has had its `200`
    /// or a batch its one status, and so with no `403` to step up on.
    ///
    /// Cancel safe.
    #[inline]
    pub(crate) async fn bound(&self) {
        self.options.wait().await;
    }

    /// Why `req` is refused for want of scope, if it is.
    ///
    /// Only a scope refusal is answered here. A missing role or permission,
    /// or no claims at all, is left for the dispatch to refuse as it always
    /// has: none of those is something a new token fixes. So is an item that
    /// does not exist, or a request that names none.
    pub(crate) async fn refusal(
        &self,
        req: &Request,
        claims: Option<&dyn Claims>,
    ) -> Option<ScopeRefusal> {
        let options = self.options.get()?;
        let param = |key: &str| {
            req.params
                .as_ref()
                .and_then(|params| params.get(key))
                .and_then(serde_json::Value::as_str)
        };

        let refusal = |required: &RequiredClaims| refusal_of(required, claims);

        match req.method.as_str() {
            crate::types::tool::commands::CALL => {
                options
                    .with_tool_requirement(param("name")?, refusal)
                    .await?
            }
            crate::types::prompt::commands::GET => {
                options
                    .with_prompt_requirement(param("name")?, refusal)
                    .await?
            }
            crate::types::resource::commands::READ => {
                options.with_resource_requirement(&Uri::from(param("uri")?), refusal)?
            }
            _ => None,
        }
    }
}

/// The scope refusal `required` answers `claims` with, if it refuses them for
/// want of scope rather than for anything else.
fn refusal_of(required: &RequiredClaims, claims: Option<&dyn Claims>) -> Option<ScopeRefusal> {
    if required.scopes().is_empty() {
        return None;
    }

    match required.validate(claims) {
        Err(error) if error.code == ErrorCode::InsufficientScope => Some(ScopeRefusal {
            error,
            scopes: required.scopes().to_vec(),
        }),
        _ => None,
    }
}

/// Validates JWT claims against required permissions.
///
/// Returns `Ok(())` if `required` is `None` or empty, if any of the
/// subject's permissions match a required one. Returns an unauthorized
/// error otherwise, or a "claims missing" error if required is set but
/// `claims` is `None`.
///
/// Accepts `Option<&dyn Claims>` so the same validator runs against any
/// engine-supplied claims type -- Volga's `DefaultClaims` or a custom
/// claims struct from an axum / hyper adapter.
#[inline]
pub(crate) fn validate_permissions(
    claims: Option<&dyn Claims>,
    required: Option<&[String]>,
) -> Result<(), Error> {
    required.map_or(Ok(()), |req| {
        let claims = claims.ok_or_else(claims_missing)?;
        contains_any(claims.permissions(), req)
            .then_some(())
            .ok_or_else(unauthorized)
    })
}

/// Validates JWT claims against required roles.
///
/// Returns `Ok(())` if `required` is `None`, or if the subject's `role`
/// or any of `roles` matches a required role.
///
/// Accepts `Option<&dyn Claims>` so the same validator runs against any
/// engine-supplied claims type.
#[inline]
pub(crate) fn validate_roles(
    claims: Option<&dyn Claims>,
    required: Option<&[String]>,
) -> Result<(), Error> {
    required.map_or(Ok(()), |req| {
        let claims = claims.ok_or_else(claims_missing)?;
        (contains(claims.role(), req) || contains_any(claims.roles(), req))
            .then_some(())
            .ok_or_else(unauthorized)
    })
}

#[inline]
fn contains_any(have: Option<&[String]>, required: &[String]) -> bool {
    have.is_some_and(|vals| vals.iter().any(|v| required.contains(v)))
}

#[inline]
fn contains(have: Option<&str>, required: &[String]) -> bool {
    have.is_some_and(|val| required.iter().any(|r| r == val))
}

#[inline]
fn unauthorized() -> Error {
    Error::new(ErrorCode::InvalidParams, ERR_UNAUTHORIZED)
}

#[inline]
fn claims_missing() -> Error {
    Error::new(ErrorCode::InvalidParams, ERR_NO_CLAIMS)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod required_claims {
        use super::*;

        #[test]
        fn nothing_required_lets_anyone_in() {
            assert!(RequiredClaims::default().validate(None).is_ok());
        }

        #[test]
        fn roles_and_permissions_must_both_hold() {
            let mut required = RequiredClaims::default();
            required.set_roles(["admin"]);
            required.set_permissions(["read"]);

            let admin = TestClaims {
                role: Some("admin".into()),
                ..Default::default()
            };
            let reader = TestClaims {
                permissions: Some(vec!["read".into()]),
                ..Default::default()
            };
            let both = TestClaims {
                role: Some("admin".into()),
                permissions: Some(vec!["read".into()]),
                ..Default::default()
            };

            assert!(required.validate(None).is_err());
            assert!(required.validate(Some(&admin)).is_err());
            assert!(required.validate(Some(&reader)).is_err());
            assert!(required.validate(Some(&both)).is_ok());
        }

        /// A scope is a grant: a call that needs two needs both, which is
        /// the opposite of roles and permissions.
        #[test]
        fn every_scope_must_be_granted() {
            let mut required = RequiredClaims::default();
            required.set_scopes(["memory:read", "memory:write"]);

            let reader = TestClaims {
                scopes: vec!["memory:read".into()],
                ..Default::default()
            };
            let both = TestClaims {
                scopes: vec!["memory:write".into(), "memory:read".into(), "x".into()],
                ..Default::default()
            };

            let err = required.validate(Some(&reader)).unwrap_err();
            assert_eq!(err.code, ErrorCode::InsufficientScope);
            assert_eq!(
                err.data(),
                Some(&serde_json::json!({
                    "requiredScopes": ["memory:read", "memory:write"],
                    "missingScopes": ["memory:write"],
                }))
            );
            assert!(required.validate(Some(&both)).is_ok());
        }

        /// No claims means no token to widen: that is not a step-up.
        #[test]
        fn a_scope_without_claims_is_the_claims_missing_error() {
            let mut required = RequiredClaims::default();
            required.set_scopes(["memory:read"]);

            let err = required.validate(None).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert_eq!(err.to_string(), ERR_NO_CLAIMS);
        }

        /// A missing role cannot be fixed by re-authorizing, so it must not
        /// read as `insufficient_scope` when the scopes are short as well.
        #[test]
        fn a_missing_role_outranks_a_missing_scope() {
            let mut required = RequiredClaims::default();
            required.set_roles(["admin"]);
            required.set_scopes(["memory:write"]);

            let err = required.validate(Some(&TestClaims::default())).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert_eq!(err.to_string(), ERR_UNAUTHORIZED);
        }

        #[test]
        fn an_empty_scope_list_requires_nothing() {
            let mut required = RequiredClaims::default();
            required.set_scopes(Vec::<String>::new());
            assert!(required.validate(None).is_ok());
        }

        #[test]
        fn the_required_scopes_are_read_back_off_the_reply() {
            let mut required = RequiredClaims::default();
            required.set_scopes(["a", "b"]);
            let err = required.validate(Some(&TestClaims::default())).unwrap_err();

            let details = crate::types::ErrorDetails::from(err);
            assert_eq!(required_scopes_of(&details), ["a", "b"]);

            let bare = crate::types::ErrorDetails::from(Error::new(
                ErrorCode::InsufficientScope,
                "no data",
            ));
            assert!(required_scopes_of(&bare).is_empty());
        }
    }

    #[derive(Default, Debug)]
    struct TestClaims {
        role: Option<String>,
        roles: Option<Vec<String>>,
        permissions: Option<Vec<String>>,
        scopes: Vec<String>,
    }

    impl Claims for TestClaims {
        fn scopes(&self) -> &[String] {
            &self.scopes
        }
        fn role(&self) -> Option<&str> {
            self.role.as_deref()
        }
        fn roles(&self) -> Option<&[String]> {
            self.roles.as_deref()
        }
        fn permissions(&self) -> Option<&[String]> {
            self.permissions.as_deref()
        }
    }

    #[test]
    fn no_required_permissions_passes_without_claims() {
        let r = validate_permissions(None, None);
        assert!(r.is_ok());
    }

    #[test]
    fn required_permissions_without_claims_fails() {
        let req = vec!["read".into()];
        let r = validate_permissions(None, Some(&req));
        assert!(r.is_err());
    }

    #[test]
    fn required_permissions_with_matching_claim_passes() {
        let req = vec!["read".into(), "write".into()];
        let claims = TestClaims {
            permissions: Some(vec!["read".into()]),
            ..Default::default()
        };
        assert!(validate_permissions(Some(&claims as &dyn Claims), Some(&req)).is_ok());
    }

    #[test]
    fn required_permissions_without_matching_claim_fails() {
        let req = vec!["admin".into()];
        let claims = TestClaims {
            permissions: Some(vec!["read".into()]),
            ..Default::default()
        };
        assert!(validate_permissions(Some(&claims as &dyn Claims), Some(&req)).is_err());
    }

    #[test]
    fn required_roles_match_via_single_role() {
        let req = vec!["admin".into()];
        let claims = TestClaims {
            role: Some("admin".into()),
            ..Default::default()
        };
        assert!(validate_roles(Some(&claims as &dyn Claims), Some(&req)).is_ok());
    }

    #[test]
    fn required_roles_match_via_roles_list() {
        let req = vec!["admin".into()];
        let claims = TestClaims {
            roles: Some(vec!["user".into(), "admin".into()]),
            ..Default::default()
        };
        assert!(validate_roles(Some(&claims as &dyn Claims), Some(&req)).is_ok());
    }

    #[test]
    fn required_roles_without_match_fails() {
        let req = vec!["admin".into()];
        let claims = TestClaims {
            role: Some("user".into()),
            ..Default::default()
        };
        assert!(validate_roles(Some(&claims as &dyn Claims), Some(&req)).is_err());
    }

    /// Verify that two different `Claims`-implementing types both flow
    /// through the same dyn-Claims validator -- this is the engine
    /// neutrality contract.
    #[test]
    fn validator_accepts_heterogeneous_claims_types() {
        #[derive(Debug)]
        struct AltClaims;
        impl Claims for AltClaims {
            fn role(&self) -> Option<&str> {
                Some("admin")
            }
        }

        let req = vec!["admin".into()];
        let alt: AltClaims = AltClaims;
        let test = TestClaims {
            role: Some("admin".into()),
            ..Default::default()
        };
        assert!(validate_roles(Some(&alt as &dyn Claims), Some(&req)).is_ok());
        assert!(validate_roles(Some(&test as &dyn Claims), Some(&req)).is_ok());
    }
}
