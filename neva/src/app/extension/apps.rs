//! The built-in MCP Apps extension (`io.modelcontextprotocol/ui`).

use super::Extension;
use crate::app::App;
use crate::types::{
    APP_MIME_TYPE, Resource, TextResourceContents, UiCsp, UiPermissions, UiResourceMeta, Uri,
};

/// The built-in MCP Apps extension (`io.modelcontextprotocol/ui`).
///
/// Advertising it is the whole registration: MCP Apps contributes no methods of
/// its own, only metadata on ordinary tools and resources. It is normally
/// reached through the
/// [`with_apps`](crate::app::options::McpOptions::with_apps) thin wrapper; go
/// through [`App::with_extension`] when you want to change how `ui://` resources
/// are surfaced.
///
/// # Advertised value
///
/// An empty object. The specification defines settings for the *client*
/// direction only (which content types a host can render), so a server has
/// nothing to say beyond "supported" -- which is exactly what `{}` means under
/// SEP-1724.
///
/// # Examples
/// ```
/// # #[cfg(all(not(feature = "legacy-spec"), feature = "apps"))] {
/// use neva::App;
/// use neva::app::extension::AppsExtension;
///
/// // Defaults: `ui://` resources are reachable by `resources/read`, and stay
/// // out of `resources/list`.
/// let app = App::new().with_extension(AppsExtension::new());
/// # let _ = app;
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct AppsExtension {
    list_resources: bool,
}

impl AppsExtension {
    /// The reverse-DNS id of the MCP Apps extension.
    pub const ID: &'static str = crate::types::APPS_EXTENSION_ID;

    /// Creates the extension with its defaults.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::app::extension::{AppsExtension, Extension};
    ///
    /// assert_eq!(AppsExtension::new().id(), "io.modelcontextprotocol/ui");
    /// # }
    /// ```
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }

    /// Lists `ui://` resources in `resources/list`.
    ///
    /// Off by default. The specification lets a server omit UI resources from
    /// the listing -- a host discovers them through the tool's
    /// `_meta.ui.resourceUri` and fetches them with `resources/read` -- and a UI
    /// template is not something a user browses. Turn it on when you want hosts
    /// to be able to review each app's security block at connection time.
    ///
    /// The switch is read when the server starts, so it applies to every
    /// [`App::add_ui_resource`] regardless of the order the builder calls
    /// happen in.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(all(not(feature = "legacy-spec"), feature = "apps"))] {
    /// use neva::App;
    /// use neva::app::extension::AppsExtension;
    ///
    /// let app = App::new()
    ///     .with_extension(AppsExtension::new().with_listed_resources());
    /// # let _ = app;
    /// # }
    /// ```
    #[inline]
    pub fn with_listed_resources(mut self) -> Self {
        self.list_resources = true;
        self
    }
}

impl Extension for AppsExtension {
    #[inline]
    fn id(&self) -> &'static str {
        Self::ID
    }

    #[inline]
    fn capability(&self) -> serde_json::Value {
        serde_json::Value::Object(Default::default())
    }

    fn register(self, app: &mut App) {
        app.options.set_ui_resource_listing(self.list_resources);
    }
}

/// An HTML document served as a `ui://` MCP Apps resource.
///
/// Registered with [`App::add_ui_resource`], which hands back a `&mut` so the
/// security block can be filled in. The server reads it when it starts, so
/// everything set on it counts however late it is set:
///
/// * `resources/read` on the URI returns the HTML as
///   [`APP_MIME_TYPE`](crate::types::APP_MIME_TYPE), carrying `_meta.ui`.
/// * `resources/list` carries the same block, when
///   [`AppsExtension::with_listed_resources`] is on.
///
/// # Examples
/// ```no_run
/// use neva::{App, types::UiCsp};
///
/// # #[tokio::main]
/// # async fn main() {
/// let mut app = App::new();
///
/// app.add_ui_resource("ui://weather/dashboard", "dashboard", "<!doctype html>...")
///     .with_title("Weather dashboard")
///     .with_csp(UiCsp::new().with_connect_domains(["https://api.openweathermap.org"]))
///     .with_prefers_border(true);
///
/// # app.run().await;
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct UiResource {
    uri: Uri,
    name: String,
    title: Option<String>,
    descr: Option<String>,
    html: String,
    ui: UiResourceMeta,
}

