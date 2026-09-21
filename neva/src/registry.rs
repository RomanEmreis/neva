//! `server.json` for the [MCP Registry](https://github.com/modelcontextprotocol/registry).
//!
//! The registry is how an MCP server is found and installed: an author
//! publishes a manifest with the `mcp-publisher` CLI, having proved they own
//! the namespace it is published under. Most of what goes in one is already
//! here -- the version, the transport the server is configured for, the crate
//! Cargo is building -- so this writes it out instead of leaving it to be typed
//! by hand.
//!
//! One thing is not derived, on purpose. The registry's `name` is a
//! reverse-DNS identifier inside a namespace the publisher has proved they own
//! (`io.github.<user>/<server>`), and it is not the MCP server name that
//! [`with_name`](crate::app::options::McpOptions::with_name) sets and
//! `serverInfo` carries. Substituting one for the other produces manifests that
//! fail namespace verification, so the name is always given explicitly.
//!
//! # Examples
//! ```rust
//! use neva::App;
//!
//! let app = App::new().with_options(|opt| opt.with_stdio().with_version("0.3.0"));
//!
//! // What the app and Cargo already know, in one call.
//! let manifest = neva::server_manifest!(app, "io.github.romanemreis/weather");
//! let json = manifest.to_json().expect("a complete manifest");
//! ```
//!
//! Publishing, once the manifest is written:
//!
//! ```no_rust
//! mcp-publisher login github
//! mcp-publisher publish
//! ```
//!
//! A Cargo package additionally has its ownership checked against the crate's
//! README: it must carry the server name as visible text, `mcp-name:
//! io.github.<user>/<server>`. crates.io strips HTML comments when it renders
//! markdown, so the hidden-comment form that works for PyPI does not work here.

use crate::error::{Error, ErrorCode};
use crate::types::Icon;
use serde::{Deserialize, Serialize};

pub use input::{Argument, Input, InputFormat, KeyValueInput};
pub use package::{Package, RegistryType, Remote, Transport};

mod input;
mod package;

/// The `server.json` schema this SDK writes.
///
/// Pinned rather than tracking `draft`: a manifest says which version of the
/// format it was written against, and a published one should not change meaning
/// underneath its author. Override it with
/// [`with_schema_url`](ServerManifest::with_schema_url) when a newer one lands
/// before this SDK moves.
pub const SCHEMA_URL: &str =
    "https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json";

/// Limits the registry enforces, checked here so they are reported against the
/// call that set the value rather than against a failed upload.
const MAX_NAME: usize = 200;
const MIN_NAME: usize = 3;
const MAX_DESCRIPTION: usize = 100;
const MAX_TITLE: usize = 100;
const MAX_VERSION: usize = 255;
const MAX_PUBLISHER_METADATA: usize = 4096;
const MAX_ICON_SRC: usize = 255;

/// The forges this SDK can name from a URL, and the hosts that name them.
///
/// Two, because those are the two whose repository URLs have a shape anyone
/// but their own registry can check. A forge outside this list is named by its
/// author with [`Repository::with_source`].
const KNOWN_FORGES: [(&str, &str); 2] = [("github.com", "github"), ("gitlab.com", "gitlab")];

/// The host a source is served from, for the sources this SDK names.
fn known_forge_host(source: &str) -> Option<&'static str> {
    KNOWN_FORGES
        .iter()
        .find(|(_, forge)| *forge == source)
        .map(|(host, _)| *host)
}

/// The forge a URL is on, when it is one of the two this SDK can read off a
/// host. `https://github.com.evil.example/me` is not github.com, so the host
/// has to end where the path begins.
fn forge_of(url: &str) -> Option<&'static str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    KNOWN_FORGES
        .iter()
        .find(|(host, _)| {
            rest.strip_prefix(host)
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .map(|(_, forge)| *forge)
}

/// Where the server's source is, so users and reviewers can read it.
///
/// # Examples
/// ```rust
/// use neva::registry::Repository;
///
/// let repo = Repository::new("https://github.com/RomanEmreis/neva")
///     .with_subfolder("examples/registry");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// Where the source can be browsed and cloned.
    url: String,

    /// Which forge that is -- `github`, `gitlab`, whatever a registry knows
    /// how to verify. Read off the URL when it names one this SDK can, and
    /// otherwise the author's to give.
    source: String,

    /// Where in the repository this server lives, if it is not the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subfolder: Option<String>,

    /// The forge's own identifier for the repository, which survives a rename
    /// and changes if the repository is deleted and recreated. For GitHub:
    /// `gh api repos/<owner>/<repo> --jq '.id'`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

impl Repository {
    /// A repository at `url`, with the forge read off the host.
    ///
    /// `github.com` and `gitlab.com` name themselves; anywhere else -- a
    /// Codeberg, a Gitea, a forge inside a company -- the source is the
    /// author's to give with [`with_source`](Self::with_source), because a
    /// host does not say what verifies it.
    ///
    /// The URL is the repository's own -- `https://<forge>/<owner>/<repo>` and
    /// nothing deeper. A path inside it goes to
    /// [`with_subfolder`](Self::with_subfolder), which is what a registry reads
    /// for a server in a monorepo;
    /// [`validate`](ServerManifest::validate) holds the URL to that shape.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Repository;
    ///
    /// let repo = Repository::new("https://github.com/RomanEmreis/neva");
    ///
    /// let elsewhere =
    ///     Repository::new("https://git.example.com/teams/platform/weather").with_source("gitea");
    /// ```
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            source: forge_of(&url).unwrap_or_default().to_owned(),
            url,
            subfolder: None,
            id: None,
        }
    }

    /// Names the forge, for one this SDK cannot read off a host.
    ///
    /// The official registry verifies `github` and `gitlab` and refuses a
    /// source it does not know, so another name here is for a registry that
    /// takes one -- or for leaving `repository` out, which is optional.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Repository;
    ///
    /// let repo = Repository::new("https://codeberg.org/me/weather").with_source("codeberg");
    /// ```
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Points at the server's own directory inside the repository.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Repository;
    ///
    /// let repo = Repository::new("https://github.com/RomanEmreis/neva")
    ///     .with_subfolder("examples/registry");
    /// ```
    pub fn with_subfolder(mut self, subfolder: impl Into<String>) -> Self {
        self.subfolder = Some(subfolder.into());
        self
    }

    /// Records the forge's own identifier for the repository.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Repository;
    ///
    /// let repo = Repository::new("https://github.com/RomanEmreis/neva")
    ///     .with_id("123456789");
    /// ```
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// Extension metadata the publisher attaches to the manifest.
///
/// One field, because `io.modelcontextprotocol.registry/publisher-provided` is
/// the one key the official registry keeps -- everything else under `_meta` is
/// dropped when publishing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Meta {
    #[serde(
        rename = "io.modelcontextprotocol.registry/publisher-provided",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    publisher_provided: Option<serde_json::Value>,
}

/// A `server.json`: what the registry lists, and how a client gets the server.
///
/// Build one from an [`App`](crate::App) with
/// [`server_manifest`](crate::App::server_manifest) -- or, for the common case,
/// with the [`server_manifest!`](crate::server_manifest) macro, which adds what
/// Cargo knows about the crate it is expanded in.
///
/// # Examples
/// ```rust
/// use neva::registry::{KeyValueInput, Package, ServerManifest};
///
/// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
///     .with_description("Weather forecasts from the national service")
///     .with_package(
///         Package::cargo("weather-mcp", "0.3.0").with_environment_variable(
///             KeyValueInput::new("WEATHER_API_KEY").required().secret(),
///         ),
///     );
///
/// let json = manifest.to_json().expect("a complete manifest");
/// assert!(json.contains("io.github.romanemreis/weather"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerManifest {
    /// Which version of the format this was written against.
    #[serde(rename = "$schema")]
    schema: String,

    /// The registry identifier: `<namespace>/<server>`, inside a namespace the
    /// publisher owns. Not the MCP server name.
    name: String,

    /// What the server does, in a sentence. Required, and at most
    /// 100 characters.
    description: String,

    /// A display name, for registries and clients that show one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,

    /// The version being published.
    version: String,

    /// The server's homepage or documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    website_url: Option<String>,

    /// Where the source is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<Repository>,

    /// Things to install.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    packages: Vec<Package>,

    /// Things already running.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    remotes: Vec<Remote>,

    /// Icons a client may show. Taken from the app's
    /// [`Implementation`](crate::types::Implementation), which declares them in
    /// the same shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icons: Option<Vec<Icon>>,

    /// Publisher-provided extension metadata.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    meta: Option<Meta>,

    /// The transport the app was configured with when this manifest was
    /// started, used for the package [`with_cargo`](Self::with_cargo) adds.
    /// Not part of the manifest: it is an answer about *this* server, and the
    /// JSON asks the question once per package.
    ///
    /// `None` says one thing only: an app was asked and had no transport. That
    /// is not stdio -- such an app cannot start -- so the gap is kept rather
    /// than filled, for [`validate`](Self::validate) to report. Where no app
    /// was involved, here or in [`new`](Self::new), it is stdio.
    #[serde(skip, default = "assumed_transport")]
    transport: Option<Transport>,
}

