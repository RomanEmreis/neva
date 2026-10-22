//! JSON Schema 2020-12 backing type for tool input/output schemas.
//!
//! The MCP 2026-07-28 Release Candidate requires that tool `inputSchema`
//! and `outputSchema` fields carry full JSON Schema 2020-12 documents.
//! Unlike the legacy `ToolSchema` (a typed struct
//! mirroring a small Draft 7-ish subset), schemas under the new draft
//! must be representable as **arbitrary** JSON -- including `oneOf`,
//! `anyOf`, `$ref`, `additionalProperties`, conditional `if`/`then`,
//! and any custom keywords the implementor chooses to embed.
//!
//! [`InputSchema`] is therefore a `#[serde(transparent)]` newtype around
//! [`serde_json::Value`]. The schema **is** the value -- there is no
//! wrapper key -- so it round-trips losslessly.
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
/// exactly as if it were a raw [`Value`] -- no wrapper key is added.
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
    ///
    /// Kept `pub(crate)` so the type stays closed to arbitrary external
    /// mutation -- construct via [`InputSchema::from_value`] and read via
    /// [`InputSchema::as_value`]/[`InputSchema::into_value`]. This leaves room
    /// to attach a lazily-compiled validator beside the schema later without a
    /// breaking change.
    pub(crate) Value,
);

impl Default for InputSchema {
    /// Returns an empty object schema: `{"type": "object", "properties": {}}`.
    ///
    /// This mirrors the legacy `ToolSchema::default`
    /// shape -- an *empty object schema*, not a totally empty JSON object --
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
    /// The value is stored verbatim -- no validation is performed at this
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
        Self(value).with_object_type()
    }

    /// Parses an [`InputSchema`] from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error`] when the input is not valid JSON.
    /// Unlike the legacy `ToolSchema::from_json_str`,
    /// this constructor never panics -- library code must propagate errors
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
        Ok(Self(value).with_object_type())
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
        Self(schema.to_value()).with_object_type()
    }
}

impl From<Value> for InputSchema {
    /// Wraps any [`serde_json::Value`] as an [`InputSchema`], supplying the
    /// `"type": "object"` discriminator MCP requires when the document is an
    /// object schema that omitted it. An explicit `type` is left as written,
    /// and a boolean schema has no key to add.
    #[inline]
    fn from(value: Value) -> Self {
        Self(value).with_object_type()
    }
}

impl InputSchema {
    /// Supplies the `"type": "object"` discriminator when the document is an
    /// object schema that left it out.
    ///
    /// MCP requires a tool's `inputSchema` and `outputSchema` to be *object*
    /// schemas, and a conforming client validates that: a hand-written schema
    /// that lists `properties` but never says `"type": "object"` makes the
    /// whole `tools/list` result fail validation, taking the tools that were
    /// fine down with it. Rather than publish a document neva's own docs call
    /// invalid, the discriminator every such schema meant to carry is filled
    /// in.
    ///
    /// An explicit `type` is left alone, whatever it says -- this fills a gap,
    /// it does not overrule the author -- and a boolean schema (`true` /
    /// `false`) is not an object to add a key to.
    #[inline]
    fn with_object_type(mut self) -> Self {
        if let Value::Object(schema) = &mut self.0
            && !schema.contains_key("type")
        {
            schema.insert("type".to_string(), Value::String("object".to_string()));
        }
        self
    }
}

#[cfg(feature = "server")]
impl From<schemars::Schema> for InputSchema {
    /// Converts a [`schemars::Schema`] into an [`InputSchema`] by taking
    /// its underlying JSON [`Value`].
    #[inline]
    fn from(schema: schemars::Schema) -> Self {
        Self(schema.to_value()).with_object_type()
    }
}

// --- `#[tool]` macro support (2026-07-28 only) -------------------------------------
//
// These items back the `#[tool]` macro's JSON Schema 2020-12 emission under
// MCP 2026-07-28. The `schema_2020` module itself is always compiled
// (the `InputSchema` type exists on every config), so the macro-support items
// are individually gated on `not(feature = "legacy-spec")` -- they are unused
// on the legacy path.

