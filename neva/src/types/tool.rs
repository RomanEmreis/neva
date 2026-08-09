//! Represents an MCP tool

#[cfg(any(feature = "server", feature = "client"))]
use crate::error::{Error, ErrorCode};
use crate::shared;
use crate::types::{Cursor, Icon, PropertyType, request::RequestParamsMeta};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
#[cfg(feature = "server")]
use {
    super::helpers::TypeCategory,
    crate::shared::BoxFuture,
    crate::types::{
        ArgNames, FromHandlerArgs, FromRequest, IntoResponse, Page, Request, RequestId, Response,
    },
    crate::{
        Context,
        app::handler::{FromHandlerParams, GenericHandler, Handler, HandlerParams, RequestHandler},
    },
    std::{future::Future, sync::Arc},
};

#[cfg(all(feature = "server", feature = "legacy-spec"))]
use crate::json::JsonSchema;

#[cfg(all(feature = "server", feature = "tasks"))]
use crate::types::RelatedTaskMetadata;
#[cfg(feature = "tasks")]
use crate::types::TaskMetadata;

#[cfg(feature = "client")]
use jsonschema::validator_for;

pub use call_tool_response::CallToolResponse;

mod call_tool_response;
#[cfg(feature = "server")]
mod from_request;

/// List of commands for Tools
pub mod commands {
    /// Command name that returns a list of tools available on the server.
    pub const LIST: &str = "tools/list";

    /// Name of a notification that indicates that the list of tools has changed.
    pub const LIST_CHANGED: &str = "notifications/tools/list_changed";

    /// Command name that calls a tool on the server.
    pub const CALL: &str = "tools/call";
}

/// Represents a tool that the server is capable of calling. Part of the [`ListToolsResult`].
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Clone, Serialize, Deserialize)]
pub struct Tool {
    /// The name of the tool.
    pub name: String,

    /// Intended for UI and end-user contexts -- optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    ///
    /// If not provided, the name should be used for display (except for Tool,
    /// where `annotations.title` should be given precedence over using `name`, if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the tool.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// A JSON Schema object defining the expected parameters for the tool.
    ///
    /// > Note: Needs to a valid JSON schema object that additionally is of a type object.
    ///
    /// The concrete type is selected by the [`crate::types::ToolInputSchema`]
    /// alias: the legacy typed `ToolSchema` under `legacy-spec`, or
    /// [`crate::types::schema_2020::InputSchema`] (a Value-shaped JSON
    /// Schema 2020-12 wrapper) in the default (MCP 2026-07-28) build.
    #[serde(rename = "inputSchema")]
    pub input_schema: crate::types::ToolInputSchema,

    /// An optional JSON Schema object defining the structure of the tool's output returned in
    /// the `structuredContent` field of a [`crate::types::CallToolResponse`].
    ///
    /// > Note: Needs to a valid JSON schema object that additionally is of a type object.
    ///
    /// See [`Self::input_schema`] for a note on which concrete schema type
    /// backs this alias under each feature set.
    #[serde(rename = "outputSchema", skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<crate::types::ToolInputSchema>,

    /// Optional additional tool information.
    ///
    /// Display name precedence order is: title, annotations.title, then name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,

    /// Optional set of sized icons that the client can display in a user interface.
    ///
    /// Clients that support rendering icons **MUST** support at least the following MIME types:
    /// - `image/png` - PNG images (safe, universal compatibility)
    /// - `image/jpeg` (and `image/jpg`) - JPEG images (safe, universal compatibility)
    ///
    /// Clients that support rendering icons **SHOULD** also support:
    /// - `image/svg+xml` - SVG images (scalable but requires security precautions)
    /// - `image/webp` - WebP images (modern, efficient format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,

    /// Execution-related properties for this tool.
    #[cfg(feature = "tasks")]
    #[serde(rename = "execution", skip_serializing_if = "Option::is_none")]
    pub exec: Option<ToolExecution>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,

    /// A list of roles that are allowed to invoke the tool
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub(crate) roles: Option<Vec<String>>,

    /// A list of permissions that are allowed to invoke the tool
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub(crate) permissions: Option<Vec<String>>,

    /// A tool call handler
    #[serde(skip)]
    #[cfg(feature = "server")]
    handler: Option<RequestHandler<CallToolResponse>>,

    /// The names the handler's arguments are read from `arguments` by.
    ///
    /// Server-side only: it is the property names of [`Self::input_schema`]
    /// that a peer sees. See [`Tool::with_arg_names`].
    #[serde(skip)]
    #[cfg(feature = "server")]
    pub(crate) arg_names: ArgNames,

    /// Whether [`Self::input_schema`] came from the caller rather than from
    /// the handler's signature.
    ///
    /// [`Tool::with_arg_names`] rewrites the property names of a schema it
    /// generated itself, and must not touch one it did not write: every key
    /// there was chosen deliberately, including any that happens to look
    /// positional.
    #[serde(skip)]
    #[cfg(feature = "server")]
    custom_schema: bool,
}

/// Execution-related properties for a tool.
#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg(feature = "tasks")]
pub struct ToolExecution {
    /// Indicates whether this tool supports task-augmented execution.
    /// This allows clients to handle long-running operations through polling
    /// the task system.
    #[serde(rename = "taskSupport", skip_serializing_if = "Option::is_none")]
    pub task_support: Option<TaskSupport>,
}

/// Represents task-augmentation support options for a tool.
///
/// - `forbidden` - Tool does not support task-augmented execution (default when absent)
/// - `optional` - Tool may support task-augmented execution
/// - `required` - Tool requires task-augmented execution
///
/// Default: `forbidden`
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[cfg(feature = "tasks")]
#[serde(rename_all = "lowercase")]
pub enum TaskSupport {
    /// Tool does not support task-augmented execution.
    #[default]
    Forbidden,

    /// Tool may support task-augmented execution.
    Optional,

    /// Tool requires task-augmented execution.
    Required,
}

/// Sent from the client to request a list of tools the server has.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ListToolsRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// A response to a request to list the tools available on the server.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ListToolsResult {
    /// The server's response to a tools/list request from the client.
    pub tools: Vec<Tool>,

    /// An opaque token representing the pagination position after the last returned result.
    ///
    /// When a paginated result has more data available, the `next_cursor`
    /// field will contain `Some` token that can be used in subsequent requests
    /// to fetch the next page. When there are no more results to return, the `next_cursor` field
    /// will be `None`.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,

    /// How long (in milliseconds) the client may cache this result before
    /// re-fetching, analogous to HTTP `Cache-Control: max-age`.
    ///
    /// Mandatory under MCP 2026-07-28, not an optional hint: `0` means treat
    /// the result as immediately stale. neva always emits it; on the way in a
    /// peer that omits it is read as `0` rather than failing the parse.
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(rename = "ttlMs", default)]
    pub ttl_ms: u64,

    /// Whether this result may be cached across authorization contexts.
    ///
    /// Mandatory under MCP 2026-07-28. Defaults to
    /// [`CacheScope::Private`](crate::types::cache::CacheScope::Private).
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(rename = "cacheScope", default)]
    pub cache_scope: crate::types::CacheScope,
}