impl ServerManifest {
    /// A manifest for the server published as `name` at `version`.
    ///
    /// `name` is the registry identifier -- `io.github.<user>/<server>` -- and
    /// not the MCP server name.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0");
    /// ```
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            schema: SCHEMA_URL.to_owned(),
            name: name.into(),
            description: String::new(),
            title: None,
            version: version.into(),
            website_url: None,
            repository: None,
            packages: Vec::new(),
            remotes: Vec::new(),
            icons: None,
            meta: None,
            transport: assumed_transport(),
        }
    }

    /// Fills in what Cargo knows about the crate: the description, the
    /// repository, the website, and a [`cargo` package](Package::cargo) for the
    /// crate itself, spoken to over the transport this manifest was started
    /// with.
    ///
    /// Nothing already set is overwritten, so ordering the calls the other way
    /// round is how you say something different from what Cargo says. The
    /// version is filled only when the app did not set one of its own -- see
    /// [`App::server_manifest`](crate::App::server_manifest).
    ///
    /// Use [`cargo_env!`](crate::cargo_env) to produce the argument: the values
    /// have to be read where *your* crate is compiled, which a function in this
    /// one cannot do.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_cargo(neva::cargo_env!());
    /// ```
    pub fn with_cargo(self, cargo: CargoEnv) -> Self {
        self.with_cargo_package(cargo, |package| package)
    }

    /// [`with_cargo`](Self::with_cargo), with a say in the package it adds:
    /// the environment variables the server reads, the arguments it takes.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{KeyValueInput, ServerManifest};
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_cargo_package(neva::cargo_env!(), |package| {
    ///         package.with_environment_variable(
    ///             KeyValueInput::new("WEATHER_API_KEY")
    ///                 .with_description("API key for the weather provider")
    ///                 .required()
    ///                 .secret(),
    ///         )
    ///     });
    /// ```
    pub fn with_cargo_package<F>(mut self, cargo: CargoEnv, config: F) -> Self
    where
        F: FnOnce(Package) -> Package,
    {
        if self.description.is_empty()
            && let Some(description) = cargo.description
        {
            self.description = description;
        }
        if self.version.is_empty() {
            self.version = cargo.version.clone();
        }
        // Only when the URL names a forge this can read: a source is what a
        // registry verifies against, and inventing one for an unknown host
        // would be worse than leaving the repository out.
        if self.repository.is_none()
            && let Some(url) = cargo.repository.as_deref()
            && forge_of(url).is_some()
        {
            self.repository = Some(Repository::new(url));
        }
        if self.website_url.is_none() {
            self.website_url = cargo.homepage.clone();
        }

        let package = Package::cargo(cargo.name, cargo.version)
            .with_transport(self.transport.clone().unwrap_or_default());
        self.packages.push(config(package));
        self
    }

    /// Says what the server does. Required, and at most 100 characters -- a
    /// line, not a README.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_description("Weather forecasts from the national service");
    /// ```
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Sets the display name registries and clients may show.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest =
    ///     ServerManifest::new("io.github.romanemreis/weather", "0.3.0").with_title("Weather");
    /// ```
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the version being published.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "").with_version("0.3.0");
    /// ```
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Points at the server's homepage or documentation.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_website_url("https://romanemreis.github.io/neva-docs/");
    /// ```
    pub fn with_website_url(mut self, url: impl Into<String>) -> Self {
        self.website_url = Some(url.into());
        self
    }

    /// Records where the source is.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Repository, ServerManifest};
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_repository(Repository::new("https://github.com/RomanEmreis/neva"));
    /// ```
    pub fn with_repository(mut self, repository: Repository) -> Self {
        self.repository = Some(repository);
        self
    }

    /// Adds something to install.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, ServerManifest};
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_package(Package::cargo("weather-mcp", "0.3.0"));
    /// ```
    pub fn with_package(mut self, package: Package) -> Self {
        self.packages.push(package);
        self
    }

    /// Adds a server that is already running somewhere.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Remote, ServerManifest, Transport};
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_remote(Remote::new(Transport::streamable_http("https://mcp.example.com/mcp")));
    /// ```
    pub fn with_remote(mut self, remote: Remote) -> Self {
        self.remotes.push(remote);
        self
    }

    /// Sets the icons a client may show.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    /// use neva::types::Icon;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_icons([Icon::new("https://example.com/icon.png")]);
    /// ```
    pub fn with_icons<I: IntoIterator<Item = Icon>>(mut self, icons: I) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }

    /// Attaches publisher metadata, which the registry keeps verbatim under
    /// `_meta["io.modelcontextprotocol.registry/publisher-provided"]`.
    ///
    /// At most 4KB once serialized; anything larger is refused by
    /// [`validate`](Self::validate) rather than by the upload.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_publisher_metadata(serde_json::json!({ "tool": "neva" }));
    /// ```
    pub fn with_publisher_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.meta = Some(Meta {
            publisher_provided: Some(metadata),
        });
        self
    }

    /// Writes a different `$schema` than the one this SDK pins.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::ServerManifest;
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_schema_url("https://static.modelcontextprotocol.io/schemas/2025-09-29/server.schema.json");
    /// ```
    pub fn with_schema_url(mut self, url: impl Into<String>) -> Self {
        self.schema = url.into();
        self
    }

    /// The registry identifier this manifest publishes under.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The version being published.
    #[inline]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The transport a package added by [`with_cargo`](Self::with_cargo) is
    /// given: what the app was configured with, or `None` when it had none.
    #[inline]
    pub fn transport(&self) -> Option<&Transport> {
        self.transport.as_ref()
    }

    /// What the registry will refuse, asked here instead.
    ///
    /// Covers the rules a manifest can break without looking wrong: a name
    /// outside the reverse-DNS shape, a description over its 100 characters, a
    /// version range where a version belongs, nothing to install or call, and
    /// publisher metadata over 4KB.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, ServerManifest};
    ///
    /// let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_description("Weather forecasts")
    ///     .with_package(Package::cargo("weather-mcp", "0.3.0"));
    ///
    /// assert!(manifest.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), Error> {
        validate_name(&self.name)?;

        match self.description.chars().count() {
            0 => {
                return Err(invalid(
                    "`description` is required: say what the server does in a line",
                ));
            }
            len if len > MAX_DESCRIPTION => {
                return Err(invalid(format!(
                    "`description` is {len} characters; the registry allows {MAX_DESCRIPTION}. \
                     A crate description often runs longer -- set a shorter one with \
                     `with_description`"
                )));
            }
            _ => {}
        }

        if let Some(title) = &self.title {
            // The schema gives it a `minLength` and the registry refuses one
            // that is only whitespace: a title is either said or left out.
            if title.trim().is_empty() {
                return Err(invalid(
                    "`title` is present but blank: say one, or leave it unset",
                ));
            }
            if title.chars().count() > MAX_TITLE {
                return Err(invalid(format!(
                    "`title` is longer than the {MAX_TITLE} characters the registry allows"
                )));
            }
        }

        validate_version(&self.version, "`version`")?;

        if self.packages.is_empty() && self.remotes.is_empty() {
            return Err(invalid(
                "a manifest needs something to install or something to call: add a package with \
                 `with_package` (or `with_cargo`) or a remote with `with_remote`",
            ));
        }

        for package in &self.packages {
            package.validate()?;
        }

        // The schema types these as URIs, and asserts it: `$schema` says
        // draft-07, where a validator checks `format`. An empty string and a
        // bare host are the two ways it goes wrong -- the second is what a
        // `homepage` written as `example.com` in `Cargo.toml` becomes.
        if !is_http_uri(&self.schema) {
            return Err(invalid(format!(
                "`{}` is not a `$schema`: the schema reads it as a URI",
                elided(&self.schema)
            )));
        }

        // A website is one the registry refuses over plain HTTP -- in code,
        // for the reason its comment gives, security. The same goes for an
        // icon below; a transport's URL is the one that may be `http`,
        // because that is where a local server lives.
        if let Some(url) = &self.website_url
            && !is_https_uri(url)
        {
            return Err(invalid(format!(
                "`{}` is not a `websiteUrl`: the registry takes one over `https://` only",
                elided(url)
            )));
        }

        // An icon in a listing is one a client fetches: the schema types the
        // source as a URI and its description asks for an HTTPS URL, with 255
        // characters to say it in. An MCP `Icon` may instead carry the image
        // itself in a `data:` URI -- that is a URI, and not a listing's icon.
        for icon in self.icons.iter().flatten() {
            let src = icon.src.as_ref();
            if !is_https_uri(src) {
                return Err(invalid(format!(
                    "`{}` is not an icon source: the registry fetches one over `https://` only \
                     -- no `http`, and no image inlined in a `data:` URI",
                    elided(src)
                )));
            }
            if src.chars().count() > MAX_ICON_SRC {
                return Err(invalid(format!(
                    "an icon source is at most {MAX_ICON_SRC} characters; this one is {}. A \
                     `data:` URI carrying the image itself rarely fits -- host it instead",
                    src.chars().count()
                )));
            }
        }

        if let Some(repository) = &self.repository {
            let source = repository.source.as_str();
            if source.is_empty() {
                return Err(invalid(
                    "a repository needs a `source`: this URL is on no forge that names itself \
                     from its host, so say which one it is with `with_source`",
                ));
            }
            if !is_forge_repository_url(source, &repository.url) {
                return Err(invalid(match known_forge_host(source) {
                    Some(host) => format!(
                        "`{}` is not a {source} repository URL: a registry reads \
                         `https://{host}/<owner>/<repo>` and nothing deeper -- a path inside the \
                         repository goes to `with_subfolder`",
                        elided(&repository.url)
                    ),
                    None => format!(
                        "`{}` is not a URL a {source} repository could be read from",
                        elided(&repository.url)
                    ),
                }));
            }
        }

        // A package says how to talk to what it installs, and an app with no
        // transport has not answered that. Writing `stdio` anyway would
        // publish an install that cannot work: the same app refuses to start,
        // for the same reason.
        if self.transport.is_none() && !self.packages.is_empty() {
            return Err(invalid(
                "this manifest came from an app with no transport, so there is nothing to \
                 publish a package for: configure one with `with_stdio` or `with_http` before \
                 `server_manifest`, or build the manifest with `ServerManifest::new`",
            ));
        }

        // `remotes` is where a server that is already running is named, and a
        // running server is not one the client spawns: the schema admits only
        // the HTTP transports there.
        if self.remotes.iter().any(|remote| remote.is_stdio()) {
            return Err(invalid(
                "a remote is a server that is already running, so it cannot be reached over \
                 stdio: give it a URL with `Transport::streamable_http`",
            ));
        }

        // A remote is reached over the public internet, and the registry says
        // so twice: HTTPS only, and never a loopback host. A package's
        // transport is the opposite case -- it names where the thing just
        // installed will answer, which is routinely `http://127.0.0.1`.
        for remote in &self.remotes {
            let Some(url) = remote.transport.url() else {
                continue;
            };
            if !is_transport_url(url) {
                return Err(invalid(format!(
                    "`{}` is not a URL a client could call: a `streamable-http` transport is \
                     `http://` or `https://`, with no spaces",
                    elided(url)
                )));
            }

            // What is around a `{name}` still has to be a URL, so it is
            // stood in for before the host is read -- as the registry does.
            let dialable = without_templates(url);
            if !is_https_uri(&dialable) {
                return Err(invalid(format!(
                    "`{}` is not a remote's URL: the registry reaches one over `https://` only",
                    elided(url)
                )));
            }

            if is_loopback(&dialable) {
                return Err(invalid(format!(
                    "`{}` is a remote on this machine: a remote is a server others can reach, \
                     and a package is the entry for one they run themselves",
                    elided(url)
                )));
            }

            declared_templates(url, remote.variables.keys().map(String::as_str), "remote")?;
        }

        if let Some(metadata) = self
            .meta
            .as_ref()
            .and_then(|m| m.publisher_provided.as_ref())
        {
            let len = serde_json::to_vec(metadata).map_err(Error::from)?.len();
            if len > MAX_PUBLISHER_METADATA {
                return Err(invalid(format!(
                    "publisher metadata is {len} bytes; the registry allows \
                     {MAX_PUBLISHER_METADATA}"
                )));
            }
        }

        Ok(())
    }

    /// The manifest as the JSON that goes in `server.json`, pretty-printed and
    /// newline-terminated.
    ///
    /// [`validate`](Self::validate) runs first: there is no use for a
    /// `server.json` the registry will refuse.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, ServerManifest};
    ///
    /// let json = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
    ///     .with_description("Weather forecasts")
    ///     .with_package(Package::cargo("weather-mcp", "0.3.0"))
    ///     .to_json()
    ///     .expect("a complete manifest");
    ///
    /// assert!(json.ends_with('\n'));
    /// ```
    pub fn to_json(&self) -> Result<String, Error> {
        self.validate()?;
        let mut json = serde_json::to_string_pretty(self).map_err(Error::from)?;
        json.push('\n');
        Ok(json)
    }
}

