//! MCP Apps wire types (`io.modelcontextprotocol/ui`).
//!
//! [SEP-1865](https://github.com/modelcontextprotocol/ext-apps) lets a server
//! hand a host an interactive UI: an HTML document, declared as a `ui://`
//! resource, that the host renders in a sandboxed iframe and feeds a tool's
//! result into.
//!
//! The specification has two halves, and **only one of them is wire traffic an
//! MCP peer sees**:
//!
//! * The *data plane* -- this module. A `_meta.ui` block on a [`Tool`] naming
//!   the resource that renders it ([`UiToolMeta`]), and a `_meta.ui` block on
//!   the resource carrying its security configuration ([`UiResourceMeta`]).
//!   Plain metadata on ordinary `tools/list` and `resources/read` results.
//! * The *presentation plane* -- everything named `ui/*` (`ui/initialize`,
//!   `ui/notifications/tool-result`, the sandbox proxy, host context, theming).
//!   That is JSON-RPC over `postMessage` between a host and an iframe inside a
//!   browser. A server never sends or receives any of it, and neva models none
//!   of it.
//!
//! So a neva server serves a tool and an HTML document; the host does the
//! theater.
//!
//! # Graceful degradation
//!
//! The one behavioural rule the specification puts on a handler: a UI-bound tool
//! **MUST** still return a meaningful `content` array. The model reads
//! `content`; the iframe is for humans, and not every client has one.
//!
//! [`Tool`]: crate::types::Tool
//!
//! # Examples
//! ```
//! use neva::types::apps::{UiToolMeta, UiVisibility};
//!
//! // A tool the iframe may call but the model must not see.
//! let meta = UiToolMeta::new("ui://weather/dashboard")
//!     .with_visibility([UiVisibility::App]);
//!
//! assert!(!meta.is_model_visible());
//! assert!(meta.is_app_visible());
//! ```

use crate::types::Uri;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Reverse-DNS id of the MCP Apps extension.
///
/// The key both peers advertise the extension under in
/// `capabilities.extensions`. Reserved by the specification.
///
/// # Examples
/// ```
/// use neva::types::apps::APPS_EXTENSION_ID;
///
/// assert_eq!(APPS_EXTENSION_ID, "io.modelcontextprotocol/ui");
/// ```
pub const APPS_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";

/// The MIME type every `ui://` resource is served under.
///
/// A host renders a `ui://` resource only under this type; anything else is a
/// resource no host will show.
///
/// # Examples
/// ```
/// use neva::types::apps::APP_MIME_TYPE;
///
/// assert_eq!(APP_MIME_TYPE, "text/html;profile=mcp-app");
/// ```
pub const APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// The URI scheme the specification reserves for MCP Apps resources.
///
/// # Examples
/// ```
/// use neva::types::apps::UI_SCHEME;
///
/// assert!("ui://weather/dashboard".starts_with(UI_SCHEME));
/// ```
pub const UI_SCHEME: &str = "ui://";

/// The `_meta` key both halves of the extension nest their metadata under.
pub(crate) const UI_META_KEY: &str = "ui";

/// The deprecated flat spelling of `_meta.ui.resourceUri`.
///
/// Read-only. The specification deprecates it in favour of the nested block but
/// still tells the reading side to accept both, so a tool from a server on an
/// older SDK is understood; nothing neva writes carries it.
pub(crate) const LEGACY_RESOURCE_URI_KEY: &str = "ui/resourceUri";

/// Whether `uri` addresses an MCP Apps resource.
///
/// The scheme *is* the declaration: a `ui://` URI is an app resource, and
/// nothing else is.
///
/// # Examples
/// ```
/// use neva::types::apps::is_ui_uri;
///
/// assert!(is_ui_uri("ui://weather/dashboard"));
/// assert!(!is_ui_uri("res://weather/dashboard"));
/// ```
#[inline]
pub fn is_ui_uri(uri: &str) -> bool {
    uri.starts_with(UI_SCHEME)
}

/// Who may call a UI-bound tool.
///
/// Filtering is the **host's** job: a server lists an app-only tool in
/// `tools/list` like any other and the host keeps it out of the agent's tool
/// list. See [`UiToolMeta::visibility`].
///
/// # Examples
/// ```
/// use neva::types::apps::UiVisibility;
///
/// let json = serde_json::to_string(&UiVisibility::Model)?;
/// assert_eq!(json, r#""model""#);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiVisibility {
    /// The tool is visible to, and callable by, the agent.
    Model,

    /// The tool is callable by the app -- from this server's connection only.
    App,
}

/// `_meta.ui` on a [`Tool`](crate::types::Tool): what renders it, and for whom.
///
/// # Examples
/// ```
/// use neva::types::apps::{UiToolMeta, UiVisibility};
///
/// let meta = UiToolMeta::new("ui://weather/dashboard");
///
/// // Omitted visibility means both, which is the specification's default.
/// assert!(meta.is_model_visible() && meta.is_app_visible());
/// assert_eq!(
///     serde_json::to_value(&meta)?,
///     serde_json::json!({ "resourceUri": "ui://weather/dashboard" })
/// );
///
/// let hidden = meta.with_visibility([UiVisibility::App]);
/// assert!(!hidden.is_model_visible());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiToolMeta {
    /// URI of the UI resource that renders this tool's results.
    #[serde(
        rename = "resourceUri",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_uri: Option<Uri>,

    /// Who may call the tool. Omitted means both `model` and `app`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Vec<UiVisibility>>,
}

