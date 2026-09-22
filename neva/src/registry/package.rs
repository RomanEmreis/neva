//! How a client gets the server and how it talks to it once it has it: the
//! `packages` entries (something to install) and the `remotes` entries
//! (something already running).

use super::input::{Argument, Input, KeyValueInput};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Where a package is downloaded from, which also says how to run it.
///
/// Three of the registry's types are named here, because those are the three
/// ways a Rust binary reaches a user: source through crates.io, an image, or a
/// prebuilt archive. The registry's other types -- npm, PyPI, NuGet -- assume a
/// runtime this server does not have, and reaching them would mean wrapping the
/// binary in somebody else's package format. The field is an open set of
/// strings, so [`Other`](Self::Other) says one anyway.
///
/// # Examples
/// ```rust
/// use neva::registry::RegistryType;
///
/// assert_eq!(RegistryType::Cargo.as_str(), "cargo");
/// assert_eq!(RegistryType::Other("npm".into()).as_str(), "npm");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryType {
    /// crates.io. `cargo install <crate>` puts the binary on `PATH`, and
    /// clients run it by name -- there is no `npx`-style per-run fetcher, so a
    /// Cargo package needs no `runtimeHint`.
    Cargo,
    /// An OCI image: Docker Hub, GHCR, Quay, and the cloud registries. The
    /// image reference is the whole identifier.
    Oci,
    /// An MCP bundle -- a prebuilt archive attached to a GitHub or GitLab
    /// release, for users who should not need a toolchain. Carries the
    /// download URL as its identifier and a
    /// [SHA-256](Package::with_file_sha256) of what is behind it.
    Mcpb,
    /// A registry type this SDK does not name.
    ///
    /// What reaches the registry is the string, so a spelling one of the
    /// variants above also has is that type: `Other("mcpb".into())` is an
    /// `mcpb` package, held to the same rules by
    /// [`ServerManifest::validate`](super::ServerManifest::validate).
    /// [`from`](Self::from) and deserialization return the variant instead, so
    /// this is only reachable by writing it out.
    Other(String),
}

impl RegistryType {
    /// What the manifest writes for this registry.
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Cargo => "cargo",
            Self::Oci => "oci",
            Self::Mcpb => "mcpb",
            Self::Other(other) => other,
        }
    }
}

impl From<&str> for RegistryType {
    #[inline]
    fn from(value: &str) -> Self {
        match value {
            "cargo" => Self::Cargo,
            "oci" => Self::Oci,
            "mcpb" => Self::Mcpb,
            other => Self::Other(other.to_owned()),
        }
    }
}

impl Serialize for RegistryType {
    #[inline]
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RegistryType {
    #[inline]
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?.as_str()))
    }
}

/// How a client speaks to the server once it is running.
///
/// The registry also names `sse`, the two-endpoint HTTP+SSE transport that
/// MCP 2026-07-28 replaced with Streamable HTTP. It is not here: neva serves
/// one endpoint and never that one, so no manifest this writes could honestly
/// claim it.
///
/// # Examples
/// ```rust
/// use neva::registry::Transport;
///
/// let local = Transport::Stdio;
/// let remote = Transport::streamable_http("https://mcp.example.com/mcp");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Transport {
    /// The client spawns the server and talks to it over stdin/stdout.
    #[default]
    Stdio,

    /// Streamable HTTP, at `url`.
    StreamableHttp {
        /// Where the server answers. `http://` or `https://`, or a
        /// `{variable}` a remote's `variables` fills in.
        url: String,
        /// Headers every request carries.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Vec<KeyValueInput>,
    },
}