impl Package {
    /// What the registry will refuse about this entry.
    ///
    /// Every rule below is keyed off the spelling that goes on the wire rather
    /// than the Rust variant that produced it. The registry reads a string,
    /// and [`Other`](RegistryType::Other) can hold one this SDK also has a
    /// variant for -- so a rule that matched on the variant would let
    /// `Other("mcpb")` walk past a requirement the upload applies anyway.
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.identifier.is_empty() {
            return Err(invalid("a package needs an `identifier`"));
        }

        if self.identifier.contains(' ') {
            return Err(invalid(format!(
                "a package identifier carries no spaces: `{}`",
                elided(&self.identifier)
            )));
        }

        if self.registry_type.as_str().is_empty() {
            return Err(invalid("a package needs a `registryType`"));
        }

        if self.version == "latest" {
            return Err(invalid(
                "a package version must name one version; `latest` moves under whoever installed it",
            ));
        }

        validate_version(&self.version, "a package version")?;

        if self.registry_type.as_str() == "mcpb" {
            if self.file_sha256.is_none() {
                return Err(invalid(
                    "an MCPB package must carry the file's SHA-256: set it with `with_file_sha256`",
                ));
            }
            validate_mcpb_identifier(&self.identifier)?;
        }

        // Carrying one is half the requirement; the schema spells the other
        // half `^[a-f0-9]{64}$`. An empty string satisfies "present" and
        // nothing else, and a digest pasted in upper case is refused just the
        // same -- by the registry, if not here.
        if let Some(hash) = &self.file_sha256
            && !is_sha256(hash)
        {
            return Err(invalid(format!(
                "`fileSha256` is a SHA-256 digest written as 64 lowercase hex characters; \
                 `{}` is not one",
                elided(hash)
            )));
        }

        // What a client dials once this package is installed. It may be a
        // template too, filled from what the package asks the user for.
        if let Some(url) = self.transport.url() {
            if !is_transport_url(url) {
                return Err(invalid(format!(
                    "`{}` is not a URL a client could call: a `streamable-http` transport is \
                     `http://` or `https://`, with no spaces",
                    elided(url)
                )));
            }
            // A port the OS assigns when the listener starts is not one a
            // client can be told about in advance.
            if without_templates(url)
                .parse::<http::Uri>()
                .ok()
                .and_then(|uri| uri.port_u16())
                == Some(0)
            {
                return Err(invalid(format!(
                    "`{}` names port 0, which the OS replaces when the listener starts: a \
                     manifest cannot say which port that will be, so name one with \
                     `with_transport`",
                    elided(url)
                )));
            }

            let declared = self
                .environment_variables
                .iter()
                .map(|variable| variable.name.as_str())
                .chain(
                    self.runtime_arguments
                        .iter()
                        .chain(self.package_arguments.iter())
                        .filter_map(Argument::template_name),
                );
            declared_templates(url, declared, "package")?;
        }

        // Each type the registry knows has its own answer about
        // `registryBaseUrl`, and two of them are "not at all": an OCI or MCPB
        // identifier carries its host already, and the official registry
        // refuses the field rather than ignoring it.
        match (
            self.registry_type.as_str(),
            self.registry_base_url.as_deref(),
        ) {
            ("cargo", Some(url)) if url != CRATES_IO => Err(invalid(format!(
                "a Cargo package comes from `{CRATES_IO}` or from nowhere the registry accepts; \
                 `{url}` is refused. Leaving it unset is the same thing and says less"
            ))),
            (kind @ ("oci" | "mcpb"), Some(_)) => Err(invalid(format!(
                "an `{kind}` package must not carry a `registryBaseUrl`: its identifier names \
                 the host already"
            ))),
            // Which registry an unnamed type comes from is that type's
            // business; that the field is a URL is the schema's, and it types
            // this one as a URI.
            (_, Some(url)) if !is_http_uri(url) => Err(invalid(format!(
                "`{}` is not a `registryBaseUrl`: the schema reads it as a URI, so it is \
                 `http://` or `https://`",
                elided(url)
            ))),
            _ => Ok(()),
        }
    }
}

/// The one Cargo registry the official registry accepts: it defaults an unset
/// `registryBaseUrl` to this and refuses every other value.
const CRATES_IO: &str = "https://crates.io";

/// What a manifest that never met an app assumes: a package it derives is for
/// a binary the client spawns. Read back from JSON it is the same answer --
/// there is no app on that path either, and the packages already carry the
/// transports they were published with.
fn assumed_transport() -> Option<Transport> {
    Some(Transport::Stdio)
}

/// The schema's `^https?://[^\s]+$` for a transport URL.
///
/// A pattern rather than `format: uri`, which is why this is looser than
/// [`is_uri`] and has to stay so: a transport URL may hold a `{variable}` for a
/// remote to fill in, and braces are not URI characters. The `draft` schema
/// also admits a URL that *begins* with one, and the version pinned here does
/// not -- so one written that way is refused, as the registry would refuse it.
fn is_transport_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    matches!(rest, Some(rest) if !rest.is_empty() && !rest.chars().any(char::is_whitespace))
}

/// An MCPB package has no registry to look it up in: its identifier is the
/// URL the archive is downloaded from, and the registry holds that URL to more
/// than being one.
///
/// Everything it asks that can be asked of a string is asked here -- HTTPS, a
/// release asset on GitHub or GitLab, and the `mcp` it wants to see in the URL
/// somewhere. What is left to the upload is the one thing this cannot do: a
/// `HEAD` to see that the file is there.
fn validate_mcpb_identifier(identifier: &str) -> Result<(), Error> {
    let refused = |why: &str| {
        Err(invalid(format!(
            "`{}` is not an MCPB download URL: {why}",
            elided(identifier)
        )))
    };

    let not_a_url = "it is not a URL, and an MCPB identifier is the archive's own";
    let Ok(uri) = identifier.parse::<http::Uri>() else {
        return refused(not_a_url);
    };

    match uri.scheme_str() {
        Some("https") => {}
        // `http::Uri` reads request targets too, so a bare name parses with no
        // scheme at all -- that is a different mistake from naming `http`.
        None => return refused(not_a_url),
        Some(_) => return refused("the registry downloads one over `https://` only"),
    }

    let host = uri.host().unwrap_or_default().to_ascii_lowercase();
    let forge = match host.trim_start_matches("www.") {
        "github.com" => "github",
        "gitlab.com" => "gitlab",
        _ => {
            return refused(
                "an MCPB archive is a release asset on github.com or gitlab.com, and nowhere else",
            );
        }
    };

    let segments = uri.path().split('/').skip(1).collect::<Vec<_>>();
    let is_release_asset = match forge {
        // `/owner/repo/releases/download/<tag>/<file>`
        "github" => matches!(
            segments.as_slice(),
            [owner, repo, "releases", "download", tag, file]
                if ![owner, repo, tag, file].iter().any(|s| s.is_empty())
        ),
        // `/<project>/-/releases/<tag>/downloads/<file>`, or
        // `/<project>/-/package_files/<id>/download`. A project path may nest
        // groups, so it is whatever precedes GitLab's `/-/` delimiter.
        _ => match segments.iter().position(|segment| *segment == "-") {
            Some(delimiter) if delimiter > 0 => {
                matches!(
                    &segments[delimiter + 1..],
                    ["releases", tag, "downloads", file]
                        if !tag.is_empty() && !file.is_empty()
                ) || matches!(
                    &segments[delimiter + 1..],
                    ["package_files", id, "download"]
                        if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
                )
            }
            _ => false,
        },
    };

    if !is_release_asset {
        return refused(match forge {
            "github" => "a GitHub release asset is `/owner/repo/releases/download/<tag>/<file>`",
            _ => {
                "a GitLab release asset is `/<project>/-/releases/<tag>/downloads/<file>` or \
                  `/<project>/-/package_files/<id>/download`"
            }
        });
    }

    // Odd, and enforced: the registry wants to see what the archive is for.
    if !identifier.to_ascii_lowercase().contains("mcp") {
        return refused(
            "the registry asks the URL to say `mcp` somewhere -- name the asset for it",
        );
    }

    Ok(())
}

/// The schema's `format: uri`, with an `http` or `https` scheme -- every URI
/// field a manifest carries.
///
/// Answered by [`http::Uri`], which neva links unconditionally, rather than by
/// a parser written here: URI syntax has more corners than it looks (an IPv6
/// literal's brackets, a scheme's case, what may appear in an authority) and
/// each one was a separate bug in the hand-rolled version this replaced.
///
/// The one thing that parser cannot answer alone is the scheme. `http::Uri`
/// reads request targets as well as URLs, so `example.com` parses as an
/// authority and `/mcp` as a path, both without one -- and `example.com` is
/// exactly what a `homepage` in `Cargo.toml` is often written as.
fn is_http_uri(value: &str) -> bool {
    value
        .parse::<http::Uri>()
        .is_ok_and(|uri| matches!(uri.scheme_str(), Some("http" | "https")))
}

/// What a client dials, given where a server listens.
///
/// The two are not the same string. `0.0.0.0` and `[::]` are bind wildcards --
/// "every interface" -- and nothing connects to them; for a package entry,
/// whose server the user runs on their own machine, that is the loopback. The
/// rest of the URL is left alone.
///
/// A port of `0` has no such translation: the OS picks one when the listener
/// starts, so it is not knowable here at all, and
/// [`validate`](ServerManifest::validate) refuses it.
///
/// Gated with its one caller: only an HTTP server has an address to translate.
#[cfg(feature = "http-server")]
pub(crate) fn dial_url(url: String) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url;
    };
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };

    match [("0.0.0.0", "127.0.0.1"), ("[::]", "[::1]")]
        .into_iter()
        .find_map(|(wildcard, loopback)| {
            authority
                .strip_prefix(wildcard)
                .filter(|port| port.is_empty() || port.starts_with(':'))
                .map(|port| format!("{loopback}{port}"))
        }) {
        Some(dialable) => format!("{scheme}://{dialable}{path}"),
        None => url,
    }
}