impl UiResource {
    /// Creates a `ui://` resource serving `html`.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::app::extension::UiResource;
    ///
    /// let res = UiResource::new("ui://clock/app.html", "clock", "<!doctype html>");
    /// assert_eq!(res.uri().to_string(), "ui://clock/app.html");
    /// # }
    /// ```
    pub fn new<U, S, H>(uri: U, name: S, html: H) -> Self
    where
        U: Into<Uri>,
        S: Into<String>,
        H: Into<String>,
    {
        Self {
            uri: uri.into(),
            name: name.into(),
            title: None,
            descr: None,
            html: html.into(),
            ui: UiResourceMeta::new(),
        }
    }

    /// The `ui://` URI a tool's `_meta.ui.resourceUri` points at.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::app::extension::UiResource;
    ///
    /// let res = UiResource::new("ui://clock/app.html", "clock", "");
    /// assert!(res.uri().starts_with("ui://"));
    /// # }
    /// ```
    #[inline]
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    /// Sets a human-readable title.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::app::extension::UiResource;
    ///
    /// let mut res = UiResource::new("ui://clock/app.html", "clock", "");
    /// res.with_title("Clock");
    /// # }
    /// ```
    #[inline]
    pub fn with_title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    /// Sets a description of what the app does.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::app::extension::UiResource;
    ///
    /// let mut res = UiResource::new("ui://clock/app.html", "clock", "");
    /// res.with_descr("A ticking clock");
    /// # }
    /// ```
    #[inline]
    pub fn with_descr(&mut self, descr: impl Into<String>) -> &mut Self {
        self.descr = Some(descr.into());
        self
    }

    /// Declares the origins the app needs the host to allow.
    ///
    /// The app runs sandboxed with no same-origin server, so *every* origin it
    /// touches belongs here -- including wherever its own bundled scripts and
    /// styles are served from.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::{app::extension::UiResource, types::UiCsp};
    ///
    /// let mut res = UiResource::new("ui://weather/app.html", "weather", "");
    /// res.with_csp(UiCsp::new().with_connect_domains(["https://api.example.com"]));
    /// # }
    /// ```
    #[inline]
    pub fn with_csp(&mut self, csp: UiCsp) -> &mut Self {
        self.ui.csp = Some(csp);
        self
    }

    /// Requests browser permissions for the app's iframe.
    ///
    /// Requests, not grants: the host may ignore them, so feature-detect rather
    /// than assume.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::{app::extension::UiResource, types::UiPermissions};
    ///
    /// let mut res = UiResource::new("ui://scan/app.html", "scan", "");
    /// res.with_permissions(UiPermissions::new().with_camera());
    /// # }
    /// ```
    #[inline]
    pub fn with_permissions(&mut self, permissions: UiPermissions) -> &mut Self {
        self.ui.permissions = Some(permissions);
        self
    }

    /// Asks the host to serve the app from a dedicated sandbox origin.
    ///
    /// Useful when the app needs a stable origin for OAuth callbacks, CORS or
    /// API-key allowlists. **The format is host-defined** -- consult the host's
    /// documentation rather than guessing.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::app::extension::UiResource;
    ///
    /// let mut res = UiResource::new("ui://dash/app.html", "dash", "");
    /// res.with_domain("a904794854a047f6.claudemcpcontent.com");
    /// # }
    /// ```
    #[inline]
    pub fn with_domain(&mut self, domain: impl Into<String>) -> &mut Self {
        self.ui.domain = Some(domain.into());
        self
    }

    /// States whether the app wants a visible border and background.
    ///
    /// Worth stating either way: hosts' defaults differ.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::app::extension::UiResource;
    ///
    /// let mut res = UiResource::new("ui://clock/app.html", "clock", "");
    /// res.with_prefers_border(false);
    /// # }
    /// ```
    #[inline]
    pub fn with_prefers_border(&mut self, prefers_border: bool) -> &mut Self {
        self.ui.prefers_border = Some(prefers_border);
        self
    }

