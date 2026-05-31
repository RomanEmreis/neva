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
    crate::types::{FromRequest, IntoResponse, Page, Request, RequestId, Response},
    crate::{
        Context,
        app::handler::{FromHandlerParams, GenericHandler, Handler, HandlerParams, RequestHandler},
    },
    futures_util::future::BoxFuture,
    std::{future::Future, sync::Arc},
};

#[cfg(all(feature = "server", not(feature = "proto-2026-07-28-rc")))]
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

    /// Intended for UI and end-user contexts — optimized to be human-readable and easily understood,
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
    /// alias: the legacy typed `ToolSchema` under the default feature set,
    /// or [`crate::types::schema_2020::InputSchema`] (a Value-shaped JSON
    /// Schema 2020-12 wrapper) under the `proto-2026-07-28-rc` feature.
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

    /// Suggested TTL in milliseconds for caching this list result, when set by the server.
    #[cfg(feature = "proto-2026-07-28-rc")]
    #[serde(rename = "ttlMs", skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,

    /// Suggested cache scope for this list result, when set by the server.
    #[cfg(feature = "proto-2026-07-28-rc")]
    #[serde(rename = "cacheScope", skip_serializing_if = "Option::is_none")]
    pub cache_scope: Option<crate::types::CacheScope>,
}

/// Used by the client to invoke a tool provided by the server.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolRequestParams {
    /// Tool name.
    pub name: String,

    /// Optional arguments to pass to the tool.
    #[serde(rename = "arguments")]
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
#[cfg(not(feature = "proto-2026-07-28-rc"))]
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
    #[cfg_attr(not(feature = "proto-2026-07-28-rc"), allow(clippy::needless_update))]
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
    #[cfg_attr(not(feature = "proto-2026-07-28-rc"), allow(clippy::needless_update))]
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

#[cfg(not(feature = "proto-2026-07-28-rc"))]
impl Default for ToolSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::Object,
            properties: Some(HashMap::new()),
            required: None,
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