/// Every `{name}` in a URL has to be one the entry around it declares --
/// otherwise a client is handed a URL with a hole in it and nothing to fill it
/// from.
///
/// A remote declares them in its `variables`; a package in the environment
/// variables and arguments it asks the user for, by flag name or value hint.
fn declared_templates<'a>(
    url: &str,
    declared: impl Iterator<Item = &'a str>,
    entry: &str,
) -> Result<(), Error> {
    let declared = declared.collect::<Vec<_>>();
    match template_variables(url).find(|name| !declared.contains(name)) {
        None => Ok(()),
        Some(missing) => Err(invalid(format!(
            "`{}` in `{}` is filled in by nothing this {entry} declares{}",
            elided(missing),
            elided(url),
            match declared.is_empty() {
                true => String::new(),
                false => format!(" -- it has {}", declared.join(", ")),
            }
        ))),
    }
}

/// A host on the machine the client runs on, which a remote may not be.
fn is_loopback(url: &str) -> bool {
    let host = url
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_ascii_lowercase))
        .unwrap_or_default();

    host == "localhost" || host == "127.0.0.1" || host.ends_with(".localhost")
}

/// The `{name}`s a URL asks to have filled in, as the registry's own
/// `\{([^}]+)\}` finds them.
fn template_variables(url: &str) -> impl Iterator<Item = &str> {
    url.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}'))
        .map(|(name, _)| name)
}

/// The URL with every `{name}` stood in for, so that what is around them can
/// be parsed. The registry does the same before it looks at a remote's host.
fn without_templates(url: &str) -> String {
    let mut plain = String::with_capacity(url.len());
    let mut rest = url;
    while let Some((before, after)) = rest.split_once('{') {
        match after.split_once('}') {
            Some((_, after)) => {
                plain.push_str(before);
                plain.push('x');
                rest = after;
            }
            None => break,
        }
    }
    plain.push_str(rest);
    plain
}

/// The same, for the two fields the registry refuses over plain HTTP: a
/// `websiteUrl` and an icon's source, both checked in its code rather than
/// merely described, and both for the reason it gives -- security.
///
/// A transport's URL is deliberately not one of them: `http://127.0.0.1:3000`
/// is where a server under development answers, and that is what
/// [`App::server_manifest`](crate::App::server_manifest) derives from one.
fn is_https_uri(value: &str) -> bool {
    value
        .parse::<http::Uri>()
        .is_ok_and(|uri| uri.scheme_str() == Some("https"))
}

/// The registry's own `^https?://(www\.)?<host>/[\w.-]+/[\w.-]+/?$`: a
/// repository URL names the repository and stops there.
///
/// Keyed off the spelling, so `Other("github")` is checked as GitHub is. A
/// forge this SDK does not name has no host and no shape to hold it to -- only
/// whoever verifies that forge knows them -- so the URL is checked as a URL and
/// no further.
fn is_forge_repository_url(source: &str, url: &str) -> bool {
    let Some(host) = known_forge_host(source) else {
        return is_http_uri(url);
    };

    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let Some(rest) = rest
        .strip_prefix(host)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };

    let mut segments = rest.strip_suffix('/').unwrap_or(rest).split('/');
    let (Some(owner), Some(repo), None) = (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    [owner, repo].iter().all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    })
}

/// The schema's `^[a-f0-9]{64}$`, checked by hand rather than by a regex
/// engine this crate would otherwise not link.
fn is_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() && b <= b'f')
}

/// Keeps a rejected value quotable: what an author pasted by mistake can be
/// arbitrarily long, and an error message is not the place to repeat it.
fn elided(value: &str) -> String {
    const MAX: usize = 72;
    match value.char_indices().nth(MAX) {
        None => value.to_owned(),
        Some((end, _)) => format!("{}...", &value[..end]),
    }
}

/// The reverse-DNS shape the registry requires, checked by hand: one `/`, a
/// namespace of letters, digits, dots and dashes, and a server name that also
/// allows underscores.
fn validate_name(name: &str) -> Result<(), Error> {
    let len = name.chars().count();
    if !(MIN_NAME..=MAX_NAME).contains(&len) {
        return Err(invalid(format!(
            "`name` is {len} characters; the registry allows {MIN_NAME} to {MAX_NAME}"
        )));
    }

    let Some((namespace, server)) = name.split_once('/') else {
        return Err(invalid(format!(
            "`name` is the registry identifier `<namespace>/<server>`, not the MCP server name: \
             `{name}` has no `/`. A GitHub namespace looks like `io.github.<user>/<server>`"
        )));
    };

    if server.contains('/') {
        return Err(invalid(format!("`name` has more than one `/`: `{name}`")));
    }

    if namespace.is_empty()
        || !namespace
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(invalid(format!(
            "the namespace in `{name}` may hold letters, digits, `.` and `-` only"
        )));
    }

    if server.is_empty()
        || !server
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(invalid(format!(
            "the server name in `{name}` may hold letters, digits, `.`, `-` and `_` only"
        )));
    }

    Ok(())
}

/// A version, not a range: the registry rejects `^1.2.3`, `~1.2.3`, `>=1.2.3`,
/// `1.x` and `1.*`, which is what a Cargo dependency requirement looks like.
fn validate_version(version: &str, what: &str) -> Result<(), Error> {
    if version.is_empty() {
        return Err(invalid(format!(
            "{what} is required: set one with `with_version`, or let `with_cargo` take the \
             crate's"
        )));
    }

    if version.chars().count() > MAX_VERSION {
        return Err(invalid(format!(
            "{what} is longer than the {MAX_VERSION} characters the registry allows"
        )));
    }

    if looks_like_version_range(version) {
        return Err(invalid(format!(
            "{what} must name one version, not a range: `{}`",
            elided(version)
        )));
    }

    Ok(())
}

/// The registry's own `looksLikeVersionRange`, mirrored by hand because this
/// crate links no regex engine: a comparator in front of a version, two
/// versions joined by ` - ` or `||`, or a dotted version with a wildcard in it.
///
/// Mirrored, not extended. The registry does not enforce semver ("we decided
/// that we would not"), so a string that merely looks unusual -- `1.2.3,
/// <2.0.0`, or the `1.2.3 <2.0.0` that upstream's patterns also let past -- is
/// published as written, and refusing it here would refuse a manifest that
/// uploads fine.
///
/// Two deliberate departures, in opposite directions:
///
/// * **A leading comparator is a range, whatever follows it.** Upstream asks
///   the whole string to be one comparator and one version, so `^1.2.3 ||
///   2.0.0` and `>=1.2.3 <2.0.0` -- a dependency requirement pasted where a
///   version goes -- match none of its four patterns. No version begins with a
///   comparator, so nothing real is refused by saying so here.
/// * **A wildcard is looked for in the version, not in the text.** Upstream
///   asks whether the whole string contains an `x`, which also catches a
///   prerelease like `1.2.3-exp`. Refusing a valid prerelease is the worse
///   mistake.
fn looks_like_version_range(version: &str) -> bool {
    let version = version.trim();

    // `^1.2.3`, `>= 1.2.3`, `>=1.2.3 <2.0.0`. Whatever comes after it, a
    // comparator in front says this names a set of versions rather than one.
    if version.starts_with(['^', '~', '>', '<', '=']) {
        return true;
    }

    // `1.2.3 - 2.0.0`, `1.2 || 1.3`. The hyphen needs the space in front of it;
    // without one it is a prerelease.
    for separator in [" -", "||"] {
        if version.contains(separator) {
            return version
                .split(separator)
                .all(|atom| is_version_atom(atom.trim()));
        }
    }

    is_wildcard_version(version)
}

/// `v?\d+(\.\d+){0,3}(-[0-9A-Za-z.-]+)?` -- one version, as the range
/// patterns spell the things they join.
fn is_version_atom(value: &str) -> bool {
    let (numbers, prerelease) = split_prerelease(value);
    let numbers = numbers.split('.').collect::<Vec<_>>();

    prerelease.is_none_or(is_prerelease)
        && matches!(numbers.len(), 1..=4)
        && numbers.iter().all(|number| is_number(number))
}

/// `(v?\d+|x|X|\*)(\.(\d+|x|X|\*)){1,2}` holding at least one wildcard:
/// `1.x`, `1.2.*`, `x.2.3`.
fn is_wildcard_version(value: &str) -> bool {
    let (numbers, prerelease) = split_prerelease(value);
    let segments = numbers.split('.').collect::<Vec<_>>();

    prerelease.is_none_or(is_prerelease)
        && matches!(segments.len(), 2..=3)
        && segments
            .iter()
            .all(|segment| is_wildcard(segment) || is_number(segment))
        && segments.iter().any(|segment| is_wildcard(segment))
}

/// Splits `1.2.3-beta.1` into its numbers and its prerelease, dropping the
/// leading `v` the range patterns allow.
fn split_prerelease(value: &str) -> (&str, Option<&str>) {
    let value = value.strip_prefix('v').unwrap_or(value);
    match value.split_once('-') {
        Some((numbers, prerelease)) => (numbers, Some(prerelease)),
        None => (value, None),
    }
}

fn is_number(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit())
}

fn is_wildcard(segment: &str) -> bool {
    matches!(segment, "x" | "X" | "*")
}

fn is_prerelease(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

#[inline]
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidRequest, message.into())
}

/// What Cargo knows about the crate being built.
///
/// Produced by [`cargo_env!`](crate::cargo_env), which reads it where your
/// crate is compiled -- `CARGO_PKG_NAME` inside this crate would say `neva`.
///
/// # Examples
/// ```rust
/// let cargo = neva::cargo_env!();
///
/// assert!(!cargo.name().is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoEnv {
    name: String,
    version: String,
    description: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
}

impl CargoEnv {
    /// The crate `name` at `version`. Prefer [`cargo_env!`](crate::cargo_env),
    /// which fills these in from Cargo itself.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::CargoEnv;
    ///
    /// let cargo = CargoEnv::new("weather-mcp", "0.3.0");
    /// ```
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            description: None,
            repository: None,
            homepage: None,
        }
    }

    /// The crate's description. An empty one is no description: that is what
    /// Cargo hands over for a field nobody set.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::CargoEnv;
    ///
    /// let cargo = CargoEnv::new("weather-mcp", "0.3.0").with_description("Weather forecasts");
    /// ```
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = non_empty(description);
        self
    }

    /// The crate's repository URL.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::CargoEnv;
    ///
    /// let cargo = CargoEnv::new("weather-mcp", "0.3.0")
    ///     .with_repository("https://github.com/RomanEmreis/neva");
    /// ```
    pub fn with_repository(mut self, repository: impl Into<String>) -> Self {
        self.repository = non_empty(repository);
        self
    }

    /// The crate's homepage.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::CargoEnv;
    ///
    /// let cargo = CargoEnv::new("weather-mcp", "0.3.0")
    ///     .with_homepage("https://romanemreis.github.io/neva-docs/");
    /// ```
    pub fn with_homepage(mut self, homepage: impl Into<String>) -> Self {
        self.homepage = non_empty(homepage);
        self
    }

    /// The crate's name -- the identifier it has on crates.io.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The crate's version.
    #[inline]
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[inline]
fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    (!value.is_empty()).then_some(value)
}

