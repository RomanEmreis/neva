//! JSON Schema 2020-12 backing type for tool input/output schemas.
//!
//! The MCP 2026-07-28 Release Candidate requires that tool `inputSchema`
//! and `outputSchema` fields carry full JSON Schema 2020-12 documents.
//! Unlike the legacy `ToolSchema` (a typed struct
//! mirroring a small Draft 7-ish subset), schemas under the new draft
//! must be representable as **arbitrary** JSON — including `oneOf`,
//! `anyOf`, `$ref`, `additionalProperties`, conditional `if`/`then`,
//! and any custom keywords the implementor chooses to embed.
//!
//! [`InputSchema`] is therefore a `#[serde(transparent)]` newtype around
//! [`serde_json::Value`]. The schema **is** the value — there is no
//! wrapper key — so it round-trips losslessly.
//!
//! This module is intentionally minimal in Task 3.1: it only introduces
//! the type. Wiring into [`crate::types::tool::Tool`] happens in later
//! tasks (3.2 alias, 3.4 field switch, 3.5 validator).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A JSON Schema 2020-12 document used as a tool input or output schema.
///
/// The inner [`serde_json::Value`] holds the full schema verbatim. This is
/// a transparent newtype, so [`InputSchema`] serializes and deserializes
/// exactly as if it were a raw [`Value`] — no wrapper key is added.
///
/// # Examples
///
/// ```
/// # use neva::types::schema_2020::InputSchema;
/// use serde_json::json;
///
/// let schema = InputSchema::from(json!({
///     "type": "object",
///     "properties": { "name": { "type": "string" } },
///     "required": ["name"]
/// }));
///
/// let wire = serde_json::to_value(&schema).expect("serializes");
/// assert_eq!(wire["type"], "object");
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(transparent)]
pub struct InputSchema(
    /// The underlying JSON Schema document.
    pub Value,
);

impl Default for InputSchema {
    /// Returns an empty object schema: `{"type": "object", "properties": {}}`.
    ///
    /// This mirrors the legacy `ToolSchema::default`
    /// shape — an *empty object schema*, not a totally empty JSON object —
    /// so the wire format remains compatible with existing clients that
    /// expect at minimum a `type` discriminator.
    #[inline]
    fn default() -> Self {
        Self(json!({
            "type": "object",
            "properties": {}
        }))
    }
}