/// Content Security Policy origins a `ui://` resource asks the host to allow.
///
/// These are **requests to the host**, not server behaviour: the host builds the
/// iframe's CSP from them and may refuse. An omitted or empty list is the secure
/// default -- no external access of that kind.
///
/// An app's HTML runs in a sandbox with no same-origin server, so *every* origin
/// it touches has to appear here, including wherever its own bundled JS and CSS
/// are served from.
///
/// # Examples
/// ```
/// use neva::types::apps::UiCsp;
///
/// let csp = UiCsp::new()
///     .with_connect_domains(["https://api.openweathermap.org"])
///     .with_resource_domains(["https://cdn.jsdelivr.net"]);
///
/// assert_eq!(
///     serde_json::to_value(&csp)?,
///     serde_json::json!({
///         "connectDomains": ["https://api.openweathermap.org"],
///         "resourceDomains": ["https://cdn.jsdelivr.net"]
///     })
/// );
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiCsp {
    /// Origins for network requests -- `fetch`, XHR, WebSocket. Maps to
    /// `connect-src`.
    #[serde(
        rename = "connectDomains",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub connect_domains: Option<Vec<String>>,

    /// Origins for static resources -- scripts, styles, images, fonts, media.
    /// Maps to `script-src`, `style-src`, `img-src`, `font-src` and
    /// `media-src`. Wildcard subdomains (`https://*.example.com`) are allowed.
    #[serde(
        rename = "resourceDomains",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_domains: Option<Vec<String>>,

    /// Origins for nested iframes. Maps to `frame-src`; omitted means
    /// `frame-src 'none'`.
    #[serde(
        rename = "frameDomains",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub frame_domains: Option<Vec<String>>,

    /// Allowed base URIs for the document. Maps to `base-uri`; omitted means
    /// `base-uri 'self'`.
    #[serde(
        rename = "baseUriDomains",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub base_uri_domains: Option<Vec<String>>,
}

/// Browser permissions a `ui://` resource requests for its iframe.
///
/// Requests, not grants: the host may honor them through the iframe's `allow`
/// attribute or ignore them entirely, so an app should feature-detect rather
/// than assume.
///
/// On the wire each permission is declared by the **presence of an empty
/// object** (`{"camera": {}}`) -- the same shape the MRTR client capabilities
/// use. A `false` flag here is simply absent from the JSON.
///
/// # Examples
/// ```
/// use neva::types::apps::UiPermissions;
///
/// let perms = UiPermissions::new().with_clipboard_write();
///
/// assert_eq!(
///     serde_json::to_value(&perms)?,
///     serde_json::json!({ "clipboardWrite": {} })
/// );
///
/// let read: UiPermissions = serde_json::from_value(
///     serde_json::json!({ "camera": {}, "geolocation": {} })
/// )?;
/// assert!(read.camera && read.geolocation && !read.microphone);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPermissions {
    /// Requests camera access (Permissions Policy `camera`).
    #[serde(
        default,
        deserialize_with = "de_declared",
        serialize_with = "ser_declared",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub camera: bool,

    /// Requests microphone access (Permissions Policy `microphone`).
    #[serde(
        default,
        deserialize_with = "de_declared",
        serialize_with = "ser_declared",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub microphone: bool,

    /// Requests geolocation access (Permissions Policy `geolocation`).
    #[serde(
        default,
        deserialize_with = "de_declared",
        serialize_with = "ser_declared",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub geolocation: bool,

    /// Requests clipboard write access (Permissions Policy `clipboard-write`).
    #[serde(
        rename = "clipboardWrite",
        default,
        deserialize_with = "de_declared",
        serialize_with = "ser_declared",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub clipboard_write: bool,
}

/// `_meta.ui` on a `ui://` resource: how the host should sandbox and frame it.
///
/// It may ride the `resources/list` entry (a static default the host can review
/// at connection time) and the `resources/read` content item (per-response, and
/// possibly dynamic). When both carry one, **the content item wins**.
///
/// # Examples
/// ```
/// use neva::types::apps::{UiCsp, UiResourceMeta};
///
/// let meta = UiResourceMeta::new()
///     .with_csp(UiCsp::new().with_connect_domains(["https://api.example.com"]))
///     .with_prefers_border(true);
///
/// assert_eq!(
///     serde_json::to_value(&meta)?,
///     serde_json::json!({
///         "csp": { "connectDomains": ["https://api.example.com"] },
///         "prefersBorder": true
///     })
/// );
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiResourceMeta {
    /// Origins the app needs the host to allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csp: Option<UiCsp>,

    /// Browser permissions the app asks its iframe to be granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<UiPermissions>,

    /// A dedicated sandbox origin for the view.
    ///
    /// Useful when the view needs a stable origin for OAuth callbacks, CORS
    /// policies or API-key allowlists. **The format is host-defined** -- consult
    /// the host's documentation rather than guessing. Omitted means the host's
    /// default origin, typically per-conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,

    /// Whether the app would like a visible border and background from the host.
    ///
    /// Worth stating explicitly: hosts' defaults differ, and omitting it leaves
    /// the choice to the host.
    #[serde(
        rename = "prefersBorder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub prefers_border: Option<bool>,
}