/// What Cargo knows about the crate this macro is expanded in: name, version,
/// description, repository and homepage.
///
/// A macro rather than a function because these are compile-time values of
/// *your* crate. A function in this one would read neva's.
///
/// Works in a `build.rs` as well, which is where a manifest is often written.
///
/// # Examples
/// ```rust
/// use neva::registry::ServerManifest;
///
/// let manifest =
///     ServerManifest::new("io.github.romanemreis/weather", "").with_cargo(neva::cargo_env!());
/// ```
#[cfg(feature = "registry")]
#[macro_export]
macro_rules! cargo_env {
    () => {
        $crate::registry::CargoEnv::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .with_description(env!("CARGO_PKG_DESCRIPTION"))
            .with_repository(env!("CARGO_PKG_REPOSITORY"))
            .with_homepage(env!("CARGO_PKG_HOMEPAGE"))
    };
}

/// The whole manifest in one call: what the app knows, plus what Cargo knows
/// about the crate this macro is expanded in.
///
/// `$app.server_manifest($name).with_cargo(cargo_env!())`, which is
/// [`App::server_manifest`](crate::App::server_manifest) and
/// [`ServerManifest::with_cargo`] -- reach for those directly when the manifest
/// needs more saying about it.
///
/// # Examples
/// ```rust
/// use neva::App;
///
/// let app = App::new().with_options(|opt| opt.with_stdio().with_version("0.3.0"));
///
/// let manifest = neva::server_manifest!(app, "io.github.romanemreis/weather");
/// ```
#[cfg(all(feature = "registry", feature = "server"))]
#[macro_export]
macro_rules! server_manifest {
    ($app:expr, $name:expr) => {
        $app.server_manifest($name).with_cargo($crate::cargo_env!())
    };
}

#[cfg(feature = "server")]
impl crate::App {
    /// Starts a `server.json` from what this app already knows: the version it
    /// reports and the transport it is configured with.
    ///
    /// `name` is the registry identifier -- `io.github.<user>/<server>` -- and
    /// is never derived: it belongs to a namespace the publisher proves they
    /// own, and the MCP server name
    /// ([`with_name`](crate::app::options::McpOptions::with_name)) is a
    /// different string under different rules.
    ///
    /// The version comes from
    /// [`with_version`](crate::app::options::McpOptions::with_version), and
    /// whether it was called is what decides -- not what it was called with.
    /// An app that never set one reports neva's own version, which is no
    /// server's version, so the field is left empty for
    /// [`with_cargo`](ServerManifest::with_cargo) to fill from the crate --
    /// and, failing that, refused by [`validate`](ServerManifest::validate).
    /// A version set deliberately to whatever neva happens to be at is still
    /// the author's, and is kept.
    ///
    /// The transport is the one the app is configured with. An app that has
    /// none is not a stdio app -- it cannot start -- so nothing is invented
    /// for it; `validate` refuses a package derived from such an app.
    ///
    /// # Examples
    /// ```rust
    /// use neva::App;
    /// use neva::registry::Package;
    ///
    /// let app = App::new().with_options(|opt| opt.with_stdio().with_version("0.3.0"));
    ///
    /// let manifest = app
    ///     .server_manifest("io.github.romanemreis/weather")
    ///     .with_description("Weather forecasts from the national service")
    ///     .with_package(Package::cargo("weather-mcp", "0.3.0"));
    ///
    /// assert!(manifest.to_json().is_ok());
    /// ```
    pub fn server_manifest(&self, name: impl Into<String>) -> ServerManifest {
        // An app that never called `with_version` reports neva's own version,
        // and publishing that as the server's would be wrong in a way nobody
        // would notice. The options record the call rather than this comparing
        // strings: a version deliberately set to the one neva happens to be at
        // is the author's, and a heuristic would take it away from them.
        let version = match self.options.version_is_explicit {
            true => self.options.implementation.version.as_str(),
            false => "",
        };

        let mut manifest = ServerManifest::new(name, version);
        // `None` when the app has no transport at all, which is not a stdio
        // server -- see `validate`.
        manifest.transport = self.options.configured_transport();
        manifest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ServerManifest {
        ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather forecasts from the national service")
            .with_package(Package::cargo("weather-mcp", "0.3.0"))
    }

    /// The whole document, against the shape the registry's own Cargo example
    /// shows: `$schema` pinned, the name as given, one cargo package over
    /// stdio.
    #[test]
    fn a_cargo_manifest_matches_the_documented_shape() {
        let json: serde_json::Value =
            serde_json::from_str(&manifest().to_json().expect("a complete manifest"))
                .expect("valid JSON");

        assert_eq!(
            json,
            serde_json::json!({
                "$schema": SCHEMA_URL,
                "name": "io.github.romanemreis/weather",
                "description": "Weather forecasts from the national service",
                "version": "0.3.0",
                "packages": [{
                    "registryType": "cargo",
                    "identifier": "weather-mcp",
                    "version": "0.3.0",
                    "transport": { "type": "stdio" },
                }],
            })
        );
    }

    /// The trap the whole feature is arranged around: the registry name is not
    /// the MCP server name, and a plain one is refused rather than published
    /// into a namespace nobody owns.
    #[test]
    fn a_name_that_is_not_a_registry_identifier_is_refused() {
        let err = manifest()
            .with_version("0.3.0")
            .with_description("Weather")
            .with_name_for_test("weather")
            .validate()
            .expect_err("a bare MCP server name is not a registry identifier");

        assert!(err.to_string().contains("no `/`"), "got: {err}");
    }

    /// Namespaces and server names have different character sets, and both are
    /// stricter than a Rust crate name.
    #[test]
    fn the_reverse_dns_shape_is_checked_on_both_sides_of_the_slash() {
        for (name, expected) in [
            ("io.github.me/weather_mcp", None),
            ("io.github.me/weather", None),
            ("io github/weather", Some("namespace")),
            ("io.github.me/wea ther", Some("server name")),
            ("io.github.me/a/b", Some("more than one `/`")),
            // Short, but the registry's own floor is three characters.
            ("a/b", None),
            ("a/", Some("2 characters")),
        ] {
            let result = validate_name(name);
            match expected {
                None => assert!(result.is_ok(), "`{name}` is a valid identifier"),
                Some(fragment) => {
                    let err = result.expect_err("an invalid identifier is refused");
                    assert!(
                        err.to_string().contains(fragment),
                        "`{name}`: expected `{fragment}`, got: {err}"
                    );
                }
            }
        }
    }

    /// A crate description is written for crates.io, which has no 100-character
    /// limit. Saying so here is the difference between fixing one line and
    /// reading a rejection from the publisher.
    #[test]
    fn a_description_over_the_limit_is_refused_by_length() {
        let err = manifest()
            .with_description("x".repeat(MAX_DESCRIPTION + 1))
            .validate()
            .expect_err("the registry allows 100 characters");

        assert!(err.to_string().contains("101 characters"), "got: {err}");
        assert!(err.to_string().contains("with_description"), "got: {err}");
    }

    /// `CARGO_PKG_VERSION` is a version; a dependency requirement is not. The
    /// four shapes are the registry's own: a comparator, a hyphen range, an
    /// `||` range, and a wildcard anywhere in a dotted version.
    #[test]
    fn a_version_range_is_refused() {
        for range in [
            "^1.2.3",
            "~1.2.3",
            ">=1.2.3",
            "> 1.2.3",
            "=1.2.3",
            // Compound ranges: a comparator in front settles it, whatever
            // follows. Upstream's patterns want the whole string to be one
            // comparator and one version, so these match none of them.
            "^1.2.3 || 2.0.0",
            ">=1.2.3 <2.0.0",
            "~1.2 || ^2.0",
            "1.*",
            "1.x",
            "1.2.X",
            // A wildcard away from the end counts too.
            "x.2.3",
            "1.x.3",
            "1.2.3 - 2.0.0",
            "1 - 2",
            "1.2 || 1.3",
            "1.0.0-alpha || 2.0.0",
        ] {
            let err = manifest()
                .with_version(range)
                .validate()
                .expect_err("a range is not a version");
            assert!(err.to_string().contains("not a range"), "{range}: {err}");

            // A package version is held to the same rule.
            assert!(
                Package::cargo("weather-mcp", range).validate().is_err(),
                "a package version is checked too: {range}"
            );
        }
    }

    /// And the versions that only resemble one are published as written: the
    /// registry does not enforce semver, so neither does this.
    #[test]
    fn a_version_that_is_not_a_range_is_left_alone() {
        for version in [
            "1.0.2",
            "0.3.0",
            "v1.2.3",
            "1.2.3-alpha.1",
            // The hyphen of a prerelease is not the hyphen of a range...
            "1.0.0-rc.1",
            // ...and an `x` inside one is not a wildcard, though the registry's
            // own check reads the whole string and would say otherwise.
            "1.2.3-exp.sha.5114f85",
            // Not versions anyone should publish, and not ranges any of the
            // patterns name either -- the registry takes them, so this does
            // not refuse them.
            "1.2.3, <2.0.0",
            "1.2.3 <2.0.0",
        ] {
            assert!(
                manifest().with_version(version).validate().is_ok(),
                "`{version}` names one version"
            );
        }
    }

    /// Nothing to install and nothing to call is a listing with no way in.
    #[test]
    fn a_manifest_with_neither_packages_nor_remotes_is_refused() {
        let err = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .validate()
            .expect_err("a manifest needs a package or a remote");

        assert!(err.to_string().contains("with_package"), "got: {err}");

        // ...and a remote alone is enough.
        assert!(
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_remote(Remote::new(Transport::streamable_http(
                    "https://mcp.example.com/mcp"
                )))
                .validate()
                .is_ok()
        );
    }

    /// The 4KB ceiling is the registry's, and it is measured on the serialized
    /// JSON rather than on what went in.
    #[test]
    fn oversized_publisher_metadata_is_refused() {
        let err = manifest()
            .with_publisher_metadata(serde_json::json!({ "blob": "x".repeat(4096) }))
            .validate()
            .expect_err("the registry allows 4096 bytes");

        assert!(err.to_string().contains("4096"), "got: {err}");

        assert!(
            manifest()
                .with_publisher_metadata(serde_json::json!({ "tool": "neva" }))
                .validate()
                .is_ok()
        );
    }

    /// An MCPB package is downloaded from a release rather than from a
    /// registry, so its hash is what says the file is the published one --
    /// and its identifier is the URL that file is at, held to what the
    /// registry asks of one.
    #[test]
    fn an_mcpb_package_is_a_release_asset_with_a_hash() {
        const DIGEST: &str = "fe333e598595000ae021bd27117db32ec69af6987f507ba7a63c90638ff633ce";
        const ASSET: &str =
            "https://github.com/example/weather/releases/download/v0.3.0/weather-mcp.mcpb";

        let judge = |identifier: &str, hash: Option<&str>| {
            let mut package =
                Package::new(RegistryType::Mcpb, identifier, "0.3.0", Transport::Stdio);
            if let Some(hash) = hash {
                package = package.with_file_sha256(hash);
            }
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(package)
                .validate()
                .map_err(|err| err.to_string())
        };

        assert!(judge(ASSET, Some(DIGEST)).is_ok());

        let err = judge(ASSET, None).expect_err("an MCPB package carries its hash");
        assert!(err.contains("SHA-256"), "got: {err}");

        // Everything the registry asks of the URL that can be asked of a
        // string. The `mcp` rule is its own, and surprising enough to be worth
        // hearing about before the upload.
        for (identifier, expected) in [
            ("not-a-url", "not a URL"),
            (
                "http://github.com/example/weather/releases/download/v0.3.0/w-mcp.mcpb",
                "https://",
            ),
            (
                "https://cdn.example.com/releases/download/v0.3.0/weather-mcp.mcpb",
                "nowhere else",
            ),
            ("https://github.com/example/weather-mcp", "release asset is"),
            (
                "https://gitlab.com/group/sub/weather/-/releases/v1/files/w-mcp.mcpb",
                "release asset is",
            ),
            (
                "https://github.com/example/weather/releases/download/v0.3.0/bundle.zip",
                "say `mcp`",
            ),
        ] {
            let err = judge(identifier, Some(DIGEST)).expect_err("the registry refuses this URL");
            assert!(err.contains(expected), "`{identifier}`: {err}");
        }

        // GitLab says the same thing two ways, and a nested group is a project
        // path like any other.
        for ok in [
            "https://gitlab.com/group/sub/weather/-/releases/v1/downloads/w-mcp.mcpb",
            "https://gitlab.com/me/weather/-/package_files/123/download?mcp=1",
        ] {
            assert!(judge(ok, Some(DIGEST)).is_ok(), "`{ok}` is a release asset");
        }

        // And the spelling does not get a package out of the rule.
        let err = judge("not-a-url", Some(DIGEST)).expect_err("refused as `Mcpb`");
        let spelled = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .with_package(
                Package::new(
                    RegistryType::Other("mcpb".into()),
                    "not-a-url",
                    "0.3.0",
                    Transport::Stdio,
                )
                .with_file_sha256(DIGEST),
            )
            .validate()
            .expect_err("refused as `Other(\"mcpb\")` too");
        assert_eq!(err, spelled.to_string());
    }

    /// The official registry defaults a Cargo package's registry to crates.io
    /// and refuses any other, and refuses the field outright on the two types
    /// whose identifier carries a host. Three rules, one per named type.
    #[test]
    fn a_registry_base_url_is_checked_against_the_type_that_carries_it() {
        let with_base = |package: Package, url: &str| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(package.with_registry_base_url(url))
                .validate()
        };

        assert!(
            with_base(Package::cargo("weather-mcp", "0.3.0"), CRATES_IO).is_ok(),
            "crates.io is the one Cargo registry accepted"
        );
        let err = with_base(
            Package::cargo("weather-mcp", "0.3.0"),
            "https://crates.example.com",
        )
        .expect_err("a private Cargo mirror is not accepted");
        assert!(err.to_string().contains("crates.io"), "got: {err}");

        let oci = Package::new(
            RegistryType::Oci,
            "docker.io/example/weather:1.0.0",
            "1.0.0",
            Transport::Stdio,
        );
        let err = with_base(oci, "https://docker.io").expect_err("OCI refuses the field");
        assert!(err.to_string().contains("must not carry"), "got: {err}");

        // A registry this SDK does not name brings its own base URL, and what
        // the schema asks of it is that it be a URI.
        let other = || {
            Package::new(
                RegistryType::Other("npm".into()),
                "@me/w",
                "1.0.0",
                Transport::Stdio,
            )
        };
        assert!(with_base(other(), "https://registry.npmjs.org").is_ok());
        for bad in ["", "registry.npmjs.org", "not a url"] {
            let err = with_base(other(), bad).expect_err("a base URL is a URL");
            assert!(
                err.to_string().contains("registryBaseUrl"),
                "`{bad}`: {err}"
            );
        }
    }