/// Used by the client to invoke a tool provided by the server.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolRequestParams {
    /// Tool name.
    pub name: String,

    /// Optional arguments to pass to the tool.
    ///
    /// Omitted from the wire when absent rather than written as `null`: the
    /// schema types this as an optional object, and `null` is not one -- a
    /// strict peer rejects the call for the field it was not given rather than
    /// for the arguments it was.
    #[serde(rename = "arguments", default, skip_serializing_if = "Option::is_none")]
    pub args: Option<HashMap<String, Value>>,

    /// If specified, the caller is requesting task-augmented execution for this request.
    /// The request will return a [`crate::types::CreateTaskResult`] immediately, and the actual result can be
    /// retrieved later via `tasks/result`.
    ///
    /// **Note:** Task augmentation is subject to capability negotiation - receivers **MUST** declare support
    /// for task augmentation of specific request types in their capabilities.
    #[cfg(feature = "tasks")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,

    /// Metadata related to the request that provides additional protocol-level information.
    ///
    /// > **Note:** This can include progress tracking tokens and other protocol-specific properties
    /// > that are not part of the primary request parameters.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

/// Represents an input schema
#[cfg(feature = "legacy-spec")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolSchema {
    /// Schema object type
    ///
    /// > Note: always "object"
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A list of properties for command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, SchemaProperty>>,

    /// The required properties of the schema
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// `type` / `properties` / `required` are what neva itself reads; a schema
    /// is a whole document, and `$schema`, `$defs`, `$ref`, `additionalProperties`,
    /// `allOf` and the `if`/`then`/`else` triple are all meaningful to the peer
    /// that receives it. Dropping them would publish a schema quietly wider
    /// than the one the tool declared -- SEP-2106 requires the vocabulary to
    /// survive untouched -- so everything else round-trips through here.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Represents schema property description
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchemaProperty {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A Human-readable description of a property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
}

/// One value-carrying argument of a tool handler.
///
/// Produced by [`ToolHandler::args`] in the handler's own parameter order and
/// turned into the tool's `inputSchema`: the property under the argument's
/// name, listed in `required` unless the parameter is an `Option<T>`.
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct ToolArg {
    /// The schema property published for the argument.
    pub property: SchemaProperty,

    /// Whether a call must supply the argument.
    ///
    /// `false` for an `Option<T>` parameter, which resolves to `None` when a
    /// call leaves it out.
    pub required: bool,
}

/// Additional properties describing a Tool to clients.
///
/// > **Note:** All properties in ToolAnnotations are **hints**.
/// > They are not guaranteed to provide a faithful description of
/// > tool behavior (including descriptive properties like `title`).
/// > Clients should never make tool use decisions based on [`ToolAnnotations`]
/// > received from untrusted servers.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolAnnotations {
    /// A human-readable title for the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// If `true`, the tool may perform destructive updates to its environment.
    /// If `false`, the tool performs only additive updates.
    ///
    /// **Note:** This property is meaningful only when `readonly == false`
    ///
    /// Default: `true`
    #[serde(rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive: Option<bool>,

    /// If `true`, calling the tool repeatedly with the same arguments
    /// will have no additional effect on its environment.
    ///
    /// **Note:** This property is meaningful only when `readonly == false`
    ///
    /// Default: `false`
    #[serde(rename = "idempotentHint", skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,

    /// If `true`, this tool may interact with an **"open world"** of external entities.
    /// If `false`, the tool's domain of interaction is closed.
    ///
    /// For example, the world of a web search tool is open, whereas that
    /// of a memory tool is not.
    ///
    /// Default: `true`
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world: Option<bool>,

    /// If `true`, the tool does not modify its environment.
    ///
    /// Default: `false`
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
}

#[cfg(feature = "server")]
impl IntoResponse for ListToolsResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into()),
        }
    }
}

#[cfg(feature = "server")]
impl From<Vec<Tool>> for ListToolsResult {
    #[inline]
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    fn from(tools: Vec<Tool>) -> Self {
        Self {
            next_cursor: None,
            tools,
            ..Default::default()
        }
    }
}

#[cfg(feature = "server")]
impl From<Page<'_, Tool>> for ListToolsResult {
    #[inline]
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    fn from(page: Page<'_, Tool>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            tools: page.items.to_vec(),
            ..Default::default()
        }
    }
}

#[cfg(feature = "server")]
impl ListToolsResult {
    /// Create a new [`ListToolsResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

#[cfg(feature = "client")]
impl ListToolsResult {
    /// Get tool by name
    #[inline]
    pub fn get(&self, name: impl AsRef<str>) -> Option<&Tool> {
        self.get_by(|t| t.name == name.as_ref())
    }

    /// Get tool by condition
    #[inline]
    pub fn get_by<F>(&self, mut f: F) -> Option<&Tool>
    where
        F: FnMut(&Tool) -> bool,
    {
        self.tools.iter().find(|&t| f(t))
    }
}

#[cfg(feature = "legacy-spec")]
impl Default for ToolSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::Object,
            properties: Some(HashMap::new()),
            required: None,
            extra: Default::default(),
        }
    }
}

impl Default for ToolAnnotations {
    #[inline]
    fn default() -> Self {
        Self {
            title: None,
            destructive: Some(true),
            idempotent: Some(false),
            open_world: Some(true),
            readonly: Some(false),
        }
    }
}

#[cfg(feature = "tasks")]
impl From<&str> for TaskSupport {
    #[inline]
    fn from(value: &str) -> Self {
        match value {
            "forbidden" => Self::Forbidden,
            "required" => Self::Required,
            "optional" => Self::Optional,
            _ => unreachable!(),
        }
    }
}

#[cfg(feature = "tasks")]
impl From<String> for TaskSupport {
    #[inline]
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

#[cfg(all(feature = "server", feature = "legacy-spec"))]
impl ToolSchema {
    /// Creates a new [`ToolSchema`] object
    #[inline]
    pub(crate) fn new(
        props: Option<HashMap<String, SchemaProperty>>,
        required: Option<Vec<String>>,
    ) -> Self {
        Self {
            r#type: PropertyType::Object,
            properties: props,
            required,
            extra: Default::default(),
        }
    }

    /// Deserializes a new [`ToolSchema`] from a JSON string.
    ///
    /// > **Panics:** This constructor panics on malformed JSON and is kept
    /// > with its existing signature for backwards compatibility. Prefer
    /// > [`ToolSchema::from_value`] when the input is already a parsed
    /// > [`serde_json::Value`] and you want fallible deserialization.
    #[inline]
    pub fn from_json_str(json: &str) -> Self {
        serde_json::from_str(json).expect("InputSchema: Incorrect JSON string provided")
    }

    /// Builds a [`ToolSchema`] from a [`serde_json::Value`].
    ///
    /// Unlike [`crate::types::schema_2020::InputSchema::from_value`], which
    /// is infallible because the 2026-07-28 schema type is a transparent
    /// [`serde_json::Value`] newtype, this constructor is **fallible**:
    /// the legacy [`ToolSchema`] is a typed subset of JSON Schema and the
    /// supplied value must deserialize into that typed shape. Any
    /// deserialization error is returned through [`crate::error::Error`].
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error`] when `value` cannot be deserialized
    /// into a [`ToolSchema`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::tool::ToolSchema;
    /// use serde_json::json;
    ///
    /// let schema = ToolSchema::from_value(json!({
    ///     "type": "object",
    ///     "properties": { "name": { "type": "string" } }
    /// })).expect("valid schema");
    /// assert!(schema.properties.is_some());
    /// ```
    #[inline]
    pub fn from_value(value: Value) -> Result<Self, crate::error::Error> {
        let schema = serde_json::from_value(value)?;
        Ok(schema)
    }

