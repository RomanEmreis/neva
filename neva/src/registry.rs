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

/// Where the server's source is, so users and reviewers can read it.
///
/// # Examples
/// ```rust
/// use neva::registry::Repository;
///
/// let repo = Repository::new("https://github.com/RomanEmreis/neva", "github")
///     .with_subfolder("examples/registry");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// Where the source can be browsed and cloned.
    url: String,

    /// Which forge that is: `github`, `gitlab`. Registries pick their
    /// verification method from it.
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
    /// A repository at `url`, hosted on `source`.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Repository;
    ///
    /// let repo = Repository::new("https://github.com/RomanEmreis/neva", "github");
    /// ```
    pub fn new(url: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            source: source.into(),
            subfolder: None,
            id: None,
        }
    }

    /// The repository a URL points at, with the forge read off the host.
    ///
    /// Returns `None` for a URL that names no forge this knows, because
    /// `source` is what tells a registry how to verify the repository and
    /// guessing it wrong is worse than leaving it out.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Repository;
    ///
    /// let repo = Repository::from_url("https://github.com/RomanEmreis/neva")
    ///     .expect("github is a forge this knows");
    ///
    /// assert!(Repository::from_url("https://git.example.com/me/server").is_none());
    /// ```
    pub fn from_url(url: impl AsRef<str>) -> Option<Self> {
        let url = url.as_ref();
        let source = [("github.com", "github"), ("gitlab.com", "gitlab")]
            .into_iter()
            .find(|(host, _)| {
                url.starts_with(&format!("https://{host}/"))
                    || url.starts_with(&format!("http://{host}/"))
            })
            .map(|(_, source)| source)?;
        Some(Self::new(url, source))
    }

    /// Points at the server's own directory inside the repository.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Repository;
    ///
    /// let repo = Repository::new("https://github.com/RomanEmreis/neva", "github")
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
    /// let repo = Repository::new("https://github.com/RomanEmreis/neva", "github")
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
        if self.repository.is_none()
            && let Some(repository) = cargo.repository.as_deref().and_then(Repository::from_url)
        {
            self.repository = Some(repository);
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
    ///     .with_repository(Repository::new("https://github.com/RomanEmreis/neva", "github"));
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

        if let Some(title) = &self.title
            && title.chars().count() > MAX_TITLE
        {
            return Err(invalid(format!(
                "`title` is longer than the {MAX_TITLE} characters the registry allows"
            )));
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
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.identifier.is_empty() {
            return Err(invalid("a package needs an `identifier`"));
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

        if matches!(self.registry_type, RegistryType::Mcpb) && self.file_sha256.is_none() {
            return Err(invalid(
                "an MCPB package must carry the file's SHA-256: set it with `with_file_sha256`",
            ));
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

        // Each named type has its own answer about `registryBaseUrl`, and two
        // of them are "not at all": an OCI or MCPB identifier carries its host
        // already, and the official registry refuses the field rather than
        // ignoring it.
        match (&self.registry_type, self.registry_base_url.as_deref()) {
            (RegistryType::Cargo, Some(url)) if url != CRATES_IO => Err(invalid(format!(
                "a Cargo package comes from `{CRATES_IO}` or from nowhere the registry accepts; \
                 `{url}` is refused. Leaving it unset is the same thing and says less"
            ))),
            (RegistryType::Oci | RegistryType::Mcpb, Some(_)) => Err(invalid(format!(
                "an `{}` package must not carry a `registryBaseUrl`: its identifier names the \
                 host already",
                self.registry_type.as_str()
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

    if version.starts_with(['^', '~', '>', '<', '='])
        || version.contains('*')
        || version.ends_with(".x")
        || version.ends_with(".X")
    {
        return Err(invalid(format!(
            "{what} must name one version, not a range: `{version}`"
        )));
    }

    Ok(())
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

    /// `CARGO_PKG_VERSION` is a version; a dependency requirement is not.
    #[test]
    fn a_version_range_is_refused() {
        for range in ["^1.2.3", "~1.2.3", ">=1.2.3", "1.*", "1.x"] {
            let err = manifest()
                .with_version(range)
                .validate()
                .expect_err("a range is not a version");
            assert!(err.to_string().contains("not a range"), "{range}: {err}");
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
    /// registry, so its hash is what says the file is the published one.
    #[test]
    fn an_mcpb_package_without_a_hash_is_refused() {
        let package = Package::new(
            RegistryType::Mcpb,
            "https://github.com/example/weather/releases/download/v0.3.0/weather.mcpb",
            "0.3.0",
            Transport::Stdio,
        );

        let err = ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
            .with_description("Weather")
            .with_package(package.clone())
            .validate()
            .expect_err("an MCPB package carries its hash");
        assert!(err.to_string().contains("SHA-256"), "got: {err}");

        assert!(
            ServerManifest::new("io.github.romanemreis/weather", "0.3.0")
                .with_description("Weather")
                .with_package(package.with_file_sha256(
                    "fe333e598595000ae021bd27117db32ec69af6987f507ba7a63c90638ff633ce"
                ))
                .validate()
                .is_ok()
        );
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
            Some(Repository::new(
                "https://github.com/RomanEmreis/neva",
                "github"
            ))
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

    /// `source` is how a registry decides which API to verify the repository
    /// with, so a host this does not know is left for the author to name.
    #[test]
    fn a_repository_url_names_its_forge_or_none() {
        assert_eq!(
            Repository::from_url("https://github.com/RomanEmreis/neva"),
            Some(Repository::new(
                "https://github.com/RomanEmreis/neva",
                "github"
            ))
        );
        assert_eq!(
            Repository::from_url("https://gitlab.com/me/server").map(|r| r.source),
            Some("gitlab".to_owned())
        );
        assert!(Repository::from_url("https://git.example.com/me/server").is_none());
        assert!(Repository::from_url("https://github.com.evil.example/me").is_none());
    }

    /// Everything the manifest carries survives a round trip, including the
    /// `$schema` and `_meta` keys that are not Rust identifiers.
    #[test]
    fn the_manifest_reads_back() {
        let manifest = manifest()
            .with_title("Weather")
            .with_website_url("https://example.com")
            .with_repository(Repository::new(
                "https://github.com/RomanEmreis/neva",
                "github",
            ))
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