/// Autoref-specialization probe used by the `#[tool]` macro to build a
/// per-argument JSON Schema 2020-12 subschema.
///
/// Resolves to a `schemars`-derived (inlined, self-contained) subschema when
/// the argument type implements [`schemars::JsonSchema`], and to an opaque
/// `{"type":"object"}` otherwise -- chosen at the call site without requiring a
/// trait bound the macro cannot enforce.
#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
#[derive(Debug)]
pub struct SchemaProbe<T>(std::marker::PhantomData<T>);

#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
impl<T> SchemaProbe<T> {
    /// Creates a new probe.
    #[inline]
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
impl<T> Default for SchemaProbe<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Preferred specialization: exists only when `T: JsonSchema`.
#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
pub trait ViaJsonSchema {
    /// Builds an inlined, self-contained subschema for `T`.
    fn neva_subschema(&self) -> Value;
}

#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
impl<T: schemars::JsonSchema> ViaJsonSchema for &SchemaProbe<T> {
    /// The subschema lands in `properties`, so it carries none of the keywords
    /// schemars gives a root schema. `$schema` must not appear in a schema that
    /// is not a resource root (JSON Schema 2020-12 Core section 8.1.1), and the
    /// `title` a root is given by default is only the type's schemars name,
    /// such as `Array_of_Entity`. A title the type sets itself, through
    /// `#[schemars(title = "...")]`, stays, unless it is that same name.
    ///
    /// It is generated as a root rather than with `subschema_for`, which would
    /// point a recursive type at a `$defs` entry the subschema does not carry.
    /// A recursive type keeps its `$defs`, and a root that refers to itself is
    /// given an entry there too; [`object_schema`] hoists them into the tool's
    /// root.
    #[inline]
    fn neva_subschema(&self) -> Value {
        let settings = schemars::generate::SchemaSettings::draft2020_12().with(|s| {
            s.inline_subschemas = true;
            s.meta_schema = None;
        });

        let mut schema = schemars::SchemaGenerator::new(settings)
            .into_root_schema_for::<T>()
            .to_value();

        if let Value::Object(object) = &mut schema
            && object.get("title").and_then(Value::as_str) == Some(T::schema_name().as_ref())
        {
            object.remove("title");
        }

        define_self_reference(&mut schema, &T::schema_name());
        schema
    }
}

/// Fallback specialization: available for any `T`.
#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
pub trait ViaFallback {
    /// Builds an opaque object subschema.
    fn neva_subschema(&self) -> Value;
}

#[cfg(not(feature = "legacy-spec"))]
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
#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
#[inline]
pub fn primitive_subschema(ty: &str) -> Value {
    json!({ "type": ty })
}

/// Assembles a JSON Schema 2020-12 object schema from `(name, subschema)`
/// property pairs and a list of required property names. Used by the `#[tool]`
/// macro so generated code only ever names `neva::` paths. Uses the
/// unconditional `From<Value>` (no `server` feature required).
///
/// A tool whose arguments are all optional gets no `required` key at all
/// rather than an empty array, matching what the non-macro path publishes.
///
/// A subschema's `$ref`s are written against its own root, which is not where
/// it lands. Its `$defs` move to the tool's root, where `#/$defs/<Name>`
/// resolves, and an entry whose name another argument already took is renamed
/// unless both arguments brought the same definitions. Any other reference
/// into the subschema is rebased onto `#/properties/<arg>`.
#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
pub fn object_schema(properties: Vec<(String, Value)>, required: Vec<String>) -> InputSchema {
    let mut props = serde_json::Map::with_capacity(properties.len());
    let mut defs = serde_json::Map::new();
    for (name, mut schema) in properties {
        hoist_defs(&name, &mut schema, &mut defs);
        props.insert(name, schema);
    }

    let mut root = serde_json::Map::with_capacity(4);

    root.insert("type".to_string(), Value::String("object".to_string()));
    root.insert("properties".to_string(), Value::Object(props));

    if !required.is_empty() {
        root.insert(
            "required".to_string(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }
    if !defs.is_empty() {
        root.insert("$defs".to_string(), Value::Object(defs));
    }

    InputSchema::from(Value::Object(root))
}

/// Gives a root schema that refers to itself with `#` an entry of its own in
/// its `$defs`, under `name`, and points those references at it.
///
/// schemars refers to the root type with `#`. Once the schema is a property,
/// `#` is the tool's whole `inputSchema`, so it would describe the wrong
/// thing. Named, the root takes the shape schemars gives every other recursive
/// type: inlined where it is used, and defined under `$defs`.
#[cfg(not(feature = "legacy-spec"))]
fn define_self_reference(schema: &mut Value, name: &str) {
    let mut refers_to_itself = false;
    for_each_ref(schema, &mut |reference| {
        refers_to_itself |= *reference == "#"
    });

    if !refers_to_itself {
        return;
    }

    let mut defs = take_defs(schema);
    let name = unique_name(name, |taken| defs.contains_key(taken));
    let target = def_ref(&name);
    let mut retarget = |reference: &mut String| {
        if *reference == "#" {
            reference.clone_from(&target);
        }
    };

    for_each_ref(schema, &mut retarget);
    defs.values_mut()
        .for_each(|def| for_each_ref(def, &mut retarget));

    defs.insert(name, schema.clone());
    if let Value::Object(root) = schema {
        root.insert("$defs".to_string(), Value::Object(defs));
    }
}

/// Moves the `$defs` of the subschema of argument `arg` into `defs`, the
/// tool's root definitions, and rewrites the subschema's references to match.
#[cfg(not(feature = "legacy-spec"))]
fn hoist_defs(arg: &str, schema: &mut Value, defs: &mut serde_json::Map<String, Value>) {
    let mut own = take_defs(schema);

    // Clashing names are kept only when every clash is the same definition:
    // those are closed under their references, so whatever they point at is
    // at the root already. Otherwise each clashing name is renamed.
    let merge = own
        .iter()
        .all(|(name, def)| defs.get(name).is_none_or(|taken| taken == def));

    let mut renames: Vec<(String, String)> = Vec::new();

    if !merge {
        for name in own.keys().filter(|name| defs.contains_key(*name)) {
            let fresh = unique_name(name, |taken| {
                defs.contains_key(taken)
                    || own.contains_key(taken)
                    || renames.iter().any(|(_, new)| new == taken)
            });
            renames.push((name.clone(), fresh));
        }
    }

    let moved: Vec<(String, String)> = renames
        .iter()
        .map(|(old, new)| (def_ref(old), def_ref(new)))
        .collect();

    let property = format!("#/properties/{}", encode_segment(arg));
    let mut rebase = |reference: &mut String| {
        if let Some((old, new)) = moved.iter().find(|(old, _)| {
            reference
                .strip_prefix(old.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        }) {
            reference.replace_range(..old.len(), new);
        } else if let Some(fragment) = reference.strip_prefix('#')
            && !fragment.starts_with("/$defs/")
        {
            *reference = format!("{property}{fragment}");
        }
    };

    for_each_ref(schema, &mut rebase);
    own.values_mut()
        .for_each(|def| for_each_ref(def, &mut rebase));

    defs.extend(own.into_iter().map(|(name, def)| {
        let name = renames
            .iter()
            .find(|(old, _)| *old == name)
            .map_or(name, |(_, new)| new.clone());
        (name, def)
    }));
}

/// Removes the `$defs` at the root of `schema`, returning them.
#[cfg(not(feature = "legacy-spec"))]
fn take_defs(schema: &mut Value) -> serde_json::Map<String, Value> {
    match schema {
        Value::Object(root) => match root.remove("$defs") {
            Some(Value::Object(defs)) => defs,
            _ => serde_json::Map::new(),
        },
        _ => serde_json::Map::new(),
    }
}

/// Calls `f` on every `$ref` in `schema`, at any depth.
///
/// The members of `properties`, `$defs` and the like are names, not keywords,
/// and `const`, `default`, `enum` and `examples` hold instances, not schemas,
/// so a `$ref` inside an instance is data and is left alone.
#[cfg(not(feature = "legacy-spec"))]
fn for_each_ref(schema: &mut Value, f: &mut impl FnMut(&mut String)) {
    match schema {
        Value::Object(object) => {
            for (keyword, value) in object.iter_mut() {
                match (keyword.as_str(), value) {
                    ("$ref", Value::String(reference)) => f(reference),
                    ("const" | "default" | "enum" | "examples", _) => {}
                    (
                        "properties" | "patternProperties" | "$defs" | "definitions"
                        | "dependentSchemas",
                        Value::Object(named),
                    ) => named
                        .values_mut()
                        .for_each(|schema| for_each_ref(schema, f)),
                    (_, value) => for_each_ref(value, f),
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|schema| for_each_ref(schema, f)),
        _ => {}
    }
}

/// `base`, or `base` followed by the first number from 2 that makes it a name
/// not `taken`, the way schemars tells apart two types of the same name.
#[cfg(not(feature = "legacy-spec"))]
fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }

    (2u64..)
        .map(|n| format!("{base}{n}"))
        .find(|name| !taken(name))
        .unwrap_or_default()
}

/// The reference to the root `$defs` entry `name`.
#[cfg(not(feature = "legacy-spec"))]
fn def_ref(name: &str) -> String {
    format!("#/$defs/{}", encode_segment(name))
}

/// `name` as a JSON Pointer segment in a URI fragment, encoded the way
/// schemars encodes the references it writes: `~` and `/` escaped (RFC 6901),
/// and a byte a fragment cannot carry percent-encoded (RFC 3986).
#[cfg(not(feature = "legacy-spec"))]
fn encode_segment(name: &str) -> String {
    use std::fmt::Write as _;

    name.bytes()
        .fold(String::with_capacity(name.len()), |mut out, byte| {
            match byte {
                b'~' => out.push_str("~0"),
                b'/' => out.push_str("~1"),
                b'!' | b'$' | b'&'..=b';' | b'=' | b'?'..=b'Z' | b'_' | b'a'..=b'z' => {
                    out.push(char::from(byte))
                }
                _ => {
                    let _ = write!(out, "%{byte:02X}");
                }
            }
            out
        })
}

/// Builds a JSON Schema 2020-12 subschema for a type, used by the generated
/// `#[tool]` registration code. Yields a `schemars`-derived inlined schema when
/// the type implements [`schemars::JsonSchema`], otherwise `{"type":"object"}`.
///
/// The `&&` double-reference makes the `ViaJsonSchema for &SchemaProbe<T>`
/// candidate be tried first; when its `T: JsonSchema` bound is unmet it is not a
/// candidate and method resolution falls through to `ViaFallback`.
#[cfg(not(feature = "legacy-spec"))]
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
    /// MCP requires a tool's `inputSchema`/`outputSchema` to be an *object*
    /// schema, and a conforming client validates it: MCP Inspector rejects a
    /// whole `tools/list` result over one schema that lists `properties` but
    /// never says so. A hand-written schema that left the discriminator out
    /// gets it supplied rather than published as-is.
    #[test]
    fn it_supplies_the_object_type_a_hand_written_schema_omitted() {
        let schema = InputSchema::from(json!({
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }));

        assert_eq!(schema.as_value()["type"], "object");
        assert_eq!(schema.as_value()["required"][0], "name");
    }

    #[cfg(feature = "server")]
    #[test]
    fn it_supplies_the_object_type_when_parsed_from_a_string() {
        let schema = InputSchema::from_json_str(r#"{"properties":{"q":{"type":"string"}}}"#)
            .expect("valid JSON");

        assert_eq!(schema.as_value()["type"], "object");
    }

    /// Filling a gap is not the same as overruling the author: an explicit
    /// type stays exactly as written, whatever it says.
    #[test]
    fn it_leaves_an_explicit_type_alone() {
        for declared in ["string", "array", "object"] {
            let schema = InputSchema::from(json!({ "type": declared }));
            assert_eq!(schema.as_value()["type"], declared);
        }
    }

    /// A boolean schema is not an object, so there is no key to add.
    #[test]
    fn it_leaves_a_boolean_schema_alone() {
        assert_eq!(InputSchema::from(json!(true)).as_value(), &json!(true));
        assert_eq!(InputSchema::from(json!(false)).as_value(), &json!(false));
    }

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
        // or `{"value": ...}` -- the schema IS the value.
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
        // guaranteed across serde_json -- key order in maps is preserved by
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
        // keywords -- all must round-trip verbatim.
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

    #[cfg(not(feature = "legacy-spec"))]
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
        assert!(
            v.get("$defs").is_none(),
            "subschema must be inlined (no $defs)"
        );
    }

    /// The subschema is inlined into `properties`, where `$schema` must not
    /// appear (JSON Schema 2020-12 Core section 8.1.1), and the title schemars
    /// gives a root is only a type name.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn probe_leaves_the_root_keywords_out() {
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Entity {
            name: String,
        }

        for v in [
            crate::__tool_arg_subschema!(Entity),
            crate::__tool_arg_subschema!(Vec<Entity>),
        ] {
            assert!(v.get("$schema").is_none(), "{v}");
            assert!(v.get("title").is_none(), "{v}");
        }
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn probe_keeps_a_title_the_type_sets_itself() {
        #[derive(schemars::JsonSchema)]
        #[schemars(title = "A named thing")]
        #[allow(dead_code)]
        struct Entity {
            name: String,
        }

        let v = crate::__tool_arg_subschema!(Entity);
        assert_eq!(v["title"], json!("A named thing"));
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn probe_falls_back_for_type_without_json_schema() {
        struct Opaque;
        let v = crate::__tool_arg_subschema!(Opaque);
        assert_eq!(v, json!({ "type": "object" }));
    }

    #[cfg(not(feature = "legacy-spec"))]
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
        assert!(
            !s.contains("$ref"),
            "inlined schema must not contain $ref: {s}"
        );
        assert!(
            !s.contains("$defs"),
            "inlined schema must not contain $defs: {s}"
        );
        assert!(v["properties"]["inner"]["properties"]["label"].is_object());
    }

    #[cfg(not(feature = "legacy-spec"))]
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

    /// Recursive types for the `$ref` tests (#172).
    #[cfg(not(feature = "legacy-spec"))]
    #[allow(dead_code)]
    mod recursive {
        #[derive(schemars::JsonSchema)]
        pub(super) struct Node {
            pub(super) label: String,
            pub(super) children: Vec<Node>,
        }

        #[derive(schemars::JsonSchema)]
        pub(super) struct A {
            pub(super) b: Option<Box<B>>,
        }

        #[derive(schemars::JsonSchema)]
        pub(super) struct B {
            pub(super) a: Vec<A>,
        }

        #[derive(schemars::JsonSchema)]
        pub(super) enum Expr {
            Lit(i32),
            Add(Box<Expr>, Box<Expr>),
        }

        /// Named `Node` by schemars too, but a different type.
        pub(super) mod other {
            #[derive(schemars::JsonSchema)]
            pub(crate) struct Node {
                pub(crate) id: u32,
                pub(crate) next: Option<Box<Node>>,
            }

            /// A root named `Node` whose `$defs` already hold the other `Node`.
            #[derive(schemars::JsonSchema)]
            #[schemars(rename = "Node")]
            pub(crate) struct Shadow {
                pub(crate) again: Option<Box<Shadow>>,
                pub(crate) tree: super::Node,
            }
        }
    }

    /// Every local `$ref` in `schema` names something that exists, read
    /// against the root of `schema`.
    #[cfg(not(feature = "legacy-spec"))]
    fn assert_refs_resolve(schema: &Value) {
        let mut copy = schema.clone();
        let mut refs = Vec::new();
        for_each_ref(&mut copy, &mut |reference| refs.push(reference.clone()));
        for reference in refs {
            let pointer = reference
                .strip_prefix('#')
                .unwrap_or_else(|| panic!("not local: {reference}"));
            assert!(
                schema.pointer(pointer).is_some(),
                "{reference} resolves to nothing in {schema}"
            );
        }
    }

    #[cfg(not(feature = "legacy-spec"))]
    fn tool(args: Vec<(&str, Value)>) -> Value {
        crate::__macro_support::object_schema(
            args.into_iter()
                .map(|(name, schema)| (name.to_string(), schema))
                .collect(),
            Vec::new(),
        )
        .into_value()
    }

    /// A type that refers to itself used to publish `{"$ref": "#"}`, which
    /// inside `properties` is the tool's whole `inputSchema`.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_self_reference_points_at_the_type_not_the_tool() {
        let v = tool(vec![(
            "tree",
            crate::__tool_arg_subschema!(recursive::Node),
        )]);

        let tree = &v["properties"]["tree"];
        assert_eq!(tree["properties"]["label"]["type"], json!("string"));
        assert_eq!(
            tree["properties"]["children"]["items"],
            json!({ "$ref": "#/$defs/Node" })
        );
        assert!(tree.get("$defs").is_none(), "{v}");
        assert_eq!(
            v["$defs"]["Node"]["properties"]["children"]["items"],
            json!({ "$ref": "#/$defs/Node" })
        );
        assert_refs_resolve(&v);
    }

    /// A recursive type below the root used to carry its `$defs` inside the
    /// property, where `#/$defs/<Name>` does not look.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_nested_recursive_type_has_its_defs_at_the_root() {
        let v = tool(vec![(
            "forest",
            crate::__tool_arg_subschema!(Vec<recursive::Node>),
        )]);

        let forest = &v["properties"]["forest"];
        assert!(forest.get("$defs").is_none(), "{v}");
        assert_eq!(
            forest["items"]["properties"]["children"]["items"],
            json!({ "$ref": "#/$defs/Node" })
        );
        assert!(v["$defs"]["Node"].is_object(), "{v}");
        assert_refs_resolve(&v);
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_type_two_arguments_share_is_defined_once() {
        let v = tool(vec![
            ("tree", crate::__tool_arg_subschema!(recursive::Node)),
            ("forest", crate::__tool_arg_subschema!(Vec<recursive::Node>)),
        ]);

        let defs = v["$defs"].as_object().unwrap();
        assert_eq!(defs.keys().collect::<Vec<_>>(), vec!["Node"], "{v}");
        assert_refs_resolve(&v);
    }

    /// Two arguments may bring two types schemars names alike: each keeps
    /// its own definition, under a name unique across the tool.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn two_types_of_one_name_keep_apart() {
        let v = tool(vec![
            ("tree", crate::__tool_arg_subschema!(recursive::Node)),
            (
                "chain",
                crate::__tool_arg_subschema!(recursive::other::Node),
            ),
        ]);

        let defs = v["$defs"].as_object().unwrap();
        assert_eq!(defs.keys().collect::<Vec<_>>(), vec!["Node", "Node2"]);
        assert!(defs["Node"]["properties"]["label"].is_object(), "{v}");
        assert!(defs["Node2"]["properties"]["id"].is_object(), "{v}");
        assert_eq!(
            v["properties"]["chain"]["properties"]["next"]["anyOf"][0],
            json!({ "$ref": "#/$defs/Node2" })
        );
        assert_eq!(
            defs["Node2"]["properties"]["next"]["anyOf"][0],
            json!({ "$ref": "#/$defs/Node2" })
        );
        assert_refs_resolve(&v);
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_root_named_like_one_of_its_defs_gets_a_name_of_its_own() {
        let v = crate::__tool_arg_subschema!(recursive::other::Shadow);

        let defs = v["$defs"].as_object().unwrap();
        assert_eq!(defs.len(), 2, "{v}");
        assert_refs_resolve(&v);
        assert_refs_resolve(&tool(vec![("shadow", v)]));
    }

    /// A reference schemars did not write, into the subschema rather than its
    /// `$defs`, follows the subschema to its property. A `$ref` inside an
    /// instance is data, and one to another document is not local.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn other_references_follow_the_subschema() {
        let v = tool(vec![(
            "pair",
            json!({
                "type": "object",
                "properties": {
                    "first": { "type": "string" },
                    "second": { "$ref": "#/properties/first" },
                    "default": { "$ref": "https://example.com/schema" }
                },
                "default": { "$ref": "#" }
            }),
        )]);

        let pair = &v["properties"]["pair"];
        assert_eq!(
            pair["properties"]["second"],
            json!({ "$ref": "#/properties/pair/properties/first" })
        );
        assert_eq!(
            pair["properties"]["default"],
            json!({ "$ref": "https://example.com/schema" })
        );
        assert_eq!(pair["default"], json!({ "$ref": "#" }));
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_name_is_encoded_as_schemars_encodes_it() {
        assert_eq!(def_ref("Tree_for_string"), "#/$defs/Tree_for_string");
        assert_eq!(def_ref("a/b~c"), "#/$defs/a~1b~0c");
        assert_eq!(def_ref("two words%"), "#/$defs/two%20words%25");
    }

    /// The published schema means what the type does: a validator compiles it,
    /// and judges every level of the tree by the type, not by the tool.
    #[cfg(all(not(feature = "legacy-spec"), feature = "client"))]
    #[test]
    fn a_validator_reads_the_recursive_types_right() {
        let v = tool(vec![
            ("tree", crate::__tool_arg_subschema!(recursive::Node)),
            ("forest", crate::__tool_arg_subschema!(Vec<recursive::Node>)),
            (
                "chain",
                crate::__tool_arg_subschema!(recursive::other::Node),
            ),
            (
                "shadow",
                crate::__tool_arg_subschema!(recursive::other::Shadow),
            ),
            ("pair", crate::__tool_arg_subschema!(recursive::A)),
            ("expr", crate::__tool_arg_subschema!(recursive::Expr)),
        ]);
        let validator = jsonschema::draft202012::new(&v).unwrap_or_else(|e| panic!("{e}: {v}"));

        let leaf = json!({ "label": "c", "children": [] });
        let tree = json!({ "label": "a", "children": [{ "label": "b", "children": [leaf] }] });
        let bad_tree = json!({ "label": "a", "children": [{ "label": "b", "children": [{}] }] });
        let chain = json!({ "id": 1, "next": { "id": 2, "next": null } });
        let bad_chain = json!({ "id": 1, "next": { "id": "two" } });
        let expr = json!({ "Add": [{ "Lit": 1 }, { "Add": [{ "Lit": 2 }, { "Lit": 3 }] }] });
        let bad_expr = json!({ "Add": [{ "Lit": 1 }, { "Add": [{ "Lit": 2 }, { "Sub": 3 }] }] });

        for (args, valid) in [
            (json!({ "tree": tree }), true),
            (json!({ "tree": bad_tree }), false),
            (json!({ "forest": [tree, leaf] }), true),
            (json!({ "forest": [bad_tree] }), false),
            (json!({ "chain": chain }), true),
            (json!({ "chain": bad_chain }), false),
            (
                json!({ "shadow": { "again": { "tree": leaf }, "tree": tree } }),
                true,
            ),
            (
                json!({ "shadow": { "again": { "tree": bad_tree }, "tree": tree } }),
                false,
            ),
            (
                json!({ "pair": { "b": { "a": [{ "b": { "a": [] } }] } } }),
                true,
            ),
            (
                json!({ "pair": { "b": { "a": [{ "b": { "a": 1 } }] } } }),
                false,
            ),
            (json!({ "expr": expr }), true),
            (json!({ "expr": bad_expr }), false),
        ] {
            assert_eq!(validator.is_valid(&args), valid, "{args}");
        }
    }
}