    /// Adds a new property into the schema.
    /// If a property with this name already exists, it overwrites it
    pub fn with_prop<T: Into<PropertyType>>(
        self,
        name: &str,
        descr: &str,
        property_type: T,
    ) -> Self {
        self.add_property_impl(name, descr, property_type.into())
    }

    /// Adds a new required property into the schema.
    /// If a property with this name already exists, it overwrites it
    pub fn with_required<T: Into<PropertyType>>(
        self,
        name: &str,
        descr: &str,
        property_type: T,
    ) -> Self {
        self.add_required_property_impl(name, descr, property_type.into())
    }

    /// Builder-style: extend `self` with the properties of a
    /// [`schemars`]-generated [`JsonSchema`] type.
    ///
    /// Note that this is distinct from the static [`ToolSchema::from_schema`]
    /// -- `with_schema` is a chainable instance method, while `from_schema` is
    /// a static constructor.
    pub fn with_schema<T: JsonSchema>(self) -> Self {
        let json_schema = schemars::schema_for!(T);
        self.with_schema_impl(json_schema)
    }

    /// Creates a new [`ToolSchema`] from a type that implements
    /// [`schemars::JsonSchema`].
    ///
    /// Mirrors [`crate::types::schema_2020::InputSchema::from_schema`] so
    /// that both schema flavours expose the same generic-constructor API
    /// surface: `Foo::from_schema::<T>()`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::tool::ToolSchema;
    /// use schemars::JsonSchema;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize, JsonSchema)]
    /// struct Args { name: String }
    ///
    /// let schema = ToolSchema::from_schema::<Args>();
    /// assert!(schema.properties.is_some());
    /// ```
    #[inline]
    pub fn from_schema<T: JsonSchema>() -> Self {
        let json_schema = schemars::schema_for!(T);
        Self::from_schemars(json_schema)
    }

    /// Creates a new [`ToolSchema`] from an already-built
    /// [`schemars::Schema`].
    ///
    /// Mirrors [`crate::types::schema_2020::InputSchema::from_schemars`].
    /// Use this when you have a hand-built [`schemars::Schema`] (or one
    /// produced by a `SchemaSettings` builder) and want to attach it to a
    /// tool without going through the [`schemars::schema_for!`] macro.
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::tool::ToolSchema;
    /// use schemars::JsonSchema;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize, JsonSchema)]
    /// struct Args { name: String }
    ///
    /// let schema = ToolSchema::from_schemars(schemars::schema_for!(Args));
    /// assert!(schema.properties.is_some());
    /// ```
    #[inline]
    pub fn from_schemars(json_schema: schemars::Schema) -> Self {
        Self::default().with_schema_impl(json_schema)
    }

    // Deprecated: renamed to `from_schemars` for symmetry with
    // `InputSchema::from_schemars`. The new generic static
    // `ToolSchema::from_schema::<T>()` matches `InputSchema::from_schema::<T>()`,
    // freeing the `from_schema(schemars::Schema)` name for the generic form.
    /// Creates a new [`ToolSchema`] from a [`schemars::Schema`].
    ///
    /// **Deprecated:** renamed to [`ToolSchema::from_schemars`] for symmetry
    /// with [`crate::types::schema_2020::InputSchema::from_schemars`]. The
    /// `from_schema` name is now occupied by the generic static constructor
    /// [`ToolSchema::from_schema::<T>()`].
    #[deprecated(note = "renamed to from_schemars for symmetry with InputSchema")]
    #[inline]
    pub fn from_schema_legacy(json_schema: schemars::Schema) -> Self {
        Self::from_schemars(json_schema)
    }

    #[inline]
    fn with_schema_impl(mut self, json_schema: schemars::Schema) -> Self {
        let required = json_schema.get("required").and_then(|v| v.as_array());
        if let Some(props) = json_schema.get("properties").and_then(|v| v.as_object()) {
            for (field, def) in props {
                let req = required
                    .map(|arr| !arr.iter().any(|v| v == field))
                    .unwrap_or(true);
                let type_str = def.get("type").and_then(|v| v.as_str()).unwrap_or("string");
                self = if req {
                    self.add_required_property_impl(field, field, type_str.into())
                } else {
                    self.add_property_impl(field, field, type_str.into())
                };
            }
        }
        self
    }

    #[inline]
    fn add_property_impl(mut self, name: &str, descr: &str, property_type: PropertyType) -> Self {
        self.properties.get_or_insert_with(HashMap::new).insert(
            name.into(),
            SchemaProperty {
                r#type: property_type,
                descr: Some(descr.into()),
            },
        );
        self
    }

    #[inline]
    fn add_required_property_impl(
        mut self,
        name: &str,
        descr: &str,
        property_type: PropertyType,
    ) -> Self {
        self = self.add_property_impl(name, descr, property_type);
        self.required.get_or_insert_with(Vec::new).push(name.into());
        self
    }
}

#[cfg(feature = "server")]
impl SchemaProperty {
    /// Creates a new [`SchemaProperty`] for a `T`
    #[inline]
    pub(crate) fn new<T: TypeCategory>() -> Self {
        Self {
            r#type: T::category(),
            descr: None,
        }
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for CallToolRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for ListToolsRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

/// Describes a generic MCP Tool handler
#[cfg(feature = "server")]
pub trait ToolHandler<Args>: GenericHandler<Args> {
    /// Returns the handler's value-carrying arguments, in declaration order.
    ///
    /// Parameters extracted from request metadata ([`crate::types::Meta`],
    /// [`Context`], DI) are not arguments and do not appear here, so the
    /// position in this list is the slot [`crate::types::ArgNames`] indexes.
    /// An `Option<T>` parameter *is* an argument -- it occupies a slot and is
    /// published -- it is simply not required.
    #[inline]
    fn args() -> Vec<ToolArg> {
        Vec::new()
    }
}

#[cfg(feature = "server")]
pub(crate) struct ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: FromHandlerArgs<CallToolRequestParams>,
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

#[cfg(feature = "server")]
impl<F, R, Args> ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: FromHandlerArgs<CallToolRequestParams>,
{
    /// Creates a new [`ToolFunc`] wrapped into [`Arc`]
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self {
            func,
            _marker: std::marker::PhantomData,
        };
        Arc::new(func)
    }
}

#[cfg(feature = "server")]
impl<F, R, Args> Handler<CallToolResponse> for ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: FromHandlerArgs<CallToolRequestParams> + Send + Sync,
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<'_, Result<CallToolResponse, Error>> {
        let HandlerParams::Tool(params, names) = params else {
            unreachable!()
        };
        Box::pin(async move {
            let args = Args::from_args(params, &names)?;
            Ok(self.func.call(args).await.into())
        })
    }
}

impl CallToolRequestParams {
    /// Creates a new [`CallToolRequestParams`] for the given tool name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: None,
            meta: None,
            #[cfg(feature = "tasks")]
            task: None,
        }
    }

    /// Specifies tool arguments
    pub fn with_args<Args: shared::IntoArgs>(mut self, args: Args) -> Self {
        self.args = args.into_args();
        self
    }

    /// Sets the metadata for the request
    pub fn with_meta(mut self, meta: RequestParamsMeta) -> Self {
        self.meta = Some(meta);
        self
    }

    /// Sets the TTL for the [`CallToolRequestParams`],
    /// which will be used if the tool is support tasks.
    #[cfg(feature = "tasks")]
    pub fn with_ttl(mut self, ttl: Option<usize>) -> Self {
        self.task = Some(TaskMetadata { ttl });
        self
    }
}