impl Transport {
    /// Streamable HTTP at `url`, with no headers yet.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Transport;
    ///
    /// let transport = Transport::streamable_http("https://mcp.example.com/mcp");
    /// ```
    pub fn streamable_http(url: impl Into<String>) -> Self {
        Self::StreamableHttp {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    /// The URL a client would dial, for the transport that has one.
    #[inline]
    pub(super) fn url(&self) -> Option<&str> {
        match self {
            Self::Stdio => None,
            Self::StreamableHttp { url, .. } => Some(url),
        }
    }

    /// Adds a header every request to this transport carries.
    ///
    /// Ignored by [`Stdio`](Self::Stdio), which has no requests to put one on.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{KeyValueInput, Transport};
    ///
    /// let transport = Transport::streamable_http("https://mcp.example.com/mcp")
    ///     .with_header(KeyValueInput::new("X-Tenant").required());
    /// ```
    pub fn with_header(mut self, header: KeyValueInput) -> Self {
        if let Self::StreamableHttp { headers, .. } = &mut self {
            headers.push(header);
        }
        self
    }
}

/// Something a client installs and runs: which registry it comes from, which
/// version, and how to talk to it.
///
/// # Examples
/// ```rust
/// use neva::registry::{KeyValueInput, Package};
///
/// let package = Package::cargo("weather-mcp", "0.3.0")
///     .with_environment_variable(
///         KeyValueInput::new("WEATHER_API_KEY").required().secret(),
///     );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    /// Where the package is downloaded from.
    pub(super) registry_type: RegistryType,

    /// The registry's base URL, for a registry type that takes one.
    ///
    /// Rarely set: a Cargo package comes from crates.io, and an OCI or MCPB
    /// identifier carries its host already. It is here for
    /// [`Other`](RegistryType::Other), and for a registry that reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) registry_base_url: Option<String>,

    /// What the package is called in that registry -- the crate name, for Cargo.
    pub(super) identifier: String,

    /// The exact version. Not a range, and not `latest`.
    pub(super) version: String,

    /// How a client talks to the server this package installs.
    pub(super) transport: Transport,

    /// Environment variables the server is run with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) environment_variables: Vec<KeyValueInput>,

    /// Arguments passed to the server's own binary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) package_arguments: Vec<Argument>,

    /// Arguments passed to whatever runs the binary (`docker`, `npx`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) runtime_arguments: Vec<Argument>,

    /// Which runner that is. Goes with
    /// [`runtime_arguments`](Self::with_runtime_argument); a Cargo package has
    /// neither, because `cargo install` leaves a binary that is run by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) runtime_hint: Option<String>,

    /// SHA-256 of the package file. Required for [`Mcpb`](RegistryType::Mcpb),
    /// optional elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) file_sha256: Option<String>,
}

impl Package {
    /// A package of `registry_type`, at `version`, spoken to over `transport`.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, RegistryType, Transport};
    ///
    /// let package = Package::new(
    ///     RegistryType::Oci,
    ///     "docker.io/example/weather-mcp:1.0.0",
    ///     "1.0.0",
    ///     Transport::Stdio,
    /// );
    /// ```
    pub fn new(
        registry_type: RegistryType,
        identifier: impl Into<String>,
        version: impl Into<String>,
        transport: Transport,
    ) -> Self {
        Self {
            registry_type,
            registry_base_url: None,
            identifier: identifier.into(),
            version: version.into(),
            transport,
            environment_variables: Vec::new(),
            package_arguments: Vec::new(),
            runtime_arguments: Vec::new(),
            runtime_hint: None,
            file_sha256: None,
        }
    }

    /// A crate on crates.io, spoken to over stdio.
    ///
    /// Stdio because that is what `cargo install` leaves behind: a binary on
    /// `PATH` that the client spawns. Say otherwise with
    /// [`with_transport`](Self::with_transport).
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::Package;
    ///
    /// let package = Package::cargo("weather-mcp", "0.3.0");
    /// ```
    pub fn cargo(identifier: impl Into<String>, version: impl Into<String>) -> Self {
        Self::new(RegistryType::Cargo, identifier, version, Transport::Stdio)
    }

    /// Says how a client talks to the server this package installs.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, Transport};
    ///
    /// let package = Package::cargo("weather-mcp", "0.3.0")
    ///     .with_transport(Transport::streamable_http("http://localhost:3000/mcp"));
    /// ```
    pub fn with_transport(mut self, transport: Transport) -> Self {
        self.transport = transport;
        self
    }

