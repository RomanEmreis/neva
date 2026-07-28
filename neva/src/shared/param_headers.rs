//! `x-mcp-header`: mirroring tool arguments into HTTP headers.
//!
//! A server may annotate a property of a tool's `inputSchema` with
//! `x-mcp-header: "<name>"`, asking the client to mirror that argument's value
//! into the `Mcp-Param-<name>` header on the Streamable HTTP transport, so
//! intermediaries can route or police a call without parsing its body.
//!
//! Servers *may* use the annotation; clients **must** honor it. A definition
//! that breaks the constraints below is not merely ignored -- the client drops
//! the whole tool from `tools/list`, so one malformed tool cannot quietly
//! change what a well-formed one sends.

use std::collections::HashMap;
use std::sync::Arc;

/// Tool name -> the arguments that tool mirrors into headers.
///
/// Populated from `tools/list` and read on `tools/call`; shared by handle so
/// the transport task sees what the client registered.
pub(crate) type Registry = Arc<dashmap::DashMap<String, Vec<ParamHeader>>>;

/// The annotation keyword inside a property schema.
const KEYWORD: &str = "x-mcp-header";

/// Prefix of the header a mirrored argument lands in.
pub(crate) const PARAM_HEADER_PREFIX: &str = "Mcp-Param-";

/// One mirrored argument: where to read it, and what to call the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParamHeader {
    /// The chain of `properties` keys leading to the annotated property.
    pub(crate) path: Vec<String>,
    /// The `{name}` of `Mcp-Param-{name}`.
    pub(crate) header: String,
}

/// Why a tool definition was rejected.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ParamHeaderError {
    /// The annotation was not a non-empty HTTP token.
    InvalidName(String),
    /// Two properties asked for the same header (case-insensitively).
    DuplicateName(String),
    /// Annotated a type that cannot be mirrored (only integer/string/boolean).
    UnsupportedType(String),
    /// Annotated somewhere not statically reachable through `properties`.
    Unreachable(String),
}

impl std::fmt::Display for ParamHeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(n) => {
                write!(f, "`x-mcp-header` value {n:?} is not a valid HTTP token")
            }
            Self::DuplicateName(n) => {
                write!(f, "`x-mcp-header` value {n:?} is used more than once")
            }
            Self::UnsupportedType(p) => write!(
                f,
                "`x-mcp-header` on {p:?} needs a primitive type (integer, string or boolean)"
            ),
            Self::Unreachable(p) => write!(
                f,
                "`x-mcp-header` on {p:?} is not statically reachable through `properties`"
            ),
        }
    }
}

/// Whether `c` is an RFC 9110 `tchar`.
fn is_tchar(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// Collects the `x-mcp-header` annotations of a tool's `inputSchema`.
///
/// Returns `Err` if any annotation breaks the spec's constraints; the caller
/// drops the tool in that case.
pub(crate) fn collect(
    input_schema: &serde_json::Value,
) -> Result<Vec<ParamHeader>, ParamHeaderError> {
    let mut found = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    walk(input_schema, &mut Vec::new(), &mut found, &mut seen)?;
    reject_unreachable(input_schema, &mut Vec::new())?;
    Ok(found)
}

/// Descends the `properties` chain, which is the only path the spec allows an
/// annotation to sit on.
fn walk(
    schema: &serde_json::Value,
    path: &mut Vec<String>,
    found: &mut Vec<ParamHeader>,
    seen: &mut HashMap<String, ()>,
) -> Result<(), ParamHeaderError> {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return Ok(());
    };

    for (name, prop) in props {
        path.push(name.clone());

        if let Some(raw) = prop.get(KEYWORD) {
            let header = raw.as_str().unwrap_or_default();
            if header.is_empty() || !header.chars().all(is_tchar) {
                return Err(ParamHeaderError::InvalidName(header.to_owned()));
            }
            let key = header.to_ascii_lowercase();
            if seen.insert(key, ()).is_some() {
                return Err(ParamHeaderError::DuplicateName(header.to_owned()));
            }
            // `number` is excluded deliberately: it has no single lossless
            // textual form, so mirroring it could not round-trip.
            let ty = prop
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            if !matches!(ty, "integer" | "string" | "boolean") {
                return Err(ParamHeaderError::UnsupportedType(path.join(".")));
            }
            found.push(ParamHeader {
                path: path.clone(),
                header: header.to_owned(),
            });
        }

        walk(prop, path, found, seen)?;
        path.pop();
    }
    Ok(())
}