#[cfg(feature = "server")]
impl CallToolRequestParams {
    /// Includes [`Context`] into request metadata. If metadata is `None` it creates a new.
    pub(crate) fn with_context(mut self, ctx: Context) -> Self {
        self.meta.get_or_insert_default().context = Some(ctx);
        self
    }

    /// Associates [`CallToolRequestParams`] with the appropriated task
    #[cfg(feature = "tasks")]
    pub(crate) fn with_task(mut self, task_id: impl Into<String>) -> Self {
        self.meta.get_or_insert_default().task = Some(RelatedTaskMetadata { id: task_id.into() });
        self
    }
}

impl Debug for Tool {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tool")
            .field("name", &self.name)
            .field("title", &self.title)
            .field("descr", &self.descr)
            .field("input_schema", &self.input_schema)
            .field("output_schema", &self.output_schema)
            .field("annotations", &self.annotations)
            .field("meta", &self.meta)
            .finish()
    }
}

/// Builds a [`crate::types::ToolInputSchema`] from the ordered argument list
/// produced by [`ToolHandler::args`], naming the *n*-th property after the
/// *n*-th entry of `names`.
///
/// Naming the properties from the very [`ArgNames`] extraction reads by is
/// what keeps the published schema and the handler in agreement: a peer is
/// told to send exactly the keys the handler will look for.
///
/// Under `legacy-spec` this returns the typed legacy `ToolSchema`. In the
/// default (MCP 2026-07-28) build the legacy `ToolSchema` struct is absent, so
/// this constructs an [`crate::types::schema_2020::InputSchema`] by
/// serializing each [`SchemaProperty`] into a `serde_json::Value` and wrapping
/// the result as a JSON Schema 2020-12 object schema. The same call site at
/// [`Tool::new`] compiles under either feature set.
#[cfg(feature = "server")]
#[inline]
fn build_input_schema_from_args(
    args: &[ToolArg],
    names: &ArgNames,
) -> crate::types::ToolInputSchema {
    #[cfg(feature = "legacy-spec")]
    {
        if args.is_empty() {
            return ToolSchema::new(None, None);
        }
        let props = args
            .iter()
            .enumerate()
            .map(|(idx, arg)| (names.get(idx).to_owned(), arg.property.clone()))
            .collect::<HashMap<_, _>>();
        let required = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| arg.required)
            .map(|(idx, _)| names.get(idx).to_owned())
            .collect::<Vec<_>>();
        let required = (!required.is_empty()).then_some(required);
        ToolSchema::new(Some(props), required)
    }
    #[cfg(not(feature = "legacy-spec"))]
    {
        use serde_json::{Map, Value, json};
        let mut properties = Map::with_capacity(args.len());
        let mut required = Vec::with_capacity(args.len());
        for (idx, arg) in args.iter().enumerate() {
            let name = names.get(idx);
            let prop =
                serde_json::to_value(&arg.property).unwrap_or_else(|_| Value::Object(Map::new()));
            properties.insert(name.to_owned(), prop);
            if arg.required {
                required.push(Value::String(name.to_owned()));
            }
        }
        let value = if required.is_empty() {
            json!({ "type": "object", "properties": properties })
        } else {
            json!({ "type": "object", "properties": properties, "required": required })
        };
        crate::types::schema_2020::InputSchema::from(value)
    }
}

/// Whether the schema can put a property *name* in front of a peer that its
/// top-level `properties` map does not list.
///
/// This is the question the startup check needs answered, and it is narrower
/// than "is the schema open". A peer builds its call from what the schema
/// *advertises*: permitting further names is not the same as naming one, and
/// an argument nothing names is one no schema-driven caller can ever send.
/// So `additionalProperties`, `unevaluatedProperties` and `propertyNames` are
/// not exemptions however they are set -- they widen what is accepted while
/// naming nothing -- and a tool whose argument only they would admit is still
/// worth reporting.
///
/// What does advertise a name from outside the map:
///
/// * composition -- `$ref` and friends pull in properties defined elsewhere;
/// * `patternProperties` -- names properties by regex rather than literally.
///
/// Following either properly means evaluating the schema, which is far more
/// than a startup sanity check should carry, so a schema that uses them is
/// left alone rather than failed on a guess.
///
/// `not` is the one composition keyword missing from that list, because it
/// composes in the opposite direction: a subschema under `not` describes what
/// an instance must *fail*, so a name appearing there is one a peer is being
/// told to avoid, never one it could be sent.
#[cfg(all(feature = "server", not(feature = "legacy-spec")))]
fn advertises_properties_elsewhere(schema: &serde_json::Map<String, Value>) -> bool {
    const ADVERTISES: [&str; 9] = [
        "$ref",
        "allOf",
        "anyOf",
        "oneOf",
        "if",
        "then",
        "else",
        "dependentSchemas",
        "patternProperties",
    ];

    ADVERTISES.iter().any(|kw| schema.contains_key(*kw))
}

/// Renames the generated properties of `schema` from the names the arguments
/// are currently read by to the names they are about to be read by.
///
/// Renaming *from the current names* rather than from the positional form is
/// what lets [`Tool::with_arg_names`] be called more than once -- which
/// `map_tool!` already does once on the caller's behalf. After the first call
/// there are no `argN` properties left to find, and looking for them would
/// leave the schema on the old names while the handler moved to the new ones.
///
/// Only keys `from` actually names are touched, so a hand-written schema
/// (which has none of them) passes through untouched.
///
/// The rename happens in two passes -- take every source property out, then
/// put them all back under their new names -- because a new name may be a
/// source key for a *later* slot: a handler is perfectly entitled to a
/// parameter called `arg1`. Renaming in place would drop that parameter's
/// property on top of a source not yet moved.
#[cfg(feature = "server")]
#[inline]
fn rename_args(schema: &mut crate::types::ToolInputSchema, from: &ArgNames, to: &ArgNames) {
    #[cfg(feature = "legacy-spec")]
    {
        if let Some(props) = schema.properties.as_mut() {
            let taken = (0..to.len())
                .map(|slot| props.remove(from.get(slot)))
                .collect::<Vec<_>>();
            for (slot, prop) in taken.into_iter().enumerate() {
                if let Some(prop) = prop {
                    props.insert(to.get(slot).to_owned(), prop);
                }
            }
        }
        if let Some(required) = schema.required.as_mut() {
            // Each entry is visited once and tested against the name it came
            // in with, so a rewritten entry is never re-read as a source.
            for name in required.iter_mut() {
                if let Some(slot) = from.slot_of(name) {
                    *name = to.get(slot).to_owned();
                }
            }
        }
    }
    #[cfg(not(feature = "legacy-spec"))]
    {
        let Some(schema) = schema.0.as_object_mut() else {
            return;
        };
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            let taken = (0..to.len())
                .map(|slot| props.remove(from.get(slot)))
                .collect::<Vec<_>>();
            for (slot, prop) in taken.into_iter().enumerate() {
                if let Some(prop) = prop {
                    props.insert(to.get(slot).to_owned(), prop);
                }
            }
        }
        if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
            for name in required.iter_mut() {
                if let Some(slot) = name.as_str().and_then(|name| from.slot_of(name)) {
                    *name = Value::String(to.get(slot).to_owned());
                }
            }
        }
    }
}