#[cfg(all(feature = "server", not(feature = "proto-2026-07-28-rc")))]
impl ToolSchema {
    /// Creates a new [`ToolSchema`] object
    #[inline]
    pub(crate) fn new(props: Option<HashMap<String, SchemaProperty>>) -> Self {
        Self {
            r#type: PropertyType::Object,
            properties: props,
            required: None,
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
    /// is infallible because the RC schema type is a transparent
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
    /// — `with_schema` is a chainable instance method, while `from_schema` is
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
    /// Returns a tool arguments schema
    #[inline]
    fn args() -> Option<HashMap<String, SchemaProperty>> {
        None
    }
}

#[cfg(feature = "server")]
pub(crate) struct ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: TryFrom<CallToolRequestParams, Error = Error>,
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

#[cfg(feature = "server")]
impl<F, R, Args> ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: TryFrom<CallToolRequestParams, Error = Error>,
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
    Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync,
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<'_, Result<CallToolResponse, Error>> {
        let HandlerParams::Tool(params) = params else {
            unreachable!()
        };
        Box::pin(async move {
            let args = Args::try_from(params)?;
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

/// Builds a [`crate::types::ToolInputSchema`] from the typed argument map
/// produced by [`ToolHandler::args`].
///
/// Under the default feature set this returns the typed legacy
/// `ToolSchema` verbatim. Under `proto-2026-07-28-rc` the legacy
/// `ToolSchema` struct is absent, so this constructs an
/// [`crate::types::schema_2020::InputSchema`] directly from the
/// `Option<HashMap<String, SchemaProperty>>` by serializing each
/// [`SchemaProperty`] into a `serde_json::Value` and wrapping the result
/// as a JSON Schema 2020-12 object schema. The same call site at
/// [`Tool::new`] compiles under either feature set.
#[cfg(feature = "server")]
#[inline]
fn build_input_schema_from_args(
    args: Option<HashMap<String, SchemaProperty>>,
) -> crate::types::ToolInputSchema {
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    {
        ToolSchema::new(args)
    }
    #[cfg(feature = "proto-2026-07-28-rc")]
    {
        use serde_json::{Map, Value, json};
        let properties = args
            .as_ref()
            .map(|m| {
                let mut obj = Map::with_capacity(m.len());
                for (k, v) in m {
                    let v_json =
                        serde_json::to_value(v).unwrap_or_else(|_| Value::Object(Map::new()));
                    obj.insert(k.clone(), v_json);
                }
                Value::Object(obj)
            })
            .unwrap_or_else(|| Value::Object(Map::new()));
        let value = json!({ "type": "object", "properties": properties });
        crate::types::schema_2020::InputSchema::from(value)
    }
}

#[cfg(feature = "server")]
impl Tool {
    /// Initializes a new [`Tool`]
    pub fn new<F, Args, R>(name: impl Into<String>, handler: F) -> Self
    where
        F: ToolHandler<Args, Output = R>,
        R: Into<CallToolResponse> + Send + 'static,
        Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync + 'static,
    {
        let handler = ToolFunc::new(handler);
        let input_schema = build_input_schema_from_args(F::args());
        Self {
            name: name.into(),
            title: None,
            descr: None,
            input_schema,
            output_schema: None,
            meta: None,
            annotations: None,
            handler: Some(handler),
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
    /// Under the default feature set this is the typed `ToolSchema`
    /// (with builder methods like `with_prop`/`with_required`); under
    /// `proto-2026-07-28-rc` it is
    /// [`crate::types::schema_2020::InputSchema`] (a Value-shaped JSON
    /// Schema 2020-12 wrapper). The schema model differs between flags,
    /// so closure bodies that rely on the typed builder API are RC-incompatible
    /// by design.
    pub fn with_input_schema<F>(&mut self, config: F) -> &mut Self
    where
        F: FnOnce(crate::types::ToolInputSchema) -> crate::types::ToolInputSchema,
    {
        self.input_schema = config(Default::default());
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

    /// Invoke a tool
    #[inline]
    pub(crate) async fn call(&self, params: HandlerParams) -> Result<CallToolResponse, Error> {
        match self.handler {
            Some(ref handler) => handler.call(params).await,
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
    /// `proto-2026-07-28-rc` the schema is already a [`serde_json::Value`]
    /// (wrapped by [`crate::types::schema_2020::InputSchema`]), so we borrow
    /// it directly via [`crate::types::schema_2020::InputSchema::as_value`]
    /// — no re-serialization is needed.
    pub fn validate<'a>(&self, resp: &'a CallToolResponse) -> Result<&'a CallToolResponse, Error> {
        let Some(schema_ref) = self.output_schema.as_ref() else {
            return Err(Error::new(
                ErrorCode::ParseError,
                "Tool: Output schema not specified",
            ));
        };

        #[cfg(not(feature = "proto-2026-07-28-rc"))]
        let schema = serde_json::to_value(schema_ref).map_err(Into::<Error>::into)?;
        #[cfg(feature = "proto-2026-07-28-rc")]
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
        fn args() -> Option<HashMap<String, SchemaProperty>> {
            let mut args = HashMap::new();
            $(
            {
                let prop = SchemaProperty::new::<$param>();
                if prop.r#type != PropertyType::None {
                    args.insert(
                        prop.r#type.to_string(),
                        prop
                    );
                }
            };
            )*
            if args.len() == 0 {
                None
            } else {
                Some(args)
            }
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

    #[tokio::test]
    async fn it_creates_and_calls_tool() {
        let tool = Tool::new("sum", |a: i32, b: i32| async move { a + b });

        let params = CallToolRequestParams {
            name: "sum".into(),
            meta: None,
            #[cfg(feature = "tasks")]
            task: None,
            args: Some(HashMap::from([
                ("a".into(), serde_json::to_value(5).unwrap()),
                ("b".into(), serde_json::to_value(2).unwrap()),
            ])),
        };

        let resp = tool.call(params.into()).await.unwrap();
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(
            json,
            r#"{"content":[{"type":"text","text":"7"}],"isError":false}"#
        );
    }

    #[test]
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
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

    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[allow(dead_code)]
    struct MyT {
        name: String,
    }

    #[test]
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    #[allow(deprecated)]
    fn from_schemars_matches_from_schema_legacy_name() {
        // The deprecated wrapper `from_schema_legacy` must delegate to
        // `from_schemars`, so their outputs for the same input schema
        // must be identical. The wrapper retains the old behaviour
        // (non-generic, takes a `schemars::Schema`) under a renamed
        // identifier — see deviation note for why we did not keep the
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
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    fn from_schema_generic_constructor_works() {
        let s: ToolSchema = ToolSchema::from_schema::<MyT>();
        let props = s.properties.expect("properties should be set");
        assert!(!props.is_empty(), "expected at least one property");
        assert!(props.contains_key("name"));
    }

    #[test]
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
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
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    fn from_value_invalid_returns_error() {
        // A bare JSON string is not a valid ToolSchema (which expects
        // an object with a `type` discriminator). Deserialization must
        // fail, not panic.
        let result = ToolSchema::from_value(serde_json::Value::String("not a schema".into()));
        assert!(result.is_err(), "expected Err for non-object value");
    }

    #[test]
    #[cfg(feature = "proto-2026-07-28-rc")]
    fn list_tools_result_serializes_cache_hints() {
        use crate::types::CacheScope;
        let r = ListToolsResult {
            tools: vec![],
            next_cursor: None,
            ttl_ms: Some(60_000),
            cache_scope: Some(CacheScope::Session),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["ttlMs"], serde_json::json!(60_000));
        assert_eq!(v["cacheScope"], serde_json::json!("session"));

        let back: ListToolsResult = serde_json::from_value(v).unwrap();
        assert_eq!(back.ttl_ms, Some(60_000));
        assert_eq!(back.cache_scope, Some(CacheScope::Session));
    }

    #[test]
    #[cfg(feature = "proto-2026-07-28-rc")]
    fn list_tools_result_omits_cache_hints_when_none() {
        let r = ListToolsResult::default();
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("ttlMs").is_none());
        assert!(v.get("cacheScope").is_none());
    }
}