    /// Names the registry this package is downloaded from, for a registry type
    /// that takes one -- which none of the named ones do. See the field's own
    /// note above.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, RegistryType, Transport};
    ///
    /// let package = Package::new(
    ///     RegistryType::Other("npm".into()),
    ///     "@example/weather",
    ///     "1.0.0",
    ///     Transport::Stdio,
    /// )
    /// .with_registry_base_url("https://registry.npmjs.org");
    /// ```
    pub fn with_registry_base_url(mut self, url: impl Into<String>) -> Self {
        self.registry_base_url = Some(url.into());
        self
    }

    /// Adds an environment variable the server is run with.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{KeyValueInput, Package};
    ///
    /// let package = Package::cargo("weather-mcp", "0.3.0")
    ///     .with_environment_variable(KeyValueInput::new("WEATHER_API_KEY").required().secret());
    /// ```
    pub fn with_environment_variable(mut self, variable: KeyValueInput) -> Self {
        self.environment_variables.push(variable);
        self
    }

    /// Adds an argument passed to the server's own binary.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Argument, Package};
    ///
    /// let package = Package::cargo("weather-mcp", "0.3.0")
    ///     .with_package_argument(Argument::named("--port").with_default("3000"));
    /// ```
    pub fn with_package_argument(mut self, argument: Argument) -> Self {
        self.package_arguments.push(argument);
        self
    }

    /// Adds an argument passed to the runner rather than to the server, and
    /// names the runner if it has not been named yet.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Argument, Package, RegistryType, Transport};
    ///
    /// let package = Package::new(
    ///     RegistryType::Oci,
    ///     "docker.io/example/weather-mcp:1.0.0",
    ///     "1.0.0",
    ///     Transport::Stdio,
    /// )
    /// .with_runtime_hint("docker")
    /// .with_runtime_argument(Argument::named("--network").with_value("host"));
    /// ```
    pub fn with_runtime_argument(mut self, argument: Argument) -> Self {
        self.runtime_arguments.push(argument);
        self
    }

    /// Names what runs the package -- `docker`, `npx`, `uvx`.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, RegistryType, Transport};
    ///
    /// let package = Package::new(
    ///     RegistryType::Oci,
    ///     "docker.io/example/weather-mcp:1.0.0",
    ///     "1.0.0",
    ///     Transport::Stdio,
    /// )
    /// .with_runtime_hint("docker");
    /// ```
    pub fn with_runtime_hint(mut self, hint: impl Into<String>) -> Self {
        self.runtime_hint = Some(hint.into());
        self
    }

    /// The SHA-256 of the package file, which an MCPB package must carry.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Package, RegistryType, Transport};
    ///
    /// let package = Package::new(
    ///     RegistryType::Mcpb,
    ///     "https://github.com/example/weather/releases/download/v1.0.0/weather.mcpb",
    ///     "1.0.0",
    ///     Transport::Stdio,
    /// )
    /// .with_file_sha256("fe333e598595000ae021bd27117db32ec69af6987f507ba7a63c90638ff633ce");
    /// ```
    pub fn with_file_sha256(mut self, hash: impl Into<String>) -> Self {
        self.file_sha256 = Some(hash.into());
        self
    }

    /// What the package is called in its registry.
    #[inline]
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// The version this entry names.
    #[inline]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// How a client talks to the server this package installs.
    #[inline]
    pub fn transport(&self) -> &Transport {
        &self.transport
    }
}

/// A server that is already running somewhere: no package to install, just a
/// URL to call.
///
/// # Examples
/// ```rust
/// use neva::registry::{Input, Remote, Transport};
///
/// let remote = Remote::new(Transport::streamable_http("https://{tenant}.example.com/mcp"))
///     .with_variable("tenant", Input::new().with_description("your tenant").required());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// How to reach it. Never [`Stdio`](Transport::Stdio) -- there is no
    /// process on this side to spawn, which
    /// [`validate`](super::ServerManifest::validate) checks.
    #[serde(flatten)]
    pub(super) transport: Transport,