#[cfg(feature = "server")]
impl Tool {
    /// Initializes a new [`Tool`]
    pub fn new<F, Args, R>(name: impl Into<String>, handler: F) -> Self
    where
        F: ToolHandler<Args, Output = R>,
        R: Into<CallToolResponse> + Send + 'static,
        Args: FromHandlerArgs<CallToolRequestParams> + Send + Sync + 'static,
    {
        let handler = ToolFunc::new(handler);
        let args = F::args();
        let arg_names = ArgNames::positional(args.len());
        let input_schema = build_input_schema_from_args(&args, &arg_names);
        Self {
            name: name.into(),
            title: None,
            descr: None,
            input_schema,
            output_schema: None,
            meta: None,
            annotations: None,
            handler: Some(handler),
            arg_names,
            custom_schema: false,
            icons: None,
            #[cfg(feature = "http-server")]
            roles: None,
            #[cfg(feature = "http-server")]
            permissions: None,
            #[cfg(feature = "tasks")]
            exec: None,
        }
    }

    /// Sets a title for a tool
    pub fn with_title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    /// Sets a description for a tool
    pub fn with_description(&mut self, description: &str) -> &mut Self {
        self.descr = Some(description.into());
        self
    }

    /// Sets an input schema for the tool.
    ///
    /// > **Note:** Automatically generated schema will be overwritten
    ///
    /// The closure receives and returns a [`crate::types::ToolInputSchema`].
    /// Under `legacy-spec` this is the typed `ToolSchema`
    /// (with builder methods like `with_prop`/`with_required`); in the
    /// default (MCP 2026-07-28) build it is
    /// [`crate::types::schema_2020::InputSchema`] (a Value-shaped JSON
    /// Schema 2020-12 wrapper). The schema model differs between flags,
    /// so closure bodies that rely on the typed builder API do not carry
    /// across the two profiles by design.
    pub fn with_input_schema<F>(&mut self, config: F) -> &mut Self
    where
        F: FnOnce(crate::types::ToolInputSchema) -> crate::types::ToolInputSchema,
    {
        self.input_schema = config(Default::default());
        self.custom_schema = true;
        self
    }

    /// Declares the names of the handler's arguments, in the order the handler
    /// takes them.
    ///
    /// Arguments are extracted from a call's `arguments` map **by name**, and
    /// a tool registered from a bare closure has no names to extract by --
    /// Rust does not keep a closure's parameter names. Such a tool therefore
    /// publishes the positional `arg0`, `arg1`, ... properties and reads those
    /// same keys. This method replaces both at once: the auto-generated
    /// `inputSchema` properties are renamed and extraction starts reading the
    /// new names, so what a peer is told to send cannot drift from what the
    /// handler looks for.
    ///
    /// Only the value-carrying parameters are named. Parameters served from
    /// request metadata -- [`crate::types::Meta`], [`Context`], a DI-injected
    /// `Dc<T>` -- are skipped here exactly as they are skipped in the schema.
    /// An `Option<T>` parameter *is* named: it occupies an argument slot and
    /// is published like any other, it is simply not in `required`.
    ///
    /// Tools declared with the `#[tool]` macro call this for you with the
    /// function's own parameter names; there is nothing to do for those.
    ///
    /// > **Note:** a schema supplied through [`Self::with_input_schema`] is
    /// > taken verbatim and never renamed -- name its properties as you name
    /// > them here. The two calls may appear in either order.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_tool("greet", |name: String, age: i32| async move {
    ///     format!("Hello, {name}! You are {age}.")
    /// })
    /// .with_arg_names(["name", "age"]);
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn with_arg_names<T, I>(&mut self, names: T) -> &mut Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        let declared = self.arg_names.declare(names);
        // Only a schema this crate generated is rewritten. A hand-written one
        // is the caller's text, and every key in it was chosen on purpose --
        // renaming a property that merely looks positional would silently
        // overwrite whatever they named it after.
        if !self.custom_schema {
            rename_args(&mut self.input_schema, &self.arg_names, &declared);
        }
        self.arg_names = declared;
        self
    }

    /// Sets an output schema for the tool.
    ///
    /// > **Note:** Automatically generated schema will be overwritten
    ///
    /// See [`Self::with_input_schema`] for the closure-type note that
    /// applies under each feature flag.
    pub fn with_output_schema<F>(&mut self, config: F) -> &mut Self
    where
        F: FnOnce(crate::types::ToolInputSchema) -> crate::types::ToolInputSchema,
    {
        self.output_schema = Some(config(Default::default()));
        self
    }

    /// Sets a list of roles that are allowed to invoke the tool
    #[cfg(feature = "http-server")]
    pub fn with_roles<T, I>(&mut self, roles: T) -> &mut Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.roles = Some(roles.into_iter().map(Into::into).collect());
        self
    }

    /// Sets a list of permissions that are allowed to invoke the tool
    #[cfg(feature = "http-server")]
    pub fn with_permissions<T, I>(&mut self, permissions: T) -> &mut Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>,
    {
        self.permissions = Some(permissions.into_iter().map(Into::into).collect());
        self
    }

    /// Configures the annotations for the tool
    pub fn with_annotations<F>(&mut self, config: F) -> &mut Self
    where
        F: FnOnce(ToolAnnotations) -> ToolAnnotations,
    {
        self.annotations = Some(config(Default::default()));
        self
    }

    /// Sets the [`Tool`] icons
    pub fn with_icons(&mut self, icons: impl IntoIterator<Item = Icon>) -> &mut Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }

    /// Sets the [`Tool`] icons
    #[cfg(feature = "tasks")]
    pub fn with_task_support(&mut self, support: impl Into<TaskSupport>) -> &mut Self {
        self.exec = Some(ToolExecution::new(support.into()));
        self
    }

    /// Describes how the tool's published `inputSchema` and its handler
    /// disagree about the arguments, if they do.
    ///
    /// The two are generated together and cannot drift on their own. They can
    /// be pulled apart by hand: replacing the schema with
    /// [`Self::with_input_schema`] renames what peers are told to send without
    /// touching what the handler reads, and a miscounted
    /// [`Self::with_arg_names`] leaves trailing arguments unnamed. Both make
    /// every call to the tool fail, so they are worth catching at startup
    /// rather than on a peer's first call.
    pub(crate) fn arg_name_conflict(&self) -> Option<String> {
        let arity = self.arg_names.arity();

        // A declaration is checked against the handler whatever the arity is.
        // Zero is a real count, not an absence of one: names given to a
        // handler that reads none are as much a miscount as too few names for
        // one that reads several, and describe a tool just as broken.
        if self.arg_names.is_declared() {
            let declared = self.arg_names.len();
            if declared != arity {
                return Some(format!(
                    "tool `{}` declares {declared} argument name(s) but its handler takes \
                     {arity}. Name every argument the handler reads, metadata parameters \
                     (`Context`, `Meta<_>`, `Dc<_>`) excluded.",
                    self.name,
                ));
            }
            if let Some(duplicate) = self.arg_names.duplicate() {
                return Some(format!(
                    "tool `{}` declares the argument name `{duplicate}` twice. Arguments are \
                     read from a call by name, so two parameters sharing one name would both \
                     be handed the same value.",
                    self.name,
                ));
            }
        }

        // Past here every check reads an argument, so a handler that takes
        // none has nothing left to disagree with the schema about.
        if arity == 0 {
            return None;
        }

        // Whichever names the handler reads by -- declared or positional --
        // the schema has to ask peers for those very keys, or a call built
        // faithfully from the schema still misses every argument.
        //
        // Only checked against a schema that describes every argument in one
        // top-level `properties` map. A schema that composes -- `$ref`,
        // `allOf`, `oneOf`, a conditional branch -- may well publish an
        // argument somewhere this cannot follow, and a heuristic that guessed
        // there would fail tools that are perfectly well-formed. Resolving
        // composition properly means a full JSON Schema evaluator, which is
        // far more than a startup sanity check should carry.
        let properties = self.schema_properties()?;
        let missing = (0..arity)
            .map(|slot| self.arg_names.get(slot))
            .find(|name| !properties(name))?;

        Some(if self.arg_names.is_declared() {
            format!(
                "tool `{}` declares the argument `{missing}` but publishes an inputSchema \
                 without it. A peer sends what the schema asks for, so the two have to name \
                 the same arguments: either rename the schema property, or pass the schema's \
                 own names to `.with_arg_names([...])`.",
                self.name,
            )
        } else {
            format!(
                "tool `{}` publishes an inputSchema without the argument `{missing}` that its \
                 handler reads. A tool registered from a closure has no argument names -- Rust \
                 does not keep a closure's parameter names -- so it reads the positional `arg0`, \
                 `arg1`, ... keys, and replacing its schema renamed only what peers are told to \
                 send. Declare the names with `.with_arg_names([...])`, or register the tool with \
                 the `map_tool!` macro or the `#[tool]` attribute.",
                self.name,
            )
        })
    }

    /// A predicate over the property names the input schema publishes, or
    /// `None` when the schema does not describe all of them in one top-level
    /// `properties` map.
    #[inline]
    fn schema_properties(&self) -> Option<impl Fn(&str) -> bool + '_> {
        // The legacy schema is a closed struct of `type`/`properties`/
        // `required` -- it cannot compose, so its map is always the whole
        // story.
        #[cfg(feature = "legacy-spec")]
        let props = self.input_schema.properties.as_ref()?;

        #[cfg(not(feature = "legacy-spec"))]
        let props = {
            let schema = self.input_schema.as_value().as_object()?;
            if advertises_properties_elsewhere(schema) {
                return None;
            }
            schema.get("properties").and_then(Value::as_object)?
        };

        Some(move |name: &str| props.contains_key(name))
    }

    /// Invoke a tool
    #[inline]
    pub(crate) async fn call(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResponse, Error> {
        match self.handler {
            Some(ref handler) => {
                handler
                    .call(HandlerParams::Tool(params, self.arg_names.clone()))
                    .await
            }
            None => Err(Error::new(
                ErrorCode::InternalError,
                "Tool handler not specified",
            )),
        }
    }
}