impl InputSchema {
    /// Creates a new empty object schema.
    ///
    /// Equivalent to [`InputSchema::default`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::schema_2020::InputSchema;
    /// let schema = InputSchema::new();
    /// assert_eq!(schema, InputSchema::default());
    /// ```
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrows the underlying [`serde_json::Value`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::schema_2020::InputSchema;
    /// let schema = InputSchema::default();
    /// assert_eq!(schema.as_value()["type"], "object");
    /// ```
    #[inline]
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Consumes the [`InputSchema`] and returns the underlying [`serde_json::Value`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::schema_2020::InputSchema;
    /// let schema = InputSchema::default();
    /// let value = schema.into_value();
    /// assert_eq!(value["type"], "object");
    /// ```
    #[inline]
    pub fn into_value(self) -> Value {
        self.0
    }
}

#[cfg(feature = "server")]
impl InputSchema {
    /// Wraps an arbitrary [`serde_json::Value`] as an [`InputSchema`].
    ///
    /// The value is stored verbatim — no validation is performed at this
    /// point. Validation of incoming tool arguments against the schema
    /// happens elsewhere (see `validate_call_args` in the `tool` module).
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::schema_2020::InputSchema;
    /// use serde_json::json;
    ///
    /// let schema = InputSchema::from_value(json!({ "type": "string" }));
    /// assert_eq!(schema.as_value()["type"], "string");
    /// ```
    #[inline]
    pub fn from_value(value: Value) -> Self {
        Self(value)
    }

    /// Parses an [`InputSchema`] from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error`] when the input is not valid JSON.
    /// Unlike the legacy `ToolSchema::from_json_str`,
    /// this constructor never panics — library code must propagate errors
    /// rather than expect, per project conventions.
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::schema_2020::InputSchema;
    /// let schema = InputSchema::from_json_str(r#"{"type":"object"}"#)
    ///     .expect("valid JSON");
    /// assert_eq!(schema.as_value()["type"], "object");
    /// ```
    #[inline]
    pub fn from_json_str(json: &str) -> Result<Self, crate::error::Error> {
        let value: Value = serde_json::from_str(json)?;
        Ok(Self(value))
    }

    /// Creates an [`InputSchema`] from a type that implements
    /// [`schemars::JsonSchema`].
    ///
    /// Uses [`schemars::schema_for!`] under the hood and stores the
    /// generated schema as a raw [`serde_json::Value`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use neva::types::schema_2020::InputSchema;
    /// use schemars::JsonSchema;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize, JsonSchema)]
    /// struct Args { name: String }
    ///
    /// let schema = InputSchema::from_schema::<Args>();
    /// assert!(schema.as_value().is_object());
    /// ```
    #[inline]
    pub fn from_schema<T: schemars::JsonSchema>() -> Self {
        let json_schema = schemars::schema_for!(T);
        Self::from_schemars(json_schema)
    }

    /// Converts an already-built [`schemars::Schema`] into an
    /// [`InputSchema`] by extracting its underlying [`serde_json::Value`].
    ///
    /// Useful when callers construct a [`schemars::Schema`] by hand
    /// (or via a `SchemaSettings` builder) and want to attach it to a
    /// tool without going through the `schema_for!` macro.
    #[inline]
    pub fn from_schemars(schema: schemars::Schema) -> Self {
        Self(schema.to_value())
    }
}

impl From<Value> for InputSchema {
    /// Wraps any [`serde_json::Value`] as an [`InputSchema`]. The value
    /// is stored verbatim.
    #[inline]
    fn from(value: Value) -> Self {
        Self(value)
    }
}

#[cfg(feature = "server")]
impl From<schemars::Schema> for InputSchema {
    /// Converts a [`schemars::Schema`] into an [`InputSchema`] by taking
    /// its underlying JSON [`Value`].
    #[inline]
    fn from(schema: schemars::Schema) -> Self {
        Self(schema.to_value())
    }
}

// --- `#[tool]` macro support (RC only) -------------------------------------
//
// These items back the `#[tool]` macro's JSON Schema 2020-12 emission under
// `proto-2026-07-28-rc`. The `schema_2020` module itself is always compiled
// (the `InputSchema` type exists on every config), so the macro-support items
// are individually gated on the RC flag — they are unused on the legacy path.

/// Autoref-specialization probe used by the `#[tool]` macro to build a
/// per-argument JSON Schema 2020-12 subschema.
///
/// Resolves to a `schemars`-derived (inlined, self-contained) subschema when
/// the argument type implements [`schemars::JsonSchema`], and to an opaque
/// `{"type":"object"}` otherwise — chosen at the call site without requiring a
/// trait bound the macro cannot enforce.
#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
#[derive(Debug)]
pub struct SchemaProbe<T>(std::marker::PhantomData<T>);

#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
impl<T> SchemaProbe<T> {
    /// Creates a new probe.
    #[inline]
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
impl<T> Default for SchemaProbe<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Preferred specialization: exists only when `T: JsonSchema`.
#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
pub trait ViaJsonSchema {
    /// Builds an inlined, self-contained subschema for `T`.
    fn neva_subschema(&self) -> Value;
}

#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
impl<T: schemars::JsonSchema> ViaJsonSchema for &SchemaProbe<T> {
    #[inline]
    fn neva_subschema(&self) -> Value {
        let settings =
            schemars::generate::SchemaSettings::draft2020_12().with(|s| s.inline_subschemas = true);
        schemars::SchemaGenerator::new(settings)
            .into_root_schema_for::<T>()
            .to_value()
    }
}

/// Fallback specialization: available for any `T`.
#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
pub trait ViaFallback {
    /// Builds an opaque object subschema.
    fn neva_subschema(&self) -> Value;
}

#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
impl<T> ViaFallback for SchemaProbe<T> {
    #[inline]
    fn neva_subschema(&self) -> Value {
        json!({ "type": "object" })
    }
}

/// Builds an inline primitive subschema `{"type": ty}` for a tool argument.
/// Used by the `#[tool]` macro for arguments mapping to a JSON primitive,
/// keeping `serde_json` entirely inside neva (generated code never names it).
#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
#[inline]
pub fn primitive_subschema(ty: &str) -> Value {
    json!({ "type": ty })
}

/// Assembles a JSON Schema 2020-12 object schema from `(name, subschema)`
/// property pairs and a list of required property names. Used by the `#[tool]`
/// macro so generated code only ever names `neva::` paths. Uses the
/// unconditional `From<Value>` (no `server` feature required).
#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
pub fn object_schema(properties: Vec<(String, Value)>, required: Vec<String>) -> InputSchema {
    let mut props = serde_json::Map::with_capacity(properties.len());
    for (name, schema) in properties {
        props.insert(name, schema);
    }
    let mut root = serde_json::Map::with_capacity(3);
    root.insert("type".to_string(), Value::String("object".to_string()));
    root.insert("properties".to_string(), Value::Object(props));
    root.insert(
        "required".to_string(),
        Value::Array(required.into_iter().map(Value::String).collect()),
    );
    InputSchema::from(Value::Object(root))
}

/// Builds a JSON Schema 2020-12 subschema for a type, used by the generated
/// `#[tool]` registration code. Yields a `schemars`-derived inlined schema when
/// the type implements [`schemars::JsonSchema`], otherwise `{"type":"object"}`.
///
/// The `&&` double-reference makes the `ViaJsonSchema for &SchemaProbe<T>`
/// candidate be tried first; when its `T: JsonSchema` bound is unmet it is not a
/// candidate and method resolution falls through to `ViaFallback`.
#[cfg(feature = "proto-2026-07-28-rc")]
#[doc(hidden)]
#[macro_export]
macro_rules! __tool_arg_subschema {
    ($t:ty) => {{
        #[allow(unused_imports)]
        use $crate::__macro_support::{ViaFallback as _, ViaJsonSchema as _};
        (&&$crate::__macro_support::SchemaProbe::<$t>::new()).neva_subschema()
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_is_empty_object_schema() {
        let schema = InputSchema::default();
        let value = serde_json::to_value(&schema).expect("serializes");
        assert_eq!(
            value,
            json!({
                "type": "object",
                "properties": {}
            })
        );

        // And the round-trip back from JSON yields the same value.
        let s = serde_json::to_string(&schema).expect("to_string");
        let parsed: InputSchema = serde_json::from_str(&s).expect("from_str");
        assert_eq!(parsed, schema);
    }

    #[test]
    fn new_equals_default() {
        assert_eq!(InputSchema::new(), InputSchema::default());
    }

    #[test]
    fn transparent_serde_has_no_wrapper_key() {
        // Serializing must not introduce a wrapper field like `{"0": ...}`
        // or `{"value": ...}` — the schema IS the value.
        let raw = json!({ "type": "string", "minLength": 1 });
        let schema = InputSchema(raw.clone());
        let serialized = serde_json::to_value(&schema).expect("serializes");
        assert_eq!(serialized, raw);

        // Deserialization is symmetric.
        let from_json: InputSchema = serde_json::from_value(raw.clone()).expect("from_value");
        assert_eq!(from_json.as_value(), &raw);
    }

    #[test]
    fn as_value_and_into_value_yield_inner() {
        let raw = json!({ "type": "boolean" });
        let schema = InputSchema(raw.clone());
        assert_eq!(schema.as_value(), &raw);
        assert_eq!(schema.into_value(), raw);
    }

    #[cfg(feature = "server")]
    #[test]
    fn from_json_str_invalid_returns_error() {
        // Malformed JSON must surface as an Err, never a panic.
        let result = InputSchema::from_json_str("{not valid json");
        assert!(result.is_err(), "expected Err for malformed JSON");
    }

    #[cfg(feature = "server")]
    #[test]
    fn from_json_str_valid_round_trips() {
        let input = r#"{"type":"object","properties":{"x":{"type":"integer"}},"required":["x"]}"#;
        let schema = InputSchema::from_json_str(input).expect("valid JSON parses");

        // Re-serializing through serde_json::to_string yields a string that
        // deserializes back to the same Value (byte-exactness is not
        // guaranteed across serde_json — key order in maps is preserved by
        // default but we compare as Value to be canonical).
        let re_serialized = serde_json::to_string(&schema).expect("serializes");
        let reparsed: Value = serde_json::from_str(&re_serialized).expect("reparses");
        let expected: Value = serde_json::from_str(input).expect("expected parses");
        assert_eq!(reparsed, expected);
    }

    #[cfg(feature = "server")]
    #[test]
    fn from_value_preserves_arbitrary_keys() {
        // The whole point of Value-shaped schemas: full JSON Schema 2020-12
        // expressivity. `oneOf`, `$ref`, `additionalProperties`, custom
        // keywords — all must round-trip verbatim.
        let raw = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://example.com/widget.schema.json",
            "title": "Widget",
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "kind": { "$ref": "#/$defs/kind" }
            },
            "required": ["id"],
            "additionalProperties": false,
            "oneOf": [
                { "required": ["id"] },
                { "required": ["kind"] }
            ],
            "$defs": {
                "kind": { "enum": ["a", "b", "c"] }
            },
            "x-custom-vendor-keyword": { "anything": [1, 2, 3] }
        });

        let schema = InputSchema::from_value(raw.clone());
        assert_eq!(schema.as_value(), &raw);

        // And serializing then deserializing keeps every key.
        let s = serde_json::to_string(&schema).expect("serializes");
        let back: InputSchema = serde_json::from_str(&s).expect("deserializes");
        assert_eq!(back.as_value(), &raw);
    }

    #[cfg(feature = "server")]
    #[test]
    fn from_impl_for_value_wraps_inner() {
        let raw = json!({ "type": "null" });
        let schema: InputSchema = raw.clone().into();
        assert_eq!(schema.as_value(), &raw);
    }

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[test]
    fn probe_uses_schemars_when_type_derives_json_schema() {
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Point {
            x: i32,
            y: i32,
        }

        let v = crate::__tool_arg_subschema!(Point);
        assert_eq!(v["type"], json!("object"));
        assert!(v["properties"].is_object(), "expected rich properties");
        assert!(v["properties"]["x"].is_object());
        // Inlined: a self-contained subschema must not carry `$defs`.
        assert!(v.get("$defs").is_none(), "subschema must be inlined (no $defs)");
    }

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[test]
    fn probe_falls_back_for_type_without_json_schema() {
        struct Opaque;
        let v = crate::__tool_arg_subschema!(Opaque);
        assert_eq!(v, json!({ "type": "object" }));
    }

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[test]
    fn probe_inlines_nested_struct_without_refs() {
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Inner {
            label: String,
        }
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Outer {
            inner: Inner,
        }

        let v = crate::__tool_arg_subschema!(Outer);
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("$ref"), "inlined schema must not contain $ref: {s}");
        assert!(!s.contains("$defs"), "inlined schema must not contain $defs: {s}");
        assert!(v["properties"]["inner"]["properties"]["label"].is_object());
    }

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[test]
    fn object_schema_assembles_properties_and_required() {
        use crate::__macro_support::{object_schema, primitive_subschema};
        let s = object_schema(
            vec![
                ("a".to_string(), primitive_subschema("number")),
                ("b".to_string(), primitive_subschema("string")),
            ],
            vec!["a".to_string(), "b".to_string()],
        );
        let v = s.into_value();
        assert_eq!(v["type"], json!("object"));
        assert_eq!(v["properties"]["a"]["type"], json!("number"));
        assert_eq!(v["properties"]["b"]["type"], json!("string"));
        let mut req: Vec<String> = serde_json::from_value(v["required"].clone()).unwrap();
        req.sort();
        assert_eq!(req, vec!["a".to_string(), "b".to_string()]);
    }
}