    /// What the `{curly_braces}` in the URL stand for.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(super) variables: HashMap<String, Input>,
}

impl Remote {
    /// Whether this names a transport a remote cannot have.
    #[inline]
    pub(super) fn is_stdio(&self) -> bool {
        matches!(self.transport, Transport::Stdio)
    }

    /// A remote reached over `transport`.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Remote, Transport};
    ///
    /// let remote = Remote::new(Transport::streamable_http("https://mcp.example.com/mcp"));
    /// ```
    pub fn new(transport: Transport) -> Self {
        Self {
            transport,
            variables: HashMap::new(),
        }
    }

    /// Gives a `{name}` in the URL something to stand for.
    ///
    /// # Examples
    /// ```rust
    /// use neva::registry::{Input, Remote, Transport};
    ///
    /// let remote = Remote::new(Transport::streamable_http("https://{region}.example.com/mcp"))
    ///     .with_variable("region", Input::new().with_default("eu"));
    /// ```
    pub fn with_variable(mut self, name: impl Into<String>, variable: Input) -> Self {
        self.variables.insert(name.into(), variable);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Cargo entry the registry documents: no
    /// `registryBaseUrl`, no `runtimeHint`, stdio.
    #[test]
    fn a_cargo_package_is_the_shape_the_registry_documents() {
        let json = serde_json::to_value(Package::cargo("widget-mcp", "0.3.0"))
            .expect("a package serializes");

        assert_eq!(
            json,
            serde_json::json!({
                "registryType": "cargo",
                "identifier": "widget-mcp",
                "version": "0.3.0",
                "transport": { "type": "stdio" },
            })
        );
    }

    /// The transport is tagged by `type`, and the tag is the registry's
    /// spelling rather than Rust's.
    #[test]
    fn transports_are_tagged_the_way_the_schema_spells_them() {
        assert_eq!(
            serde_json::to_value(Transport::Stdio).expect("stdio serializes"),
            serde_json::json!({ "type": "stdio" })
        );
        assert_eq!(
            serde_json::to_value(Transport::streamable_http("https://example.com/mcp"))
                .expect("streamable http serializes"),
            serde_json::json!({ "type": "streamable-http", "url": "https://example.com/mcp" })
        );
    }

    /// A remote is its transport with the variables beside it, not nested
    /// under it.
    #[test]
    fn a_remote_flattens_its_transport() {
        let remote = Remote::new(Transport::streamable_http(
            "https://{tenant}.example.com/mcp",
        ))
        .with_variable("tenant", Input::new().required());

        let json = serde_json::to_value(&remote).expect("a remote serializes");

        assert_eq!(json["type"], "streamable-http");
        assert_eq!(json["url"], "https://{tenant}.example.com/mcp");
        assert_eq!(json["variables"]["tenant"]["isRequired"], true);
        assert_eq!(
            serde_json::from_value::<Remote>(json).expect("and reads back"),
            remote
        );
    }

    /// The registry type is an open set of strings, so one this SDK does not
    /// name survives a round trip unchanged.
    #[test]
    fn an_unknown_registry_type_round_trips() {
        let json = serde_json::to_value(RegistryType::Other("deb".into())).expect("serializes");

        assert_eq!(json, serde_json::json!("deb"));
        assert_eq!(
            serde_json::from_value::<RegistryType>(json).expect("reads back"),
            RegistryType::Other("deb".into())
        );
        // The types this SDK does not name are still readable as themselves.
        assert_eq!(
            serde_json::from_value::<RegistryType>(serde_json::json!("npm")).expect("reads back"),
            RegistryType::Other("npm".into())
        );
        assert_eq!(
            serde_json::from_value::<RegistryType>(serde_json::json!("cargo")).expect("reads back"),
            RegistryType::Cargo
        );
    }
}