#[cfg(feature = "client")]
impl Tool {
    /// Validates [`CallToolResponse`] against this tool output schema.
    ///
    /// Under the legacy feature set the schema is the typed `ToolSchema`
    /// struct and is materialized via [`serde_json::to_value`]. Under
    /// MCP 2026-07-28 the schema is already a [`serde_json::Value`]
    /// (wrapped by [`crate::types::schema_2020::InputSchema`]), so we borrow
    /// it directly via [`crate::types::schema_2020::InputSchema::as_value`]
    /// -- no re-serialization is needed.
    pub fn validate<'a>(&self, resp: &'a CallToolResponse) -> Result<&'a CallToolResponse, Error> {
        let Some(schema_ref) = self.output_schema.as_ref() else {
            return Err(Error::new(
                ErrorCode::ParseError,
                "Tool: Output schema not specified",
            ));
        };

        #[cfg(feature = "legacy-spec")]
        let schema = serde_json::to_value(schema_ref).map_err(Into::<Error>::into)?;
        #[cfg(not(feature = "legacy-spec"))]
        let schema = schema_ref.as_value().clone();

        let validator =
            validator_for(&schema).map_err(|err| Error::new(ErrorCode::ParseError, err))?;

        let content = resp.struct_content()?;
        validator
            .validate(content)
            .map(|_| resp)
            .map_err(|err| Error::new(ErrorCode::ParseError, err.to_string()))
    }
}

#[cfg(feature = "tasks")]
impl Tool {
    /// Returns a task support for the tool if specified.
    #[inline]
    pub fn task_support(&self) -> Option<TaskSupport> {
        self.exec.as_ref().and_then(|e| e.task_support)
    }
}

#[cfg(feature = "server")]
impl ToolAnnotations {
    /// Creates a new [`ToolAnnotations`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }

    /// Deserializes a new [`ToolAnnotations`] from a JSON string
    #[inline]
    pub fn from_json_str(json: &str) -> Self {
        serde_json::from_str(json).expect("ToolAnnotations: Incorrect JSON string provided")
    }

    /// Sets a title for the tool.
    #[inline]
    pub fn with_title(mut self, title: &str) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets/Unsets a hint that the tool may perform destructive updates to its environment.
    ///
    /// Also sets the readonly hint to `false`
    #[inline]
    pub fn with_destructive(mut self, destructive: bool) -> Self {
        self.destructive = Some(destructive);
        self.readonly = Some(false);
        self
    }

    /// Sets/Unsets a hint that the tool is idempotent.
    /// So calling it repeatedly when it's `true` with the same arguments
    /// will have no additional effect on its environment.
    ///
    /// Also sets the readonly hint to `false`
    pub fn with_idempotent(mut self, idempotent: bool) -> Self {
        self.idempotent = Some(idempotent);
        self.readonly = Some(false);
        self
    }

    /// Sets/Unsets the hint that the tool may interact with an **"open world"** of external entities.
    #[inline]
    pub fn with_open_world(mut self, open_world: bool) -> Self {
        self.open_world = Some(open_world);
        self
    }
}

#[cfg(all(feature = "server", feature = "tasks"))]
impl ToolExecution {
    /// Creates a new [`ToolExecution`] with a task support
    #[inline]
    pub fn new(support: TaskSupport) -> Self {
        Self {
            task_support: Some(support),
        }
    }
}