/// The settings object advertised under
/// `capabilities.extensions["io.modelcontextprotocol/ui"]`.
///
/// The specification defines these for the **client** direction only: a client
/// says which content types it can render, and `mimeTypes` is required -- a
/// client that omits it has not declared MCP Apps support. A server advertises
/// the extension with an empty object, since no server-side settings are
/// defined.
///
/// # Examples
/// ```
/// use neva::types::apps::AppsCapability;
///
/// let cap = AppsCapability::new();
/// assert!(cap.supports_html());
/// assert_eq!(
///     serde_json::to_value(&cap)?,
///     serde_json::json!({ "mimeTypes": ["text/html;profile=mcp-app"] })
/// );
///
/// // A peer that named no types has not declared support.
/// let empty: AppsCapability = serde_json::from_value(serde_json::json!({}))?;
/// assert!(!empty.supports_html());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsCapability {
    /// The content types the peer can render.
    ///
    /// Required by the specification, so it is always written -- even empty,
    /// which is how a peer says it renders nothing.
    #[serde(rename = "mimeTypes", default)]
    pub mime_types: Vec<String>,
}

impl UiToolMeta {
    /// Creates metadata binding a tool to the UI resource at `resource_uri`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiToolMeta;
    ///
    /// let meta = UiToolMeta::new("ui://weather/dashboard");
    /// assert_eq!(meta.resource_uri.as_deref(), Some("ui://weather/dashboard"));
    /// ```
    #[inline]
    pub fn new(resource_uri: impl Into<Uri>) -> Self {
        Self {
            resource_uri: Some(resource_uri.into()),
            visibility: None,
        }
    }

    /// Sets who may call the tool.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::{UiToolMeta, UiVisibility};
    ///
    /// let meta = UiToolMeta::new("ui://shop/cart")
    ///     .with_visibility([UiVisibility::Model, UiVisibility::App]);
    ///
    /// assert!(meta.is_model_visible() && meta.is_app_visible());
    /// ```
    #[inline]
    pub fn with_visibility<T>(mut self, visibility: T) -> Self
    where
        T: IntoIterator<Item = UiVisibility>,
    {
        self.visibility = Some(visibility.into_iter().collect());
        self
    }

    /// Whether the agent may see and call this tool.
    ///
    /// An omitted `visibility` means both scopes, so an unset value is `true`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::{UiToolMeta, UiVisibility};
    ///
    /// assert!(UiToolMeta::default().is_model_visible());
    /// assert!(!UiToolMeta::default()
    ///     .with_visibility([UiVisibility::App])
    ///     .is_model_visible());
    /// ```
    #[inline]
    pub fn is_model_visible(&self) -> bool {
        self.allows(UiVisibility::Model)
    }

    /// Whether the app may call this tool.
    ///
    /// An omitted `visibility` means both scopes, so an unset value is `true`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::{UiToolMeta, UiVisibility};
    ///
    /// assert!(UiToolMeta::default().is_app_visible());
    /// assert!(!UiToolMeta::default()
    ///     .with_visibility([UiVisibility::Model])
    ///     .is_app_visible());
    /// ```
    #[inline]
    pub fn is_app_visible(&self) -> bool {
        self.allows(UiVisibility::App)
    }

    /// Whether `scope` is in the declared visibility, treating an unset list as
    /// the specification's `["model", "app"]` default.
    #[inline]
    fn allows(&self, scope: UiVisibility) -> bool {
        self.visibility
            .as_ref()
            .is_none_or(|scopes| scopes.contains(&scope))
    }
}

