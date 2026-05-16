//! Engine-neutral authorization validators.
//!
//! These helpers enforce the `with_roles` / `with_permissions` rules
//! attached to tools, prompts, and resources. They operate on neva's
//! own [`Claims`] trait so any HTTP engine — Volga, axum, hyper, a
//! custom adapter — can opt in by implementing the trait for its claims
//! type. The Volga adapter re-uses these same validators so behavior is
//! identical across engines.

use super::types::Claims;
use crate::error::{Error, ErrorCode};

const ERR_NO_CLAIMS: &str = "Claims are not provided";
const ERR_UNAUTHORIZED: &str = "Subject is not authorized to invoke this";

/// Validates JWT claims against required permissions.
///
/// Returns `Ok(())` if `required` is `None` or empty, if any of the
/// subject's permissions match a required one. Returns an unauthorized
/// error otherwise, or a "claims missing" error if required is set but
/// `claims` is `None`.
///
/// Accepts `Option<&dyn Claims>` so the same validator runs against any
/// engine-supplied claims type — Volga's `DefaultClaims` or a custom
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

    #[derive(Default, Debug)]
    struct TestClaims {
        role: Option<String>,
        roles: Option<Vec<String>>,
        permissions: Option<Vec<String>>,
    }

    impl Claims for TestClaims {
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
    /// through the same dyn-Claims validator — this is the engine
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