macro_rules! impl_generic_tool_handler ({ $($param:ident)* } => {
    #[cfg(feature = "server")]
    impl<Func, Fut: Send, $($param: TypeCategory,)*> ToolHandler<($($param,)*)> for Func
    where
        Func: Fn($($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {
        #[inline]
        #[allow(unused_mut)]
        fn args() -> Vec<ToolArg> {
            let mut args = Vec::new();
            $(
            {
                let property = SchemaProperty::new::<$param>();
                // Metadata-served parameters are not arguments: they take no
                // schema property and consume no argument slot.
                if property.r#type != PropertyType::None {
                    args.push(ToolArg {
                        property,
                        required: !<$param as TypeCategory>::is_optional(),
                    });
                }
            };
            )*
            args
        }
    }
});

impl_generic_tool_handler! {}
impl_generic_tool_handler! { T1 }
impl_generic_tool_handler! { T1 T2 }
impl_generic_tool_handler! { T1 T2 T3 }
impl_generic_tool_handler! { T1 T2 T3 T4 }
impl_generic_tool_handler! { T1 T2 T3 T4 T5 }

#[cfg(test)]
#[cfg(feature = "server")]
mod tests {
    use super::*;
    use serde_json::json;

    fn call_params(args: [(&str, Value); 2]) -> CallToolRequestParams {
        CallToolRequestParams {
            name: "sum".into(),
            meta: None,
            #[cfg(feature = "tasks")]
            task: None,
            args: Some(args.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()),
        }
    }

    /// The property names of an `inputSchema`, sorted.
    fn schema_props(tool: &Tool) -> Vec<String> {
        #[cfg(feature = "legacy-spec")]
        let props = tool.input_schema.properties.as_ref().unwrap();
        #[cfg(not(feature = "legacy-spec"))]
        let props = tool.input_schema.as_value()["properties"]
            .as_object()
            .unwrap();

        let mut names = props.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    #[tokio::test]
    async fn it_creates_and_calls_tool() {
        // A closure keeps no parameter names, so the tool publishes the
        // positional ones and reads exactly those.
        let tool = Tool::new("sum", |a: i32, b: i32| async move { a + b });

        assert_eq!(schema_props(&tool), ["arg0", "arg1"]);

        let params = call_params([("arg0", json!(5)), ("arg1", json!(2))]);
        let resp = tool.call(params).await.unwrap();
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(
            json,
            r#"{"content":[{"type":"text","text":"7"}],"isError":false}"#
        );
    }

    #[tokio::test]
    async fn it_calls_a_tool_with_declared_arg_names() {
        let mut tool = Tool::new("sum", |a: i32, b: i32| async move { a - b });
        tool.with_arg_names(["a", "b"]);

        // Declaring the names renames the published properties too, so a peer
        // is told to send the keys the handler actually reads.
        assert_eq!(schema_props(&tool), ["a", "b"]);

        let params = call_params([("b", json!(2)), ("a", json!(5))]);
        let resp = tool.call(params).await.unwrap();
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(
            json,
            r#"{"content":[{"type":"text","text":"3"}],"isError":false}"#
        );
    }

    #[tokio::test]
    async fn it_does_not_swap_same_typed_args_of_different_types() {
        // The bug this extraction path replaces: with two differently typed
        // arguments, a map iteration order that put `name` first made the
        // call fail outright.
        let mut tool = Tool::new("greet", |name: String, age: i32| async move {
            format!("{name} is {age}")
        });
        tool.with_arg_names(["name", "age"]);

        for args in [
            [("name", json!("John")), ("age", json!(30))],
            [("age", json!(30)), ("name", json!("John"))],
        ] {
            let resp = tool.call(call_params(args)).await.unwrap();
            let json = serde_json::to_string(&resp).unwrap();

            assert_eq!(
                json,
                r#"{"content":[{"type":"text","text":"John is 30"}],"isError":false}"#
            );
        }
    }

    /// The `required` list of an `inputSchema`, sorted.
    fn schema_required(tool: &Tool) -> Vec<String> {
        #[cfg(feature = "legacy-spec")]
        let required = tool.input_schema.required.clone().unwrap_or_default();
        #[cfg(not(feature = "legacy-spec"))]
        let required = tool.input_schema.as_value()["required"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        let mut required: Vec<String> = required;
        required.sort();
        required
    }

    #[tokio::test]
    async fn it_publishes_an_optional_arg_as_not_required() {
        let mut tool = Tool::new("greet", |name: String, age: Option<i32>| async move {
            match age {
                Some(age) => format!("{name} is {age}"),
                None => format!("{name} is ageless"),
            }
        });
        tool.with_arg_names(["name", "age"]);

        // An optional argument is a property like any other -- a peer has to be
        // told it exists to be able to send it -- it is just not required.
        assert_eq!(schema_props(&tool), ["age", "name"]);
        assert_eq!(schema_required(&tool), ["name"]);

        let supplied = call_params([("name", json!("John")), ("age", json!(30))]);
        let resp = tool.call(supplied).await.unwrap();
        assert!(serde_json::to_string(&resp).unwrap().contains("John is 30"));

        let omitted = CallToolRequestParams {
            name: "greet".into(),
            meta: None,
            #[cfg(feature = "tasks")]
            task: None,
            args: Some(HashMap::from([("name".to_owned(), json!("John"))])),
        };
        let resp = tool.call(omitted).await.unwrap();
        assert!(
            serde_json::to_string(&resp)
                .unwrap()
                .contains("John is ageless")
        );
    }

    #[tokio::test]
    async fn it_reads_an_explicit_null_as_an_absent_optional_arg() {
        let mut tool = Tool::new(
            "greet",
            |age: Option<i32>| async move { format!("{age:?}") },
        );
        tool.with_arg_names(["age"]);

        let resp = tool
            .call(CallToolRequestParams {
                name: "greet".into(),
                meta: None,
                #[cfg(feature = "tasks")]
                task: None,
                args: Some(HashMap::from([("age".to_owned(), Value::Null)])),
            })
            .await
            .unwrap();

        assert!(serde_json::to_string(&resp).unwrap().contains("None"));
    }

    #[test]
    fn an_all_optional_tool_requires_nothing() {
        let tool = Tool::new("greet", |name: Option<String>| async move {
            name.unwrap_or_default()
        });

        assert_eq!(schema_props(&tool), ["arg0"]);
        assert!(schema_required(&tool).is_empty());
    }

    #[test]
    fn it_gives_same_typed_args_distinct_schema_properties() {
        // Keying the generated schema by type name collapsed both `i32`
        // arguments into a single `number` property.
        let tool = Tool::new("sum", |a: i32, b: i32| async move { a + b });

        assert_eq!(schema_props(&tool).len(), 2);
    }

    #[test]
    fn it_renames_a_parameter_that_is_itself_named_after_a_slot() {
        // Nothing stops a handler from calling a parameter `arg1`. Renaming
        // the generated properties one at a time would drop `arg1`'s property
        // onto the slot the *next* parameter still has to be moved out of.
        let mut tool = Tool::new("f", |arg1: String, other: String| async move {
            format!("{arg1}{other}")
        });
        tool.with_arg_names(["arg1", "other"]);

        assert_eq!(schema_props(&tool), ["arg1", "other"]);
        assert_eq!(schema_required(&tool), ["arg1", "other"]);
        assert!(tool.arg_name_conflict().is_none());
    }

    #[test]
    fn it_renames_when_a_declared_name_reuses_a_later_slot() {
        let mut tool = Tool::new("f", |other: String, arg0: String| async move {
            format!("{other}{arg0}")
        });
        tool.with_arg_names(["other", "arg0"]);

        assert_eq!(schema_props(&tool), ["arg0", "other"]);
        assert!(tool.arg_name_conflict().is_none());
    }

    #[tokio::test]
    async fn it_renames_again_when_names_are_redeclared() {
        // `map_tool!` already declares names on the caller's behalf, so a
        // second `with_arg_names` is an ordinary thing to do. By then there
        // are no positional properties left to rename from -- the rename has
        // to start from the names currently in force.
        let mut tool = Tool::new(
            "greet",
            |a: String, b: i32| async move { format!("{a}{b}") },
        );
        tool.with_arg_names(["name", "age"]);
        tool.with_arg_names(["who", "years"]);

        assert_eq!(schema_props(&tool), ["who", "years"]);
        assert_eq!(schema_required(&tool), ["who", "years"]);
        assert!(tool.arg_name_conflict().is_none());

        let resp = tool
            .call(call_params([("years", json!(30)), ("who", json!("John"))]))
            .await
            .unwrap();

        assert!(serde_json::to_string(&resp).unwrap().contains("John30"));
    }

    #[test]
    fn it_swaps_declared_names_without_losing_a_property() {
        let mut tool = Tool::new("f", |a: String, b: String| async move { format!("{a}{b}") });
        tool.with_arg_names(["first", "second"]);
        tool.with_arg_names(["second", "first"]);

        assert_eq!(schema_props(&tool), ["first", "second"]);
        assert!(tool.arg_name_conflict().is_none());
    }

    #[test]
    fn it_rejects_a_duplicate_declared_name() {
        let mut tool = Tool::new("f", |a: String, b: String| async move { format!("{a}{b}") });
        tool.with_arg_names(["value", "value"]);

        let conflict = tool.arg_name_conflict().expect("must be reported");

        assert!(
            conflict.contains("declares the argument name `value` twice"),
            "unexpected conflict: {conflict}"
        );
    }

    #[test]
    fn it_leaves_a_hand_written_schema_untouched() {
        let mut tool = Tool::new("sum", |a: i32, b: i32| async move { a + b });
        tool.with_input_schema(|_| {
            #[cfg(feature = "legacy-spec")]
            {
                ToolSchema::from_json_str(
                    r#"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}}}"#,
                )
            }
            #[cfg(not(feature = "legacy-spec"))]
            {
                crate::types::schema_2020::InputSchema::from(json!({
                    "type": "object",
                    "properties": { "a": { "type": "number" }, "b": { "type": "number" } }
                }))
            }
        })
        .with_arg_names(["a", "b"]);

        assert_eq!(schema_props(&tool), ["a", "b"]);
    }

    /// A hand-written schema is left alone even where it uses a name that
    /// looks like one this crate generates. `arg0` there is the caller's own
    /// property, not a leftover to rewrite, and renaming it would drop the
    /// definition they gave the argument it collides with.
    #[test]
    fn it_leaves_a_positional_looking_property_of_a_hand_written_schema_alone() {
        let mut tool = Tool::new("sum", |a: i32, b: i32| async move { a + b });
        tool.with_input_schema(|_| {
            const JSON: &str = r#"{
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "number" },
                    "arg0": { "type": "boolean" }
                }
            }"#;
            #[cfg(feature = "legacy-spec")]
            {
                ToolSchema::from_json_str(JSON)
            }
            #[cfg(not(feature = "legacy-spec"))]
            {
                crate::types::schema_2020::InputSchema::from_json_str(JSON).unwrap_or_default()
            }
        })
        .with_arg_names(["name", "age"]);

        assert_eq!(schema_props(&tool), ["age", "arg0", "name"]);

        // The caller's own definition of `name` must survive: renaming `arg0`
        // onto it would leave the boolean behind.
        #[cfg(feature = "legacy-spec")]
        {
            let props = tool.input_schema.properties.as_ref().unwrap();
            assert_eq!(props["name"].r#type, PropertyType::String);
            assert_eq!(props["arg0"].r#type, PropertyType::Bool);
        }
        #[cfg(not(feature = "legacy-spec"))]
        {
            let props = &tool.input_schema.as_value()["properties"];
            assert_eq!(props["name"]["type"], "string");
            assert_eq!(props["arg0"]["type"], "boolean");
        }
        assert!(tool.arg_name_conflict().is_none());
    }

    #[test]
    #[cfg(feature = "legacy-spec")]
    fn it_deserializes_input_schema() {
        let json = r#"{
            "properties": {
                "name": {
                    "type": "string",
                    "description": "A name to whom say hello"
                }
            }
        }"#;

        let schema: ToolSchema = serde_json::from_str(json).unwrap();

        assert_eq!(schema.r#type, PropertyType::Object);
        assert!(schema.properties.is_some());
    }

    #[cfg(feature = "legacy-spec")]
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[allow(dead_code)]
    struct MyT {
        name: String,
    }

    #[test]
    #[cfg(feature = "legacy-spec")]
    #[allow(deprecated)]
    fn from_schemars_matches_from_schema_legacy_name() {
        // The deprecated wrapper `from_schema_legacy` must delegate to
        // `from_schemars`, so their outputs for the same input schema
        // must be identical. The wrapper retains the old behaviour
        // (non-generic, takes a `schemars::Schema`) under a renamed
        // identifier -- see deviation note for why we did not keep the
        // exact `from_schema` name.
        let a = ToolSchema::from_schemars(schemars::schema_for!(MyT));
        let b = ToolSchema::from_schema_legacy(schemars::schema_for!(MyT));

        // ToolSchema does not derive PartialEq, so compare via
        // serde_json::Value canonicalisation.
        let av = serde_json::to_value(&a).unwrap();
        let bv = serde_json::to_value(&b).unwrap();
        assert_eq!(av, bv);
    }

    #[test]
    #[cfg(feature = "legacy-spec")]
    fn from_schema_generic_constructor_works() {
        let s: ToolSchema = ToolSchema::from_schema::<MyT>();
        let props = s.properties.expect("properties should be set");
        assert!(!props.is_empty(), "expected at least one property");
        assert!(props.contains_key("name"));
    }

    #[test]
    #[cfg(feature = "legacy-spec")]
    fn from_value_round_trip() {
        let original = ToolSchema::default().with_prop("name", "a name", PropertyType::String);
        let value = serde_json::to_value(&original).expect("serializes");
        let round_tripped = ToolSchema::from_value(value).expect("round trips");

        // Compare via Value since ToolSchema does not derive PartialEq.
        let a = serde_json::to_value(&original).expect("serializes original");
        let b = serde_json::to_value(&round_tripped).expect("serializes round trip");
        assert_eq!(a, b);
    }

    #[test]
    #[cfg(feature = "legacy-spec")]
    fn from_value_invalid_returns_error() {
        // A bare JSON string is not a valid ToolSchema (which expects
        // an object with a `type` discriminator). Deserialization must
        // fail, not panic.
        let result = ToolSchema::from_value(serde_json::Value::String("not a schema".into()));
        assert!(result.is_err(), "expected Err for non-object value");
    }

    /// `tools/list` is a `CacheableResult`, so both fields are mandatory and
    /// present even when the server expressed no opinion.
    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn list_tools_result_always_carries_cache_fields() {
        use crate::types::CacheScope;

        let v = serde_json::to_value(ListToolsResult::default()).unwrap();
        assert_eq!(v["ttlMs"], serde_json::json!(0));
        assert_eq!(v["cacheScope"], serde_json::json!("private"));

        let r = ListToolsResult {
            ttl_ms: 60_000,
            cache_scope: CacheScope::Public,
            ..Default::default()
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["ttlMs"], serde_json::json!(60_000));
        assert_eq!(v["cacheScope"], serde_json::json!("public"));

        // A peer that omits them still parses.
        let back: ListToolsResult =
            serde_json::from_value(serde_json::json!({ "tools": [] })).unwrap();
        assert_eq!(back.ttl_ms, 0);
        assert_eq!(back.cache_scope, CacheScope::Private);
    }
}