/// Fails if an annotation hides anywhere other than a `properties` chain --
/// under `items`, a composition/conditional keyword, or a `$ref`.
fn reject_unreachable(
    schema: &serde_json::Value,
    path: &mut Vec<String>,
) -> Result<(), ParamHeaderError> {
    const OFF_PATH: [&str; 9] = [
        "items", "oneOf", "anyOf", "allOf", "not", "if", "then", "else", "$defs",
    ];

    if let Some(obj) = schema.as_object() {
        for key in OFF_PATH {
            if let Some(sub) = obj.get(key)
                && contains_annotation(sub)
            {
                return Err(ParamHeaderError::Unreachable(if path.is_empty() {
                    key.to_owned()
                } else {
                    format!("{}.{key}", path.join("."))
                }));
            }
        }
        if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
            for (name, prop) in props {
                path.push(name.clone());
                reject_unreachable(prop, path)?;
                path.pop();
            }
        }
    }
    Ok(())
}

/// Whether `value` carries the annotation anywhere beneath it.
fn contains_annotation(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key(KEYWORD) || map.values().any(contains_annotation)
        }
        serde_json::Value::Array(items) => items.iter().any(contains_annotation),
        _ => false,
    }
}

/// Reads the value each annotated property points at, skipping any that the
/// call did not supply.
pub(crate) fn extract(headers: &[ParamHeader], args: &serde_json::Value) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|h| {
            let mut cur = args;
            for step in &h.path {
                cur = cur.get(step)?;
            }
            let value = match cur {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
                // Anything else was rejected at registration time; a peer that
                // sends it anyway is simply not mirrored.
                _ => return None,
            };
            Some((format!("{PARAM_HEADER_PREFIX}{}", h.header), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_a_top_level_annotation() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": { "type": "string", "x-mcp-header": "Region" },
                "query": { "type": "string" }
            }
        });
        let got = collect(&schema).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, ["region"]);
        assert_eq!(got[0].header, "Region");
    }

    #[test]
    fn collects_a_nested_annotation() {
        // Nested objects are fine as long as every step is a `properties` key.
        let schema = json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "object",
                    "properties": {
                        "region": { "type": "string", "x-mcp-header": "Region" }
                    }
                }
            }
        });
        let got = collect(&schema).unwrap();
        assert_eq!(got[0].path, ["target", "region"]);
    }

    #[test]
    fn rejects_a_non_token_name() {
        for bad in ["", "has space", "has\rcr", "sla/sh"] {
            let schema = json!({
                "type": "object",
                "properties": { "p": { "type": "string", "x-mcp-header": bad } }
            });
            assert!(
                matches!(collect(&schema), Err(ParamHeaderError::InvalidName(_))),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_a_case_insensitive_duplicate() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": { "type": "string", "x-mcp-header": "Region" },
                "b": { "type": "string", "x-mcp-header": "region" }
            }
        });
        assert!(matches!(
            collect(&schema),
            Err(ParamHeaderError::DuplicateName(_))
        ));
    }

    #[test]
    fn rejects_a_non_primitive_type() {
        // `number` is explicitly excluded alongside objects and arrays.
        for ty in ["number", "object", "array"] {
            let schema = json!({
                "type": "object",
                "properties": { "p": { "type": ty, "x-mcp-header": "P" } }
            });
            assert!(
                matches!(collect(&schema), Err(ParamHeaderError::UnsupportedType(_))),
                "{ty} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_an_annotation_off_the_properties_chain() {
        for key in [
            "items", "oneOf", "anyOf", "allOf", "not", "if", "then", "else",
        ] {
            let schema = json!({
                "type": "object",
                "properties": {
                    "p": {
                        "type": "array",
                        key: { "type": "string", "x-mcp-header": "P" }
                    }
                }
            });
            assert!(
                matches!(collect(&schema), Err(ParamHeaderError::Unreachable(_))),
                "{key} must be rejected"
            );
        }
    }

    #[test]
    fn extracts_present_values_and_skips_absent_ones() {
        let headers = vec![
            ParamHeader {
                path: vec!["region".into()],
                header: "Region".into(),
            },
            ParamHeader {
                path: vec!["limit".into()],
                header: "Limit".into(),
            },
            ParamHeader {
                path: vec!["dry".into()],
                header: "Dry".into(),
            },
            ParamHeader {
                path: vec!["absent".into()],
                header: "Absent".into(),
            },
        ];
        let args = json!({ "region": "us-west1", "limit": 42, "dry": true });

        let got = extract(&headers, &args);
        assert_eq!(
            got,
            vec![
                ("Mcp-Param-Region".to_string(), "us-west1".to_string()),
                ("Mcp-Param-Limit".to_string(), "42".to_string()),
                ("Mcp-Param-Dry".to_string(), "true".to_string()),
            ]
        );
    }

    #[test]
    fn extracts_through_a_nested_path() {
        let headers = vec![ParamHeader {
            path: vec!["target".into(), "region".into()],
            header: "Region".into(),
        }];
        let args = json!({ "target": { "region": "eu-west1" } });

        assert_eq!(
            extract(&headers, &args),
            vec![("Mcp-Param-Region".to_string(), "eu-west1".to_string())]
        );
    }
}