    /// Replaces the whole `_meta.ui` block.
    ///
    /// The escape hatch for a block built elsewhere; the other builders set one
    /// field each.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(feature = "apps")] {
    /// use neva::{app::extension::UiResource, types::UiResourceMeta};
    ///
    /// let mut res = UiResource::new("ui://clock/app.html", "clock", "");
    /// res.with_ui(UiResourceMeta::new().with_prefers_border(true));
    /// # }
    /// ```
    #[inline]
    pub fn with_ui(&mut self, ui: UiResourceMeta) -> &mut Self {
        self.ui = ui;
        self
    }

    /// The `resources/list` entry for this app.
    pub(crate) fn listing(&self) -> Resource {
        let mut resource =
            Resource::new(self.uri.clone(), self.name.clone()).with_mime(APP_MIME_TYPE);
        resource.title = self.title.clone();
        resource.descr = self.descr.clone();
        resource.with_ui(self.ui.clone())
    }

    /// The `resources/read` content item for this app.
    pub(crate) fn contents(&self) -> TextResourceContents {
        let contents = TextResourceContents::new(self.uri.clone(), self.html.clone())
            .with_mime(APP_MIME_TYPE)
            .with_ui(self.ui.clone());
        match self.title.as_ref() {
            Some(title) => contents.with_title(title),
            None => contents,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{APPS_EXTENSION_ID, UiCsp};

    #[test]
    fn apps_extension_id_is_reverse_dns() {
        assert_eq!(AppsExtension::new().id(), APPS_EXTENSION_ID);
        assert_eq!(AppsExtension::ID, "io.modelcontextprotocol/ui");
    }

    #[test]
    fn apps_extension_advertises_no_settings() {
        // The spec defines settings for the client direction only, so a server
        // has nothing to say beyond "supported".
        assert_eq!(
            AppsExtension::new().capability(),
            serde_json::json!({}),
            "a server must not invent settings in a reserved identifier"
        );
    }

    #[test]
    fn resources_are_not_listed_by_default() {
        assert!(!AppsExtension::new().list_resources);
        assert!(AppsExtension::new().with_listed_resources().list_resources);
    }

    #[test]
    fn contents_carry_the_app_mime_type_and_the_ui_block() {
        let mut res = UiResource::new("ui://clock/app.html", "clock", "<!doctype html>");
        res.with_csp(UiCsp::new().with_connect_domains(["https://api.example.com"]))
            .with_prefers_border(true);

        let contents = res.contents();

        assert_eq!(contents.mime.as_deref(), Some(APP_MIME_TYPE));
        assert_eq!(contents.text, "<!doctype html>");
        assert_eq!(
            contents.meta,
            Some(serde_json::json!({
                "ui": {
                    "csp": { "connectDomains": ["https://api.example.com"] },
                    "prefersBorder": true
                }
            }))
        );
    }

    #[test]
    fn the_listing_entry_carries_the_same_block() {
        let mut res = UiResource::new("ui://clock/app.html", "clock", "<!doctype html>");
        res.with_title("Clock").with_prefers_border(false);

        let listing = res.listing();

        assert_eq!(listing.uri.to_string(), "ui://clock/app.html");
        assert_eq!(listing.name, "clock");
        assert_eq!(listing.title.as_deref(), Some("Clock"));
        assert_eq!(listing.mime.as_deref(), Some(APP_MIME_TYPE));
        assert_eq!(
            listing.ui().and_then(|ui| ui.prefers_border),
            Some(false),
            "the listing block is the static default a host reviews at connection time"
        );
    }

    #[test]
    fn builders_set_after_registration_still_count() {
        // `listing`/`contents` are materialized when the server starts, so a
        // `&mut UiResource` stays live for the whole builder chain.
        let mut res = UiResource::new("ui://late/app.html", "late", "");
        assert!(res.contents().ui().is_none_or(|ui| ui.domain.is_none()));

        res.with_domain("late.example.com");

        assert_eq!(
            res.contents().ui().and_then(|ui| ui.domain),
            Some("late.example.com".to_string())
        );
    }

    #[test]
    fn with_ui_replaces_the_whole_block() {
        let mut res = UiResource::new("ui://clock/app.html", "clock", "");
        res.with_prefers_border(true)
            .with_ui(UiResourceMeta::new().with_domain("clock.example.com"));

        let ui = res.contents().ui().expect("a ui block");

        assert_eq!(ui.domain.as_deref(), Some("clock.example.com"));
        assert_eq!(ui.prefers_border, None);
    }
}