impl UiCsp {
    /// Creates an empty policy: no external access of any kind.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiCsp;
    ///
    /// assert_eq!(serde_json::to_value(UiCsp::new())?, serde_json::json!({}));
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }

    /// Sets the origins the app may open network connections to (`connect-src`).
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiCsp;
    ///
    /// let csp = UiCsp::new().with_connect_domains(["wss://realtime.example.com"]);
    /// assert!(csp.connect_domains.is_some());
    /// ```
    pub fn with_connect_domains<T, I>(mut self, domains: T) -> Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.connect_domains = Some(domains.into_iter().map(Into::into).collect());
        self
    }

    /// Sets the origins the app may load scripts, styles, images, fonts and
    /// media from.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiCsp;
    ///
    /// let csp = UiCsp::new().with_resource_domains(["https://*.cloudflare.com"]);
    /// assert!(csp.resource_domains.is_some());
    /// ```
    pub fn with_resource_domains<T, I>(mut self, domains: T) -> Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.resource_domains = Some(domains.into_iter().map(Into::into).collect());
        self
    }

    /// Sets the origins the app may nest iframes from (`frame-src`).
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiCsp;
    ///
    /// let csp = UiCsp::new().with_frame_domains(["https://www.youtube.com"]);
    /// assert!(csp.frame_domains.is_some());
    /// ```
    pub fn with_frame_domains<T, I>(mut self, domains: T) -> Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.frame_domains = Some(domains.into_iter().map(Into::into).collect());
        self
    }

    /// Sets the base URIs the document may declare (`base-uri`).
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiCsp;
    ///
    /// let csp = UiCsp::new().with_base_uri_domains(["https://cdn.example.com"]);
    /// assert!(csp.base_uri_domains.is_some());
    /// ```
    pub fn with_base_uri_domains<T, I>(mut self, domains: T) -> Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.base_uri_domains = Some(domains.into_iter().map(Into::into).collect());
        self
    }
}

impl UiPermissions {
    /// Creates a set requesting nothing.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiPermissions;
    ///
    /// assert_eq!(
    ///     serde_json::to_value(UiPermissions::new())?,
    ///     serde_json::json!({})
    /// );
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }

    /// Requests camera access.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiPermissions;
    ///
    /// assert!(UiPermissions::new().with_camera().camera);
    /// ```
    #[inline]
    pub fn with_camera(mut self) -> Self {
        self.camera = true;
        self
    }

    /// Requests microphone access.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiPermissions;
    ///
    /// assert!(UiPermissions::new().with_microphone().microphone);
    /// ```
    #[inline]
    pub fn with_microphone(mut self) -> Self {
        self.microphone = true;
        self
    }

    /// Requests geolocation access.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiPermissions;
    ///
    /// assert!(UiPermissions::new().with_geolocation().geolocation);
    /// ```
    #[inline]
    pub fn with_geolocation(mut self) -> Self {
        self.geolocation = true;
        self
    }

    /// Requests clipboard write access.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiPermissions;
    ///
    /// assert!(UiPermissions::new().with_clipboard_write().clipboard_write);
    /// ```
    #[inline]
    pub fn with_clipboard_write(mut self) -> Self {
        self.clipboard_write = true;
        self
    }
}

impl UiResourceMeta {
    /// Creates metadata declaring nothing, which is the secure default: no
    /// external access, no permissions, the host's own sandbox origin.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiResourceMeta;
    ///
    /// assert_eq!(
    ///     serde_json::to_value(UiResourceMeta::new())?,
    ///     serde_json::json!({})
    /// );
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }

    /// Deserializes a block from a JSON string.
    ///
    /// The `#[resource(ui_meta = "...")]` attribute expands to this; the macro
    /// has already checked at compile time that the literal is well-formed and
    /// names only keys the specification defines.
    ///
    /// # Examples
    /// ```
    /// use neva::types::UiResourceMeta;
    ///
    /// let meta = UiResourceMeta::from_json_str(
    ///     r#"{ "csp": { "connectDomains": ["https://api.example.com"] } }"#,
    /// )?;
    ///
    /// assert!(meta.csp.is_some());
    /// # Ok::<(), neva::error::Error>(())
    /// ```
    #[inline]
    pub fn from_json_str(json: &str) -> Result<Self, crate::error::Error> {
        serde_json::from_str(json).map_err(Into::into)
    }

    /// Sets the origins the app needs.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::{UiCsp, UiResourceMeta};
    ///
    /// let meta = UiResourceMeta::new()
    ///     .with_csp(UiCsp::new().with_connect_domains(["https://api.example.com"]));
    /// assert!(meta.csp.is_some());
    /// ```
    #[inline]
    pub fn with_csp(mut self, csp: UiCsp) -> Self {
        self.csp = Some(csp);
        self
    }

    /// Sets the browser permissions the app requests.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::{UiPermissions, UiResourceMeta};
    ///
    /// let meta = UiResourceMeta::new()
    ///     .with_permissions(UiPermissions::new().with_geolocation());
    /// assert!(meta.permissions.is_some());
    /// ```
    #[inline]
    pub fn with_permissions(mut self, permissions: UiPermissions) -> Self {
        self.permissions = Some(permissions);
        self
    }

    /// Sets the dedicated sandbox origin the view should be served from.
    ///
    /// The format is host-defined; see [`Self::domain`].
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiResourceMeta;
    ///
    /// let meta = UiResourceMeta::new().with_domain("dashboard.example.com");
    /// assert_eq!(meta.domain.as_deref(), Some("dashboard.example.com"));
    /// ```
    #[inline]
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// States whether the app wants a visible border and background.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::UiResourceMeta;
    ///
    /// let meta = UiResourceMeta::new().with_prefers_border(false);
    /// assert_eq!(meta.prefers_border, Some(false));
    /// ```
    #[inline]
    pub fn with_prefers_border(mut self, prefers_border: bool) -> Self {
        self.prefers_border = Some(prefers_border);
        self
    }
}

