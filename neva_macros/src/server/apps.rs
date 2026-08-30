//! MCP Apps parameters for the `#[tool]` and `#[resource]` macros.
//!
//! Everything here runs at expansion time, where the attribute's literals are
//! still in hand. That is the point: a `ui://` typo, a misspelled CSP key or a
//! MIME type no host will render are all mistakes this catches as a
//! `compile_error!` rather than as a resource a host 404s on in production.

use super::get_str_param;
use proc_macro2::TokenStream;
use quote::quote;
use syn::Expr;

/// The URI scheme the specification reserves for MCP Apps resources.
const UI_SCHEME: &str = "ui://";

/// The MIME type every `ui://` resource is served under.
const APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Top-level keys of an MCP Apps resource `_meta.ui` block.
const UI_META_KEYS: [&str; 4] = ["csp", "permissions", "domain", "prefersBorder"];

/// Keys of the `csp` object.
const CSP_KEYS: [&str; 4] = [
    "connectDomains",
    "resourceDomains",
    "frameDomains",
    "baseUriDomains",
];

/// Keys of the `permissions` object.
const PERMISSION_KEYS: [&str; 4] = ["camera", "microphone", "geolocation", "clipboardWrite"];

/// The `visibility` scopes a tool may name.
const VISIBILITY_SCOPES: [&str; 2] = ["model", "app"];

/// Whether `uri` addresses an MCP Apps resource.
pub(super) fn is_ui_uri(uri: &str) -> bool {
    uri.starts_with(UI_SCHEME)
}