    /// `format: uri` in this schema is an assertion, not an annotation --
    /// `$schema` names draft-07, where a validator checks it. So every field
    /// typed that way is held to being a URL, and a `homepage` written as a
    /// bare host in `Cargo.toml` is the one that arrives that way.
    #[test]
    fn a_uri_typed_field_has_to_be_a_url() {
        let website = |url: &str| {
            manifest()
                .with_website_url(url)
                .validate()
                .map_err(|err| err.to_string())
        };

        assert!(website("https://example.com/weather").is_ok());
        for bad in ["", "example.com", "www.example.com/weather"] {
            let err = website(bad).expect_err("a website URL is a URL");
            assert!(err.contains("websiteUrl"), "`{bad}`: {err}");
        }

        // The field is checked wherever it came from, and a `homepage` written
        // without a scheme is how it most often arrives: `Cargo.toml` takes
        // that string as readily as a URL.
        let err = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .with_cargo(CargoEnv::new("weather-mcp", "0.3.0").with_homepage("example.com"))
            .validate()
            .expect_err("a homepage copied from Cargo is a websiteUrl like any other");
        assert!(err.to_string().contains("websiteUrl"), "got: {err}");

        let err = manifest()
            .with_schema_url("2025-12-11/server.schema.json")
            .validate()
            .expect_err("the schema URL is a URL too");
        assert!(err.to_string().contains("$schema"), "got: {err}");
    }

    /// Carrying a hash is half of what the schema asks; `^[a-f0-9]{64}$` is
    /// the other half. An empty string and a typo both satisfy "present".
    #[test]
    fn a_malformed_mcpb_hash_is_refused() {
        let mcpb = |hash: &str| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(
                    Package::new(
                        RegistryType::Mcpb,
                        "https://github.com/example/weather/releases/download/v0.3.0/w.mcpb",
                        "0.3.0",
                        Transport::Stdio,
                    )
                    .with_file_sha256(hash),
                )
                .validate()
        };
        const DIGEST: &str = "fe333e598595000ae021bd27117db32ec69af6987f507ba7a63c90638ff633ce";