impl Default for AppsCapability {
    /// Declares the one content type the specification defines,
    /// [`APP_MIME_TYPE`].
    #[inline]
    fn default() -> Self {
        Self {
            mime_types: vec![APP_MIME_TYPE.into()],
        }
    }
}

impl AppsCapability {
    /// Creates a capability declaring [`APP_MIME_TYPE`], the only content type
    /// the initial specification defines.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::AppsCapability;
    ///
    /// assert!(AppsCapability::new().supports_html());
    /// ```
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }

    /// Creates a capability declaring exactly `mime_types`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::AppsCapability;
    ///
    /// let cap = AppsCapability::with_mime_types(["text/html;profile=mcp-app"]);
    /// assert!(cap.supports_html());
    /// ```
    pub fn with_mime_types<T, I>(mime_types: T) -> Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        Self {
            mime_types: mime_types.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether the peer declared it can render [`APP_MIME_TYPE`].
    ///
    /// This is the whole test for "does the other side do MCP Apps": the
    /// specification makes `mimeTypes` required, so a peer that named nothing
    /// has not declared support, however the extension key looks.
    ///
    /// # Examples
    /// ```
    /// use neva::types::apps::AppsCapability;
    ///
    /// assert!(AppsCapability::new().supports_html());
    /// assert!(!AppsCapability::with_mime_types(["text/plain"]).supports_html());
    /// ```
    #[inline]
    pub fn supports_html(&self) -> bool {
        self.mime_types.iter().any(|mime| mime == APP_MIME_TYPE)
    }
}

/// Reads a permission the specification spells as an optional empty object:
/// present means requested, whatever the object holds.
///
/// The twin of `types::mrtr::de_declared`, minus the boolean tolerance that one
/// keeps for neva clients older than the spec shape -- no such peer ever wrote
/// these keys.
fn de_declared<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Any JSON object, whatever it holds: presence is the declaration, so
    /// unknown sub-fields are accepted and ignored.
    #[derive(Deserialize)]
    struct AnyObject {}

    Ok(Option::<AnyObject>::deserialize(deserializer)?.is_some())
}

/// Writes a requested permission in the spec shape: an empty object. Only ever
/// called for a `true` flag -- a `false` one is skipped, which is how the spec
/// spells "not requested".
fn ser_declared<S>(_declared: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    serializer.serialize_map(Some(0))?.end()
}

/// Reads the `ui` entry out of a `_meta` object.
///
/// Returns `None` when `_meta` is absent, is not an object, carries no `ui` key,
/// or carries one that does not parse as `T` -- a malformed extension block is
/// not a reason to fail an otherwise valid `tools/list`.
///
/// # Examples
/// ```
/// use neva::types::apps::{self, UiToolMeta};
///
/// let meta = serde_json::json!({ "ui": { "resourceUri": "ui://clock/app.html" } });
/// let ui: UiToolMeta = apps::get_ui_meta(Some(&meta)).expect("a ui entry");
///
/// assert_eq!(ui.resource_uri.as_deref(), Some("ui://clock/app.html"));
/// assert_eq!(apps::get_ui_meta::<UiToolMeta>(None), None);
/// ```
pub fn get_ui_meta<T: DeserializeOwned>(meta: Option<&Value>) -> Option<T> {
    meta?
        .get(UI_META_KEY)
        .and_then(|ui| serde_json::from_value(ui.clone()).ok())
}

/// Reads a tool's `_meta.ui`, accepting the deprecated flat spelling.
///
/// The specification's host-side guidance is to check both formats, and this is
/// the reading side: the nested block wins, and `_meta["ui/resourceUri"]` fills
/// in the URI when the nested one does not carry it. A tool whose only MCP Apps
/// metadata is the flat key still comes back as a [`UiToolMeta`] -- otherwise a
/// client would conclude it has no UI at all.
pub(crate) fn get_tool_ui_meta(meta: Option<&Value>) -> Option<UiToolMeta> {
    let nested: Option<UiToolMeta> = get_ui_meta(meta);
    let legacy = meta
        .and_then(|meta| meta.get(LEGACY_RESOURCE_URI_KEY))
        .and_then(Value::as_str);

    match (nested, legacy) {
        (Some(mut ui), Some(legacy)) => {
            ui.resource_uri.get_or_insert_with(|| legacy.into());
            Some(ui)
        }
        (Some(ui), None) => Some(ui),
        (None, Some(legacy)) => Some(UiToolMeta::new(legacy)),
        (None, None) => None,
    }
}