/// `#[tool(ui = "ui://...")]` -> `.with_ui("ui://...")`.
pub(super) fn tool_ui_code(value: &Expr) -> syn::Result<TokenStream> {
    require_apps_feature(value, "ui")?;

    let uri = get_str_param(value)
        .ok_or_else(|| syn::Error::new_spanned(value, "`ui` must be a string literal"))?;

    if !is_ui_uri(&uri) {
        return Err(syn::Error::new_spanned(
            value,
            format!(
                "`ui` must be a `{UI_SCHEME}` URI, got `{uri}`: the scheme is what marks a \
                 resource as an MCP App, and it is reserved for them"
            ),
        ));
    }

    Ok(quote! { .with_ui(#uri) })
}

/// `#[tool(visibility = ["model", "app"])]` -> `.with_visibility([..])`.
pub(super) fn tool_visibility_code(value: &Expr) -> syn::Result<TokenStream> {
    require_apps_feature(value, "visibility")?;

    let scopes = super::get_params_arr(value).ok_or_else(|| {
        syn::Error::new_spanned(
            value,
            "`visibility` must be a string literal or an array of them, e.g. \
             `visibility = [\"model\", \"app\"]`",
        )
    })?;

    let variants = scopes
        .iter()
        .map(|scope| match scope.as_str() {
            "model" => Ok(quote! { neva::types::UiVisibility::Model }),
            "app" => Ok(quote! { neva::types::UiVisibility::App }),
            other => Err(syn::Error::new_spanned(
                value,
                format!(
                    "unknown visibility scope `{other}`, expected one of: {}",
                    VISIBILITY_SCOPES.join(", ")
                ),
            )),
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! { .with_visibility([#(#variants),*]) })
}

/// `#[resource(ui_meta = r#"{ ... }"#)]` -> `.with_ui(UiResourceMeta::from_json_str(..))`.
///
/// `uri` is the resource's own URI: the block only means anything on a `ui://`
/// resource, so putting one anywhere else is a mistake rather than a no-op.
pub(super) fn resource_ui_meta_code(value: &Expr, uri: Option<&str>) -> syn::Result<TokenStream> {
    require_apps_feature(value, "ui_meta")?;

    let json = get_str_param(value)
        .ok_or_else(|| syn::Error::new_spanned(value, "`ui_meta` must be a string literal"))?;

    if let Some(uri) = uri
        && !is_ui_uri(uri)
    {
        return Err(syn::Error::new_spanned(
            value,
            format!(
                "`ui_meta` is MCP Apps metadata and only means anything on a `{UI_SCHEME}` \
                 resource, but `uri` is `{uri}`; hosts ignore the block anywhere else"
            ),
        ));
    }

    validate_ui_meta(&json, value)?;

    Ok(quote! {
        .with_ui(neva::types::UiResourceMeta::from_json_str(#json).unwrap_or_default())
    })
}

/// The MIME type a `#[resource]` should publish, given its URI and whatever
/// `mime` the attribute asked for.
///
/// A `ui://` resource is served as [`APP_MIME_TYPE`] and nothing else -- a host
/// will not render one under any other type -- so the scheme supplies the
/// default and an explicit disagreement is an error rather than a silent
/// override.
pub(super) fn resolve_mime(
    uri: Option<&str>,
    mime: Option<(String, Expr)>,
) -> syn::Result<Option<String>> {
    let is_app = uri.is_some_and(is_ui_uri);
    match (is_app, mime) {
        (false, mime) => Ok(mime.map(|(mime, _)| mime)),
        (true, None) => Ok(Some(APP_MIME_TYPE.to_string())),
        (true, Some((mime, _))) if mime == APP_MIME_TYPE => Ok(Some(mime)),
        (true, Some((mime, spanned))) => Err(syn::Error::new_spanned(
            spanned,
            format!(
                "a `{UI_SCHEME}` resource is served as `{APP_MIME_TYPE}`, got `{mime}`: no host \
                 renders an MCP App under another type. Drop `mime` to take the default, or use \
                 a different URI scheme."
            ),
        )),
    }
}

/// Checks an `ui_meta` blob at expansion time: well-formed JSON, an object, and
/// only keys the specification defines.
///
/// The key check is the part that earns its keep. `_meta` is an open map, so a
/// `prefers_border` written in snake case, or a `connect_domains` inside `csp`,
/// serializes happily and is then ignored by every host -- a security block that
/// silently does nothing. Here it is a compile error.
fn validate_ui_meta(json: &str, spanned: &Expr) -> syn::Result<()> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| syn::Error::new_spanned(spanned, format!("invalid JSON in `ui_meta`: {e}")))?;

    let serde_json::Value::Object(map) = &value else {
        return Err(syn::Error::new_spanned(
            spanned,
            "`ui_meta` must be a JSON object, e.g. `{ \"prefersBorder\": true }`",
        ));
    };

    check_keys(map, &UI_META_KEYS, "ui_meta", spanned)?;

    if let Some(serde_json::Value::Object(csp)) = map.get("csp") {
        check_keys(csp, &CSP_KEYS, "ui_meta.csp", spanned)?;
    }

    if let Some(serde_json::Value::Object(permissions)) = map.get("permissions") {
        check_keys(
            permissions,
            &PERMISSION_KEYS,
            "ui_meta.permissions",
            spanned,
        )?;
    }

    Ok(())
}

/// Errors on the first key of `map` that is not in `known`.
fn check_keys(
    map: &serde_json::Map<String, serde_json::Value>,
    known: &[&str],
    path: &str,
    spanned: &Expr,
) -> syn::Result<()> {
    match map.keys().find(|key| !known.contains(&key.as_str())) {
        None => Ok(()),
        Some(unknown) => Err(syn::Error::new_spanned(
            spanned,
            format!(
                "unknown key `{unknown}` in `{path}`, expected one of: {}. Keys are camelCase on \
                 the wire, and a host ignores anything it does not know -- so a typo here is a \
                 block that silently does nothing. For metadata the specification does not \
                 define, set `_meta` yourself.",
                known.join(", ")
            ),
        )),
    }
}

/// Refuses an MCP Apps attribute when neva was built without the `apps` feature.
///
/// Without this the generated `.with_ui(..)` would fail as a missing method on
/// `Tool`, pointing at code the user never wrote.
#[cfg_attr(feature = "apps", allow(unused_variables))]
fn require_apps_feature(spanned: &Expr, field: &str) -> syn::Result<()> {
    #[cfg(feature = "apps")]
    {
        Ok(())
    }
    #[cfg(not(feature = "apps"))]
    {
        Err(syn::Error::new_spanned(
            spanned,
            format!("`{field}` is an MCP Apps attribute; enable neva's `apps` feature to use it"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(tokens: proc_macro2::TokenStream) -> Expr {
        syn::parse2(tokens).expect("a valid expression")
    }

    #[test]
    fn the_scheme_marks_an_app_resource() {
        assert!(is_ui_uri("ui://weather/dashboard"));
        assert!(is_ui_uri("ui://report/{id}"));
        assert!(!is_ui_uri("res://weather/dashboard"));
    }

    #[test]
    fn a_ui_uri_supplies_the_app_mime_type() {
        assert_eq!(
            resolve_mime(Some("ui://clock/app.html"), None).unwrap(),
            Some(APP_MIME_TYPE.to_string())
        );
    }

    #[test]
    fn a_plain_uri_keeps_whatever_mime_it_was_given() {
        let mime = ("text/plain".to_string(), expr(quote! { "text/plain" }));

        assert_eq!(
            resolve_mime(Some("res://notes/{id}"), Some(mime)).unwrap(),
            Some("text/plain".to_string())
        );
        assert_eq!(resolve_mime(Some("res://notes/{id}"), None).unwrap(), None);
    }

    #[test]
    fn a_ui_uri_under_another_mime_type_is_refused() {
        let mime = ("text/html".to_string(), expr(quote! { "text/html" }));

        let err = resolve_mime(Some("ui://clock/app.html"), Some(mime))
            .expect_err("no host renders an app under plain text/html");

        assert!(err.to_string().contains(APP_MIME_TYPE));
    }

    #[test]
    fn restating_the_app_mime_type_is_allowed() {
        let mime = (APP_MIME_TYPE.to_string(), expr(quote! { "x" }));

        assert!(resolve_mime(Some("ui://clock/app.html"), Some(mime)).is_ok());
    }

    #[cfg(feature = "apps")]
    mod apps_enabled {
        use super::*;

        #[test]
        fn a_tool_ui_must_use_the_reserved_scheme() {
            let err = tool_ui_code(&expr(quote! { "res://weather/dashboard" }))
                .expect_err("only ui:// marks an app");

            assert!(err.to_string().contains("ui://"));
        }

        #[test]
        fn a_tool_ui_accepts_a_ui_uri() {
            assert!(tool_ui_code(&expr(quote! { "ui://weather/dashboard" })).is_ok());
        }

        #[test]
        fn visibility_scopes_are_checked() {
            assert!(tool_visibility_code(&expr(quote! { ["model", "app"] })).is_ok());
            assert!(tool_visibility_code(&expr(quote! { "app" })).is_ok());

            let err = tool_visibility_code(&expr(quote! { ["agent"] }))
                .expect_err("`agent` is not a scope");
            assert!(err.to_string().contains("agent"));
        }

        #[test]
        fn ui_meta_rejects_malformed_json() {
            let err =
                resource_ui_meta_code(&expr(quote! { "{ not json" }), Some("ui://clock/app.html"))
                    .expect_err("malformed");

            assert!(err.to_string().contains("invalid JSON"));
        }

        #[test]
        fn ui_meta_rejects_a_non_object() {
            let err =
                resource_ui_meta_code(&expr(quote! { "[1, 2]" }), Some("ui://clock/app.html"))
                    .expect_err("must be an object");

            assert!(err.to_string().contains("JSON object"));
        }

        #[test]
        fn ui_meta_rejects_a_snake_case_key() {
            // The failure this exists to prevent: `_meta` is an open map, so
            // this would serialize fine and every host would ignore it.
            let err = resource_ui_meta_code(
                &expr(quote! { "{\"prefers_border\": true}" }),
                Some("ui://clock/app.html"),
            )
            .expect_err("camelCase on the wire");

            assert!(err.to_string().contains("prefers_border"));
            assert!(err.to_string().contains("prefersBorder"));
        }

        #[test]
        fn ui_meta_checks_nested_csp_and_permission_keys() {
            let err = resource_ui_meta_code(
                &expr(quote! { "{\"csp\": {\"connect_domains\": []}}" }),
                Some("ui://clock/app.html"),
            )
            .expect_err("a silently-ignored CSP is the worst kind");
            assert!(err.to_string().contains("ui_meta.csp"));

            let err = resource_ui_meta_code(
                &expr(quote! { "{\"permissions\": {\"clipboard_write\": {}}}" }),
                Some("ui://clock/app.html"),
            )
            .expect_err("same for permissions");
            assert!(err.to_string().contains("ui_meta.permissions"));
        }

        #[test]
        fn ui_meta_accepts_the_full_spec_shape() {
            let json = r#"{
                "csp": {
                    "connectDomains": ["https://api.example.com"],
                    "resourceDomains": ["https://cdn.example.com"],
                    "frameDomains": [],
                    "baseUriDomains": []
                },
                "permissions": { "camera": {}, "clipboardWrite": {} },
                "domain": "app.example.com",
                "prefersBorder": true
            }"#;

            assert!(
                resource_ui_meta_code(&expr(quote! { #json }), Some("ui://clock/app.html")).is_ok()
            );
        }

        #[test]
        fn ui_meta_is_refused_on_a_resource_that_is_not_an_app() {
            let err = resource_ui_meta_code(
                &expr(quote! { "{\"prefersBorder\": true}" }),
                Some("res://notes/{id}"),
            )
            .expect_err("hosts ignore the block off a ui:// resource");

            assert!(err.to_string().contains("res://notes/{id}"));
        }
    }

    #[cfg(not(feature = "apps"))]
    #[test]
    fn the_attributes_ask_for_the_feature_by_name() {
        let err = tool_ui_code(&expr(quote! { "ui://weather/dashboard" }))
            .expect_err("the generated call would not compile");

        assert!(err.to_string().contains("apps"));
    }
}