        assert!(mcpb(DIGEST).is_ok(), "a real digest passes");
        for bad in [
            "",
            "not-a-hash",
            // Upper case is what `shasum` prints on some platforms, and the
            // schema's character class does not admit it.
            &DIGEST.to_uppercase(),
            // One character short, which is the failure a truncated paste has.
            &DIGEST[..63],
            &format!("{DIGEST}0"),
        ] {
            let err = mcpb(bad).expect_err("`{bad}` is not a SHA-256 digest");
            assert!(err.to_string().contains("64 lowercase hex"), "got: {err}");
        }
    }

    /// A value that is not a digest can be arbitrarily long, and quoting it
    /// back in full would make the error unreadable.
    #[test]
    fn a_rejected_value_is_quoted_back_in_brief() {
        let err = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .with_package(Package::cargo("weather-mcp", "0.3.0").with_file_sha256("x".repeat(4096)))
            .validate()
            .expect_err("4096 `x`s are not a digest");

        assert!(
            err.to_string().len() < 200,
            "got {} chars",
            err.to_string().len()
        );
        assert!(err.to_string().contains("..."), "got: {err}");
    }

    /// The field is skipped by serde, so a manifest read back from JSON has no
    /// app behind it -- the same position as one built by hand, and not the
    /// one that gets refused.
    #[test]
    fn a_manifest_read_back_from_json_still_validates() {
        let json = manifest().to_json().expect("a complete manifest");

        let read: ServerManifest = serde_json::from_str(&json).expect("reads back");

        assert_eq!(read.transport(), Some(&Transport::Stdio));
        assert!(read.validate().is_ok(), "a published manifest stays valid");
    }

    /// Two spellings of one wire value, and the rules follow the wire. Nothing
    /// canonicalizes a hand-written `Other("mcpb")` -- `From<&str>` and
    /// deserialization both return the variant -- so validation reads the
    /// string rather than the shape that carries it.
    #[test]
    fn a_reserved_spelling_under_other_is_held_to_the_same_rules() {
        let judge = |package: Package| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(package)
                .validate()
        };
        let written_out = |name: &str| RegistryType::Other(name.to_owned());

        // The MCPB hash, which the variant is refused for.
        let err = judge(Package::new(
            written_out("mcpb"),
            "https://github.com/example/weather/releases/download/v0.3.0/w.mcpb",
            "0.3.0",
            Transport::Stdio,
        ))
        .expect_err("`mcpb` spelled by hand is still an MCPB package");
        assert!(err.to_string().contains("SHA-256"), "got: {err}");

        // ...and both `registryBaseUrl` rules.
        let err = judge(
            Package::new(
                written_out("cargo"),
                "weather-mcp",
                "0.3.0",
                Transport::Stdio,
            )
            .with_registry_base_url("https://crates.example.com"),
        )
        .expect_err("`cargo` spelled by hand keeps the crates.io rule");
        assert!(err.to_string().contains("crates.io"), "got: {err}");

        let err = judge(
            Package::new(
                written_out("oci"),
                "docker.io/example/weather:1.0.0",
                "1.0.0",
                Transport::Stdio,
            )
            .with_registry_base_url("https://docker.io"),
        )
        .expect_err("`oci` spelled by hand still refuses the field");
        assert!(err.to_string().contains("must not carry"), "got: {err}");

        // A name the registry does not reserve carries no rules of ours.
        assert!(
            judge(
                Package::new(
                    written_out("npm"),
                    "@example/weather",
                    "1.0.0",
                    Transport::Stdio
                )
                .with_registry_base_url("https://registry.npmjs.org")
            )
            .is_ok()
        );
    }

    /// An icon in a listing is one a client fetches over the network. An MCP
    /// `Icon` may instead inline the image in a `data:` URI, which is a URI and
    /// is not that -- nor would it fit the 255 characters the schema allows.
    #[test]
    fn an_icon_source_is_a_url_the_registry_can_fetch() {
        let icons = |src: &str| {
            manifest()
                .with_icons([crate::types::Icon::new(src)])
                .validate()
                .map_err(|err| err.to_string())
        };

        assert!(icons("https://example.com/icon.png").is_ok());

        for bad in [
            "",
            "not a URI",
            "icon.png",
            "https://[",
            "https://exa mple.com/i.png",
            // A URI, and not one a registry fetches.
            "data:image/png;base64,iVBORw0KGgo=",
        ] {
            let err = icons(bad).expect_err("an icon source is a URL");
            assert!(err.contains("icon source"), "`{bad}`: {err}");
        }

        let err = icons(&format!(
            "https://example.com/{}.png",
            "a".repeat(MAX_ICON_SRC)
        ))
        .expect_err("255 characters is the limit");
        assert!(err.contains("255"), "got: {err}");
    }

    /// Two rules, because the schema uses two: a transport URL is a `pattern`
    /// and may hold a `{variable}` a remote fills in, while every other URL is
    /// `format: uri` and may not.
    #[test]
    fn a_uri_field_is_stricter_than_a_transport_url() {
        // What a prefix check cannot see, and each of these was a bug in one:
        // an unterminated authority, a scheme in capitals, a request target
        // that names no host.
        assert!(is_transport_url("http://["));
        assert!(!is_http_uri("http://["));
        assert!(is_http_uri("HTTPS://EXAMPLE.COM/x"));
        assert!(!is_http_uri("example.com"));
        assert!(!is_http_uri("/just/a/path"));
        assert!(!is_http_uri("://example.com"));

        // An IPv6 literal is bracketed, and that is not the same as broken.
        assert!(is_http_uri("http://[::1]:3000/mcp"));

        // ...and the braces a remote interpolates go the other way round.
        assert!(is_transport_url("https://{tenant}.example.com/mcp"));
        assert!(!is_http_uri("https://{tenant}.example.com/mcp"));
    }

    /// A title is optional, and an empty one is not the way to leave it out:
    /// the schema gives it a `minLength`, and the registry refuses one that is
    /// only whitespace.
    #[test]
    fn a_blank_title_is_refused_rather_than_published() {
        for blank in ["", "   ", "\t"] {
            let err = manifest()
                .with_title(blank)
                .validate()
                .expect_err("a blank title is not a title");
            assert!(err.to_string().contains("blank"), "{blank:?}: {err}");
        }

        // Leaving it unset is how a manifest has no title.
        assert!(manifest().validate().is_ok());
        assert!(manifest().with_title("Weather").validate().is_ok());
    }

    /// Two fields the registry refuses over plain HTTP, in its own code and
    /// for the reason it gives -- and one it does not, because that is where a
    /// server under development answers.
    #[test]
    fn https_is_required_where_the_registry_requires_it() {
        let http = "http://example.com/x.png";

        let err = manifest()
            .with_icons([crate::types::Icon::new(http)])
            .validate()
            .expect_err("an icon is fetched over https");
        assert!(err.to_string().contains("https://"), "got: {err}");

        let err = manifest()
            .with_website_url("http://example.com")
            .validate()
            .expect_err("a website is linked over https");
        assert!(err.to_string().contains("https://"), "got: {err}");

        // A transport keeps http: `App::server_manifest` derives exactly this
        // from a local HTTP server.
        assert!(
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(
                    Package::cargo("weather-mcp", "0.3.0")
                        .with_transport(Transport::streamable_http("http://127.0.0.1:3000/mcp"))
                )
                .validate()
                .is_ok()
        );
    }

    /// A package identifier names something in a registry, and the registry
    /// refuses one with a space in it whatever the type.
    #[test]
    fn a_package_identifier_carries_no_spaces() {
        let err = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .with_package(Package::cargo("weather mcp", "0.3.0"))
            .validate()
            .expect_err("an identifier with a space is refused");

        assert!(err.to_string().contains("no spaces"), "got: {err}");
    }

    /// A remote is a server already running somewhere. Stdio names a process
    /// the client would spawn, which is the other kind of entry entirely.
    #[test]
    fn a_remote_over_stdio_is_refused() {
        let err = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .with_remote(Remote::new(Transport::Stdio))
            .validate()
            .expect_err("a remote needs a URL");

        assert!(err.to_string().contains("streamable_http"), "got: {err}");
    }

    /// `with_cargo` fills what is missing and leaves what is not, so the call
    /// order is how an author says something different from what Cargo says.
    #[test]
    fn with_cargo_fills_only_what_is_missing() {
        let cargo = CargoEnv::new("weather-mcp", "0.3.0")
            .with_description("A description from Cargo.toml")
            .with_repository("https://github.com/RomanEmreis/neva")
            .with_homepage("https://example.com/weather");

        let derived =
            ServerManifest::new("io.github.romanemreis/weather", "").with_cargo(cargo.clone());
        assert_eq!(derived.description, "A description from Cargo.toml");
        assert_eq!(derived.version, "0.3.0");
        assert_eq!(
            derived.website_url.as_deref(),
            Some("https://example.com/weather")
        );
        assert_eq!(
            derived.repository,
            Some(Repository::new("https://github.com/RomanEmreis/neva"))
        );
        assert_eq!(derived.packages.len(), 1);
        assert_eq!(derived.packages[0].identifier(), "weather-mcp");
        assert_eq!(derived.packages[0].version(), "0.3.0");

        let overridden = ServerManifest::new("io.github.romanemreis/weather", "1.0.0")
            .with_description("Said here, not in Cargo.toml")
            .with_cargo(cargo);
        assert_eq!(overridden.description, "Said here, not in Cargo.toml");
        assert_eq!(overridden.version, "1.0.0");
    }

    /// An empty `CARGO_PKG_*` is Cargo saying the field is not set, not a field
    /// set to the empty string.
    #[test]
    fn empty_cargo_fields_are_absent_rather_than_empty() {
        let cargo = CargoEnv::new("weather-mcp", "0.3.0")
            .with_description("")
            .with_repository("")
            .with_homepage("");

        let manifest = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .with_cargo(cargo);

        assert_eq!(manifest.description, "Weather");
        assert!(manifest.repository.is_none());
        assert!(manifest.website_url.is_none());
    }

    /// `source` is how a registry decides which API to verify a repository
    /// with, and the URL is what says which forge that is -- for the two whose
    /// URLs anyone but their own registry can read.
    #[test]
    fn a_repository_url_names_its_forge_or_leaves_it_to_the_author() {
        assert_eq!(
            Repository::new("https://github.com/RomanEmreis/neva").source,
            "github"
        );
        assert_eq!(
            Repository::new("http://www.gitlab.com/me/server").source,
            "gitlab"
        );

        // A host this cannot read names no forge, and does not get one made up.
        for url in [
            "https://git.example.com/me/server",
            "https://github.com.evil.example/me/server",
            "git@github.com:me/server.git",
        ] {
            assert!(
                Repository::new(url).source.is_empty(),
                "`{url}` names no forge this SDK can read"
            );
        }

        assert_eq!(
            Repository::new("https://codeberg.org/me/weather")
                .with_source("codeberg")
                .source,
            "codeberg"
        );
    }

    /// A crate whose repository is somewhere this cannot name is published
    /// without one rather than with a guess: the source is what a registry
    /// verifies against.
    #[test]
    fn with_cargo_attaches_a_repository_only_when_it_can_name_the_forge() {
        let derived = |url: &str| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_cargo(CargoEnv::new("weather-mcp", "0.3.0").with_repository(url))
                .repository
        };

        assert_eq!(
            derived("https://github.com/RomanEmreis/neva"),
            Some(Repository::new("https://github.com/RomanEmreis/neva"))
        );
        assert!(derived("https://git.example.com/me/server").is_none());
    }

    /// The registry reads a repository URL as `<forge>/<owner>/<repo>` and
    /// answers "invalid repository URL" for anything deeper -- including the
    /// `/tree/<branch>/<path>` form a monorepo crate often puts in
    /// `Cargo.toml`, whose registry spelling is `subfolder`.
    #[test]
    fn a_repository_url_must_name_the_repository_and_stop() {
        let judge = |repository: Repository| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(Package::cargo("weather-mcp", "0.3.0"))
                .with_repository(repository)
                .validate()
        };

        for url in [
            "https://github.com/RomanEmreis/neva",
            "https://github.com/RomanEmreis/neva/",
            "http://www.github.com/RomanEmreis/neva",
            "https://github.com/RomanEmreis/neva.rs",
            "https://gitlab.com/me/server",
        ] {
            assert!(
                judge(Repository::new(url)).is_ok(),
                "`{url}` names a repository"
            );
        }

        let err = judge(Repository::new(
            "https://github.com/RomanEmreis/neva/tree/main/neva",
        ))
        .expect_err("a path inside the repository is not the repository");
        assert!(err.to_string().contains("with_subfolder"), "got: {err}");

        for url in ["https://github.com/RomanEmreis", "https://github.com/"] {
            assert!(
                judge(Repository::new(url).with_source("github")).is_err(),
                "`{url}` does not name a repository"
            );
        }

        // The forge has to be the one the source names, however it got there.
        assert!(
            judge(Repository::new("https://github.com/RomanEmreis/neva").with_source("gitlab"))
                .is_err(),
            "a github.com URL is not a gitlab repository"
        );
    }

    /// A forge this SDK does not name is a forge all the same: `server.json`
    /// types the source as a string, and only the official registry narrows it
    /// to two. What cannot be checked for one is its URL shape -- that belongs
    /// to whoever verifies it -- so the URL is checked as a URL and no further.
    #[test]
    fn a_forge_this_sdk_does_not_name_is_still_a_forge() {
        let judge = |repository: Repository| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(Package::cargo("weather-mcp", "0.3.0"))
                .with_repository(repository)
                .validate()
        };

        // Codeberg, a Gitea, a company's own host behind a deeper path: all
        // spellings the format allows and this cannot second-guess.
        for (url, source) in [
            ("https://codeberg.org/me/weather", "codeberg"),
            ("https://git.acme.corp/teams/platform/weather", "gitea"),
            ("https://git.acme.corp:8443/weather", "acme"),
        ] {
            assert!(
                judge(Repository::new(url).with_source(source)).is_ok(),
                "`{source}` is a forge"
            );
        }

        // What is checked for an unnamed forge is that the URL is one.
        for url in ["", "git.acme.corp/me/weather", "ssh://git.acme.corp/me"] {
            assert!(
                judge(Repository::new(url).with_source("gitea")).is_err(),
                "`{url}` is not a URL"
            );
        }

        // A URL on no forge this SDK reads, left unnamed, has no source at all.
        let err = judge(Repository::new("https://git.acme.corp/me/weather"))
            .expect_err("a repository needs a source");
        assert!(err.to_string().contains("with_source"), "got: {err}");
    }

    /// A `streamable-http` transport is a URL a client dials, and the two
    /// entries that carry one are dialled from different places: a package's
    /// names where the thing just installed answers, a remote's names a server
    /// somebody else runs.
    #[test]
    fn a_transport_url_must_be_one_a_client_could_call() {
        let package = |url: &str| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(
                    Package::cargo("weather-mcp", "0.3.0")
                        .with_transport(Transport::streamable_http(url)),
                )
                .validate()
                .map_err(|err| err.to_string())
        };
        let remote = |url: &str| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_remote(Remote::new(Transport::streamable_http(url)))
                .validate()
                .map_err(|err| err.to_string())
        };

        for url in [
            "",
            "localhost:3000/mcp",
            "ftp://example.com/mcp",
            "https://",
        ] {
            assert!(package(url).is_err(), "`{url}` is not dialable");
            assert!(remote(url).is_err(), "`{url}` is not dialable");
        }

        // A package's transport is the server the user just installed, so it
        // is routinely on their own machine, and routinely not HTTPS.
        assert!(package("http://127.0.0.1:3000/mcp").is_ok());
        assert!(package("https://mcp.example.com/mcp").is_ok());

        // A remote is neither: the registry refuses both.
        let err = remote("http://mcp.example.com/mcp").expect_err("a remote is reached over TLS");
        assert!(err.contains("https://"), "got: {err}");
        let err = remote("https://localhost:3000/mcp").expect_err("a remote is not this machine");
        assert!(err.contains("on this machine"), "got: {err}");
        assert!(remote("https://mcp.example.com/mcp").is_ok());
    }

    /// A `{name}` in a URL is filled in by something the entry around it
    /// declares. Without one the client is handed a URL with a hole in it.
    #[test]
    fn a_url_template_needs_something_to_fill_it() {
        let remote = |transport| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_remote(transport)
                .validate()
                .map_err(|err| err.to_string())
        };
        let templated = || Transport::streamable_http("https://{tenant}.example.com/mcp");

        let err = remote(Remote::new(templated())).expect_err("`tenant` is filled by nothing");
        assert!(err.contains("tenant"), "got: {err}");

        assert!(
            remote(Remote::new(templated()).with_variable("tenant", Input::new().required()))
                .is_ok()
        );

        // A package fills one from what it asks the user for: an environment
        // variable, a flag by name, or a positional by its value hint.
        let package = |package: Package| {
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(package)
                .validate()
                .map_err(|err| err.to_string())
        };
        let with_url = |url: &str| {
            Package::cargo("weather-mcp", "0.3.0").with_transport(Transport::streamable_http(url))
        };

        let err = package(with_url("http://localhost:{port}/mcp"))
            .expect_err("`port` is filled by nothing");
        assert!(err.contains("port"), "got: {err}");

        assert!(
            package(
                with_url("http://localhost:{port}/mcp")
                    .with_environment_variable(KeyValueInput::new("port"))
            )
            .is_ok()
        );
        assert!(
            package(
                with_url("http://localhost:{--port}/mcp")
                    .with_package_argument(Argument::named("--port"))
            )
            .is_ok()
        );
        assert!(
            package(
                with_url("http://localhost:{port}/mcp")
                    .with_package_argument(Argument::positional("port"))
            )
            .is_ok()
        );
    }

    /// Everything the manifest carries survives a round trip, including the
    /// `$schema` and `_meta` keys that are not Rust identifiers.
    #[test]
    fn the_manifest_reads_back() {
        let manifest = manifest()
            .with_title("Weather")
            .with_website_url("https://example.com")
            .with_repository(Repository::new("https://github.com/RomanEmreis/neva"))
            .with_publisher_metadata(serde_json::json!({ "tool": "neva" }));

        let json = manifest.to_json().expect("a complete manifest");
        let read: ServerManifest = serde_json::from_str(&json).expect("reads back");

        assert_eq!(read, manifest);
        assert!(json.contains("\"$schema\""));
        assert!(json.contains("\"io.modelcontextprotocol.registry/publisher-provided\""));
    }

    impl ServerManifest {
        /// Sets the name, which the public builder deliberately does not: it is
        /// a constructor argument, so that a manifest never exists without one.
        fn with_name_for_test(mut self, name: impl Into<String>) -> Self {
            self.name = name.into();
            self
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod app_tests {
    use super::*;
    use crate::App;

    /// What the app knows: the version it reports and the transport it is
    /// configured with, which is the package's transport.
    #[test]
    fn the_app_supplies_its_version_and_transport() {
        let app = App::new().with_options(|opt| opt.with_stdio().with_version("0.3.0"));

        let manifest = app.server_manifest("io.github.romanemreis/weather");

        assert_eq!(manifest.version(), "0.3.0");
        assert_eq!(manifest.transport(), Some(&Transport::Stdio));
    }

    /// An app that never set a version reports neva's own, which is nobody's
    /// server version: it is left for the crate to fill, and refused if
    /// nothing does.
    #[test]
    fn the_sdk_version_is_not_published_as_the_servers() {
        let app = App::new().with_options(|opt| opt.with_stdio());

        let manifest = app
            .server_manifest("io.github.romanemreis/weather")
            .with_description("Weather")
            .with_package(Package::cargo("weather-mcp", "0.3.0"));

        assert_eq!(manifest.version(), "");
        let err = manifest.validate().expect_err("no version was ever set");
        assert!(err.to_string().contains("with_version"), "got: {err}");

        // The crate's own version is what fills it.
        let filled = app
            .server_manifest("io.github.romanemreis/weather")
            .with_description("Weather")
            .with_cargo(CargoEnv::new("weather-mcp", "0.3.0"));
        assert_eq!(filled.version(), "0.3.0");
        assert!(filled.validate().is_ok());
    }

    /// An HTTP server's package says where it answers, so a client knows to
    /// call it rather than to spawn it.
    #[cfg(feature = "http-server-volga")]
    #[test]
    fn an_http_app_supplies_its_url() {
        let app = App::new().with_options(|opt| {
            opt.with_http(|http| http.bind("127.0.0.1:3000"))
                .with_version("0.3.0")
        });

        let manifest = app
            .server_manifest("io.github.romanemreis/weather")
            .with_description("Weather")
            .with_cargo(CargoEnv::new("weather-mcp", "0.3.0"));

        assert_eq!(
            manifest.packages[0].transport(),
            &Transport::streamable_http("http://127.0.0.1:3000/mcp")
        );
    }

    /// A version the author chose is theirs, even when it is the string neva
    /// happens to be at. Comparing versions could not tell the two apart; the
    /// options record the call instead.
    #[test]
    fn a_version_set_to_the_sdks_own_is_still_the_authors() {
        let app =
            App::new().with_options(|opt| opt.with_stdio().with_version(env!("CARGO_PKG_VERSION")));

        let manifest = app
            .server_manifest("io.github.romanemreis/weather")
            .with_description("Weather")
            // A crate at a different version from the one the server reports:
            // the explicit answer wins, and the package keeps the crate's.
            .with_cargo(CargoEnv::new("weather-mcp", "0.1.0"));

        assert_eq!(manifest.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest.packages[0].version(), "0.1.0");
    }

    /// An app with no transport is not a stdio app: it cannot start at all, so
    /// a package entry derived from it would describe an install that cannot
    /// work.
    #[test]
    fn an_app_without_a_transport_does_not_publish_a_stdio_package() {
        let app = App::new().with_options(|opt| opt.with_version("0.3.0"));

        let manifest = app
            .server_manifest("io.github.romanemreis/weather")
            .with_description("Weather")
            .with_cargo(CargoEnv::new("weather-mcp", "0.3.0"));

        assert_eq!(manifest.transport(), None);
        let err = manifest
            .validate()
            .expect_err("an app with no transport has no package to publish");
        assert!(err.to_string().contains("with_stdio"), "got: {err}");

        // A remote is the author's own answer, not a derived one, so it stands.
        assert!(
            app.server_manifest("io.github.romanemreis/weather")
                .with_description("Weather")
                .with_remote(Remote::new(Transport::streamable_http(
                    "https://mcp.example.com/mcp"
                )))
                .validate()
                .is_ok()
        );
    }

    /// A bind address is not a destination. `0.0.0.0` and `[::]` mean "every
    /// interface" to a listener and nothing at all to a client, so what the
    /// package entry names is the loopback the user's own client will dial.
    #[cfg(feature = "http-server-volga")]
    #[test]
    fn a_wildcard_bind_becomes_an_address_a_client_can_dial() {
        let manifest = |bind: &str| {
            App::new()
                .with_options(|opt| opt.with_http(|http| http.bind(bind)).with_version("0.3.0"))
                .server_manifest("io.github.romanemreis/weather")
                .with_description("Weather")
                .with_cargo(CargoEnv::new("weather-mcp", "0.3.0"))
        };

        assert_eq!(
            manifest("0.0.0.0:3000").packages[0].transport(),
            &Transport::streamable_http("http://127.0.0.1:3000/mcp")
        );
        assert_eq!(
            manifest("[::]:3000").packages[0].transport(),
            &Transport::streamable_http("http://[::1]:3000/mcp")
        );
        // An address that is already one stays as it is.
        assert_eq!(
            manifest("192.168.1.10:3000").packages[0].transport(),
            &Transport::streamable_http("http://192.168.1.10:3000/mcp")
        );
    }

    /// A port of `0` is the one bind address with no translation: the OS picks
    /// the real one when the listener starts, long after this is written.
    #[cfg(feature = "http-server-volga")]
    #[test]
    fn an_ephemeral_port_cannot_be_published() {
        let manifest = App::new()
            .with_options(|opt| {
                opt.with_http(|http| http.bind("127.0.0.1:0"))
                    .with_version("0.3.0")
            })
            .server_manifest("io.github.romanemreis/weather")
            .with_description("Weather")
            .with_cargo(CargoEnv::new("weather-mcp", "0.3.0"));

        let err = manifest.validate().expect_err("port 0 is not a port");
        assert!(err.to_string().contains("port 0"), "got: {err}");
        assert!(err.to_string().contains("with_transport"), "got: {err}");
    }

    /// The macro is the two calls above in one, for the common case.
    #[test]
    fn the_macro_is_the_app_and_cargo_together() {
        let app = App::new().with_options(|opt| opt.with_stdio().with_version("0.3.0"));

        let manifest = crate::server_manifest!(app, "io.github.romanemreis/weather");

        // This crate's own Cargo metadata, since that is where it expanded.
        assert_eq!(manifest.packages[0].identifier(), env!("CARGO_PKG_NAME"));
        assert_eq!(manifest.name(), "io.github.romanemreis/weather");
    }
}