/// What a tool's `_meta.ui` says about who may call it.
///
/// Read on its own rather than off a decoded [`UiToolMeta`], because the two
/// want opposite things from a malformed block. [`get_tool_ui_meta`] is lenient
/// -- a block it cannot parse reads as absent, so one bad tool does not fail an
/// otherwise valid `tools/list`. An audience restriction cannot afford that: a
/// `resourceUri` of the wrong type sits in the same object as
/// `"visibility": ["app"]`, and dropping the pair together would publish to the
/// agent a tool the author meant to keep for the app.
pub(crate) enum DeclaredVisibility {
    /// No `visibility` key. The spec's `["model", "app"]` default.
    Unset,
    /// The scopes it named.
    Scopes(Vec<UiVisibility>),
    /// A `visibility` that does not decode. Read as a restriction, not as the
    /// default: the author was narrowing the audience, and guessing "everyone"
    /// resolves the ambiguity in the one direction that cannot be taken back.
    Unreadable,
}

/// Reads `_meta.ui.visibility` without decoding the rest of the block.
pub(crate) fn get_tool_visibility(meta: Option<&Value>) -> DeclaredVisibility {
    let Some(visibility) = meta
        .and_then(|meta| meta.get(UI_META_KEY))
        .and_then(|ui| ui.get("visibility"))
    else {
        return DeclaredVisibility::Unset;
    };

    // `null` is how serde spells an absent optional, so it is "not stated"
    // rather than "unreadable".
    if visibility.is_null() {
        return DeclaredVisibility::Unset;
    }

    match serde_json::from_value::<Vec<UiVisibility>>(visibility.clone()) {
        Ok(scopes) => DeclaredVisibility::Scopes(scopes),
        Err(_) => DeclaredVisibility::Unreadable,
    }
}

/// Writes `ui` into a `_meta` object, **merging** rather than replacing.
///
/// `_meta` legitimately carries other keys --
/// `io.modelcontextprotocol/related-task`, whatever the user put there -- so
/// only the `ui` entry is touched. A `_meta` that is absent or is not an object
/// is replaced by a fresh object, since the spec has it as one.
///
/// This is what the builders in this crate use, and what to reach for when
/// stamping the `_meta` of a [`Tool`](crate::types::Tool) or
/// [`Resource`](crate::types::Resource) by hand: assigning the field directly
/// would drop whatever else was in there.
///
/// Serializing `T` cannot fail for the types in this module, and a failure is
/// left as a no-op rather than writing a `null` the host would have to
/// interpret.
///
/// # Examples
/// ```
/// use neva::types::apps::{self, UiToolMeta};
///
/// let mut meta = Some(serde_json::json!({ "vendor/custom": 1 }));
/// apps::set_ui_meta(&mut meta, &UiToolMeta::new("ui://clock/app.html"));
///
/// assert_eq!(
///     meta,
///     Some(serde_json::json!({
///         "vendor/custom": 1,
///         "ui": { "resourceUri": "ui://clock/app.html" }
///     }))
/// );
/// ```
pub fn set_ui_meta<T: Serialize>(meta: &mut Option<Value>, ui: &T) {
    let Ok(value) = serde_json::to_value(ui) else {
        return;
    };
    match meta {
        Some(Value::Object(map)) => {
            map.insert(UI_META_KEY.into(), value);
        }
        _ => {
            let mut map = serde_json::Map::new();
            map.insert(UI_META_KEY.into(), value);
            *meta = Some(Value::Object(map));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_meta_writes_the_spec_shape() {
        let meta = UiToolMeta::new("ui://weather/dashboard")
            .with_visibility([UiVisibility::Model, UiVisibility::App]);

        assert_eq!(
            serde_json::to_value(&meta).unwrap(),
            json!({
                "resourceUri": "ui://weather/dashboard",
                "visibility": ["model", "app"]
            })
        );
    }

    #[test]
    fn tool_meta_omits_unset_visibility() {
        let meta = UiToolMeta::new("ui://weather/dashboard");

        assert_eq!(
            serde_json::to_value(&meta).unwrap(),
            json!({ "resourceUri": "ui://weather/dashboard" })
        );
    }

    #[test]
    fn omitted_visibility_means_both_scopes() {
        let meta: UiToolMeta = serde_json::from_value(json!({
            "resourceUri": "ui://weather/dashboard"
        }))
        .unwrap();

        assert!(meta.is_model_visible());
        assert!(meta.is_app_visible());
    }

    #[test]
    fn app_only_visibility_hides_the_tool_from_the_model() {
        let meta: UiToolMeta = serde_json::from_value(json!({
            "resourceUri": "ui://shop/cart",
            "visibility": ["app"]
        }))
        .unwrap();

        assert!(!meta.is_model_visible());
        assert!(meta.is_app_visible());
    }

    #[test]
    fn tool_meta_roundtrips() {
        let meta = UiToolMeta::new("ui://shop/cart").with_visibility([UiVisibility::App]);
        let json = serde_json::to_value(&meta).unwrap();

        assert_eq!(serde_json::from_value::<UiToolMeta>(json).unwrap(), meta);
    }

    #[test]
    fn csp_writes_camel_case_and_omits_unset_domains() {
        let csp = UiCsp::new()
            .with_connect_domains(["https://api.example.com"])
            .with_base_uri_domains(["https://cdn.example.com"]);

        assert_eq!(
            serde_json::to_value(&csp).unwrap(),
            json!({
                "connectDomains": ["https://api.example.com"],
                "baseUriDomains": ["https://cdn.example.com"]
            })
        );
    }

    #[test]
    fn csp_reads_every_directive() {
        let csp: UiCsp = serde_json::from_value(json!({
            "connectDomains": ["https://api.example.com"],
            "resourceDomains": ["https://cdn.example.com"],
            "frameDomains": ["https://www.youtube.com"],
            "baseUriDomains": ["https://base.example.com"]
        }))
        .unwrap();

        assert_eq!(
            csp.connect_domains.as_deref(),
            Some(["https://api.example.com".to_string()].as_slice())
        );
        assert!(csp.resource_domains.is_some());
        assert!(csp.frame_domains.is_some());
        assert!(csp.base_uri_domains.is_some());
    }

    #[test]
    fn permissions_are_declared_by_presence_of_an_empty_object() {
        let perms = UiPermissions::new().with_camera().with_clipboard_write();

        assert_eq!(
            serde_json::to_value(perms).unwrap(),
            json!({ "camera": {}, "clipboardWrite": {} })
        );
    }

    #[test]
    fn permissions_read_sub_fields_they_do_not_model() {
        // Presence is the declaration; an object carrying anything still counts.
        let perms: UiPermissions =
            serde_json::from_value(json!({ "microphone": { "future": true } })).unwrap();

        assert!(perms.microphone);
        assert!(!perms.camera);
    }

    #[test]
    fn absent_permissions_declare_nothing() {
        let perms: UiPermissions = serde_json::from_value(json!({})).unwrap();

        assert_eq!(perms, UiPermissions::new());
        assert_eq!(serde_json::to_value(perms).unwrap(), json!({}));
    }

    #[test]
    fn permissions_roundtrip() {
        let perms = UiPermissions::new().with_geolocation().with_microphone();
        let json = serde_json::to_value(perms).unwrap();

        assert_eq!(
            serde_json::from_value::<UiPermissions>(json).unwrap(),
            perms
        );
    }

    #[test]
    fn resource_meta_writes_the_spec_shape() {
        let meta = UiResourceMeta::new()
            .with_csp(
                UiCsp::new()
                    .with_connect_domains(["https://api.openweathermap.org"])
                    .with_resource_domains(["https://cdn.jsdelivr.net"]),
            )
            .with_permissions(UiPermissions::new().with_clipboard_write())
            .with_domain("a904794854a047f6.claudemcpcontent.com")
            .with_prefers_border(true);

        assert_eq!(
            serde_json::to_value(&meta).unwrap(),
            json!({
                "csp": {
                    "connectDomains": ["https://api.openweathermap.org"],
                    "resourceDomains": ["https://cdn.jsdelivr.net"]
                },
                "permissions": { "clipboardWrite": {} },
                "domain": "a904794854a047f6.claudemcpcontent.com",
                "prefersBorder": true
            })
        );
    }

    #[test]
    fn empty_resource_meta_is_an_empty_object() {
        assert_eq!(
            serde_json::to_value(UiResourceMeta::new()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn resource_meta_roundtrips() {
        let meta = UiResourceMeta::new()
            .with_csp(UiCsp::new().with_frame_domains(["https://player.vimeo.com"]))
            .with_prefers_border(false);
        let json = serde_json::to_value(&meta).unwrap();

        assert_eq!(
            serde_json::from_value::<UiResourceMeta>(json).unwrap(),
            meta
        );
    }

    #[test]
    fn capability_always_writes_mime_types() {
        assert_eq!(
            serde_json::to_value(AppsCapability::new()).unwrap(),
            json!({ "mimeTypes": ["text/html;profile=mcp-app"] })
        );
        assert_eq!(
            serde_json::to_value(AppsCapability::with_mime_types(Vec::<String>::new())).unwrap(),
            json!({ "mimeTypes": [] })
        );
    }

    #[test]
    fn a_capability_naming_no_types_does_not_declare_support() {
        // The spec makes `mimeTypes` required, so an empty settings object is
        // not a declaration -- this is exactly what `supports_apps` will gate on.
        let cap: AppsCapability = serde_json::from_value(json!({})).unwrap();

        assert!(!cap.supports_html());
    }

    #[test]
    fn a_capability_naming_other_types_does_not_declare_html() {
        let cap: AppsCapability =
            serde_json::from_value(json!({ "mimeTypes": ["text/uri-list"] })).unwrap();

        assert!(!cap.supports_html());
    }

    #[test]
    fn ui_scheme_is_the_declaration() {
        assert!(is_ui_uri("ui://weather/dashboard"));
        assert!(!is_ui_uri("res://weather/dashboard"));
        assert!(!is_ui_uri("ui:/weather/dashboard"));
        assert!(!is_ui_uri(""));
    }

    #[test]
    fn set_ui_creates_the_meta_object_when_there_is_none() {
        let mut meta = None;
        set_ui_meta(&mut meta, &UiToolMeta::new("ui://clock/app.html"));

        assert_eq!(
            meta,
            Some(json!({ "ui": { "resourceUri": "ui://clock/app.html" } }))
        );
    }

    #[test]
    fn set_ui_merges_instead_of_replacing() {
        let mut meta = Some(json!({
            "io.modelcontextprotocol/related-task": { "taskId": "42" },
            "vendor/custom": 1
        }));
        set_ui_meta(&mut meta, &UiToolMeta::new("ui://clock/app.html"));

        assert_eq!(
            meta,
            Some(json!({
                "io.modelcontextprotocol/related-task": { "taskId": "42" },
                "vendor/custom": 1,
                "ui": { "resourceUri": "ui://clock/app.html" }
            }))
        );
    }

    #[test]
    fn set_ui_overwrites_a_previous_ui_entry_only() {
        let mut meta = Some(json!({
            "vendor/custom": 1,
            "ui": { "resourceUri": "ui://old" }
        }));
        set_ui_meta(&mut meta, &UiToolMeta::new("ui://new"));

        assert_eq!(
            meta,
            Some(json!({
                "vendor/custom": 1,
                "ui": { "resourceUri": "ui://new" }
            }))
        );
    }

    #[test]
    fn set_ui_replaces_a_meta_that_is_not_an_object() {
        let mut meta = Some(json!("not an object"));
        set_ui_meta(&mut meta, &UiResourceMeta::new().with_prefers_border(true));

        assert_eq!(meta, Some(json!({ "ui": { "prefersBorder": true } })));
    }

    #[test]
    fn get_ui_reads_back_what_set_ui_wrote() {
        let written = UiToolMeta::new("ui://clock/app.html").with_visibility([UiVisibility::App]);
        let mut meta = None;
        set_ui_meta(&mut meta, &written);

        assert_eq!(get_ui_meta::<UiToolMeta>(meta.as_ref()), Some(written));
    }

    #[test]
    fn get_ui_is_none_for_meta_without_a_ui_entry() {
        assert_eq!(get_ui_meta::<UiToolMeta>(None), None);
        assert_eq!(get_ui_meta::<UiToolMeta>(Some(&json!({}))), None);
        assert_eq!(
            get_ui_meta::<UiToolMeta>(Some(&json!("not an object"))),
            None
        );
        assert_eq!(
            get_ui_meta::<UiToolMeta>(Some(&json!({ "vendor/custom": 1 }))),
            None
        );
    }

    #[test]
    fn get_ui_is_none_for_a_malformed_ui_entry() {
        // A broken extension block must not fail an otherwise valid result.
        let meta = json!({ "ui": { "visibility": "model" } });

        assert_eq!(get_ui_meta::<UiToolMeta>(Some(&meta)), None);
    }

    #[test]
    fn the_deprecated_flat_key_is_still_understood() {
        // A server on an older SDK writes only this. Reading it as "no UI"
        // would silently drop the tool's whole presentation half.
        let meta = json!({ "ui/resourceUri": "ui://weather/dashboard" });

        let ui = get_tool_ui_meta(Some(&meta)).expect("the flat spelling counts");

        assert_eq!(ui.resource_uri.as_deref(), Some("ui://weather/dashboard"));
        assert!(
            ui.is_model_visible(),
            "the flat key says nothing about scope"
        );
    }

    #[test]
    fn the_nested_block_wins_over_the_flat_key() {
        // What the TS SDK writes: both, in agreement. If they ever disagree the
        // spec shape is the one to believe.
        let meta = json!({
            "ui": { "resourceUri": "ui://new", "visibility": ["app"] },
            "ui/resourceUri": "ui://old"
        });

        let ui = get_tool_ui_meta(Some(&meta)).expect("a ui block");

        assert_eq!(ui.resource_uri.as_deref(), Some("ui://new"));
        assert!(!ui.is_model_visible());
    }

    #[test]
    fn the_flat_key_fills_a_nested_block_that_names_no_uri() {
        let meta = json!({
            "ui": { "visibility": ["app"] },
            "ui/resourceUri": "ui://shop/cart"
        });

        let ui = get_tool_ui_meta(Some(&meta)).expect("a ui block");

        assert_eq!(ui.resource_uri.as_deref(), Some("ui://shop/cart"));
        assert!(!ui.is_model_visible(), "the nested visibility survives");
    }

    #[test]
    fn a_tool_with_neither_spelling_has_no_ui() {
        assert_eq!(get_tool_ui_meta(None), None);
        assert_eq!(get_tool_ui_meta(Some(&json!({ "vendor/custom": 1 }))), None);
        assert_eq!(
            get_tool_ui_meta(Some(&json!({ "ui/resourceUri": 42 }))),
            None,
            "a non-string flat key is not a URI"
        );
    }

    #[test]
    fn nothing_neva_writes_carries_the_flat_key() {
        let mut meta = None;
        set_ui_meta(&mut meta, &UiToolMeta::new("ui://clock/app.html"));

        assert_eq!(
            meta,
            Some(json!({ "ui": { "resourceUri": "ui://clock/app.html" } })),
            "the flat key is read-only; it is removed before GA"
        );
    }
}
