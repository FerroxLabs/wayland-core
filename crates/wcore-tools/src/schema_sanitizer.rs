//! Sanitize tool JSON schemas for broad LLM-backend compatibility.
//!
//! Some local inference backends (notably llama.cpp's `json-schema-to-grammar`
//! converter used to build GBNF tool-call parsers) are strict about what JSON
//! Schema shapes they accept. Schemas that OpenAI / Anthropic / most cloud
//! providers silently accept can make llama.cpp fail the entire request with:
//!
//! ```text
//! HTTP 400: Unable to generate parser for this template.
//! Automatic parser generation failed: JSON schema conversion failed:
//! Unrecognized schema: "object"
//! ```
//!
//! Known-hostile constructs handled here:
//!
//! * `{"type": "object"}` with no `properties` — rejected as a node the
//!   grammar generator can't constrain.
//! * A schema value that is the bare string `"object"` instead of a dict
//!   (malformed MCP server output, e.g. `additionalProperties: "object"`).
//! * `"type": ["string", "null"]` array types — many converters only accept
//!   single-string `type`.
//! * `anyOf` / `oneOf` unions whose only purpose is to permit `null` for
//!   optional fields (common Pydantic/MCP shape). Anthropic rejects these at
//!   the top of `input_schema`; collapse them to the non-null branch.
//! * Unconstrained `additionalProperties` on objects with empty properties.
//!
//! Distinct from `wcore_config::compat::sanitize_json_schema`, which is a
//! narrower Bedrock-targeted root-wrapper / `additionalProperties`-stripper.
//! This module walks the final tool schema tree and fixes the broader set of
//! known-hostile constructs on a deep copy of the input.
//!
//! Ported from the prior Wayland Python engine.

use serde_json::{Map, Value, json};

/// Top-level combinator keys that OpenAI's Codex backend rejects.
const TOP_LEVEL_FORBIDDEN_KEYS: &[&str] = &["allOf", "anyOf", "oneOf", "enum", "not"];

/// JSON Schema primitive type names that may appear as a bare string in
/// malformed MCP server output.
const PRIMITIVE_TYPES: &[&str] = &[
    "object", "string", "number", "integer", "boolean", "array", "null",
];

/// Schema sibling keys whose VALUES are not schemas (literal lists), so we
/// must not recurse into them with `_sanitize_node`.
const NON_SCHEMA_LIST_KEYS: &[&str] = &["required", "enum", "examples"];

/// Reactive-only strip keys (invoked on llama.cpp grammar-parse failure).
const STRIP_ON_RECOVERY_KEYS: &[&str] = &["pattern", "format"];

/// Return a sanitized JSON Schema fragment.
///
/// Single-schema entry point: walks one schema (the `parameters` body of a
/// tool, or any nested JSON Schema) and fixes the known-hostile constructs
/// that break llama.cpp's GBNF generator and other strict validators.
///
/// The input is never mutated; a deep clone is sanitized and returned.
/// Conservative: a well-formed schema round-trips unchanged.
pub fn sanitize_schema(schema: &Value) -> Value {
    if !schema.is_object() {
        return minimal_object_schema();
    }
    let mut sanitized = sanitize_node(schema.clone(), "<schema>");
    if !sanitized.is_object() {
        return minimal_object_schema();
    }
    sanitized = strip_nullable_unions(&sanitized, true);
    sanitized
}

/// Return a copy of `tools` with each tool's parameter schema sanitized.
///
/// Input is an OpenAI-format tool list:
/// `[{"type": "function", "function": {"name": ..., "parameters": {...}}}]`.
pub fn sanitize_tool_schemas(tools: &[Value]) -> Vec<Value> {
    if tools.is_empty() {
        return Vec::new();
    }
    tools.iter().map(sanitize_single_tool).collect()
}

fn sanitize_single_tool(tool: &Value) -> Value {
    let mut out = tool.clone();
    let Some(out_obj) = out.as_object_mut() else {
        return out;
    };
    let Some(fn_val) = out_obj.get_mut("function") else {
        return out;
    };
    let Some(fn_obj) = fn_val.as_object_mut() else {
        return out;
    };

    let tool_name = fn_obj
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("<tool>")
        .to_string();

    let params = fn_obj.get("parameters").cloned();
    let new_params = match params {
        Some(Value::Object(_)) => {
            let p = params.unwrap();
            let p = sanitize_node(p, &tool_name);
            let mut top = if let Value::Object(m) = p {
                m
            } else {
                // Recursion collapsed top to non-object; substitute minimal.
                return write_params(&mut out, minimal_object_schema());
            };
            if top.get("type").and_then(Value::as_str) != Some("object") {
                top.insert("type".to_string(), Value::String("object".to_string()));
            }
            let has_props = matches!(top.get("properties"), Some(Value::Object(_)));
            if !has_props {
                top.insert("properties".to_string(), Value::Object(Map::new()));
            }
            let p = Value::Object(top);
            let p = strip_nullable_unions(&p, true);
            strip_top_level_combinators(&p)
        }
        _ => minimal_object_schema(),
    };

    write_params(&mut out, new_params)
}

fn write_params(tool: &mut Value, params: Value) -> Value {
    if let Some(fn_obj) = tool
        .as_object_mut()
        .and_then(|o| o.get_mut("function"))
        .and_then(Value::as_object_mut)
    {
        fn_obj.insert("parameters".to_string(), params);
    }
    tool.clone()
}

fn strip_top_level_combinators(params: &Value) -> Value {
    let Some(obj) = params.as_object() else {
        return params.clone();
    };
    let mut out = obj.clone();
    for key in TOP_LEVEL_FORBIDDEN_KEYS {
        out.remove(*key);
    }
    Value::Object(out)
}

/// Collapse `anyOf` / `oneOf` nullable unions to the non-null branch.
///
/// MCP / Pydantic optional fields commonly arrive as:
///
/// ```json
/// {"anyOf": [{"type": "string"}, {"type": "null"}], "default": null}
/// ```
///
/// Anthropic's tool input-schema validator rejects the null branch. Tool
/// optionality is already represented by the parent object's `required`
/// array, so we collapse the union to the single non-null variant.
///
/// Metadata (`title`, `description`, `default`, `examples`) on the outer
/// union node is carried over to the replacement variant.
pub fn strip_nullable_unions(schema: &Value, keep_nullable_hint: bool) -> Value {
    match schema {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|i| strip_nullable_unions(i, keep_nullable_hint))
                .collect(),
        ),
        Value::Object(map) => {
            let mut stripped = Map::new();
            for (k, v) in map {
                stripped.insert(k.clone(), strip_nullable_unions(v, keep_nullable_hint));
            }
            for key in ["anyOf", "oneOf"] {
                let Some(Value::Array(variants)) = stripped.get(key).cloned() else {
                    continue;
                };
                let non_null: Vec<&Value> = variants
                    .iter()
                    .filter(|item| {
                        !item
                            .as_object()
                            .and_then(|o| o.get("type"))
                            .and_then(Value::as_str)
                            .map(|s| s == "null")
                            .unwrap_or(false)
                    })
                    .collect();
                if non_null.len() == 1 && non_null.len() != variants.len() {
                    let mut replacement = match non_null[0] {
                        Value::Object(m) => m.clone(),
                        _ => Map::new(),
                    };
                    if keep_nullable_hint {
                        replacement
                            .entry("nullable")
                            .or_insert_with(|| Value::Bool(true));
                    }
                    for meta_key in ["title", "description", "default", "examples"] {
                        if let Some(meta) = stripped.get(meta_key)
                            && !replacement.contains_key(meta_key)
                        {
                            replacement.insert(meta_key.to_string(), meta.clone());
                        }
                    }
                    return strip_nullable_unions(&Value::Object(replacement), keep_nullable_hint);
                }
            }
            Value::Object(stripped)
        }
        other => other.clone(),
    }
}

fn sanitize_node(node: Value, _path: &str) -> Value {
    // Malformed: the schema position holds a bare string like "object".
    if let Value::String(s) = &node {
        if PRIMITIVE_TYPES.contains(&s.as_str()) {
            if s == "object" {
                return minimal_object_schema();
            }
            let mut m = Map::new();
            m.insert("type".to_string(), Value::String(s.clone()));
            return Value::Object(m);
        }
        return minimal_object_schema();
    }

    if let Value::Array(items) = node {
        return Value::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(i, item)| sanitize_node(item, &format!("{_path}[{i}]")))
                .collect(),
        );
    }

    let Value::Object(map) = node else {
        return node;
    };

    let mut out: Map<String, Value> = Map::new();
    for (key, value) in map {
        match key.as_str() {
            "type" if value.is_array() => {
                let arr = value.as_array().unwrap();
                let non_null: Vec<&Value> =
                    arr.iter().filter(|t| t.as_str() != Some("null")).collect();
                let had_null = arr.iter().any(|t| t.as_str() == Some("null"));
                if non_null.len() == 1
                    && let Some(s) = non_null[0].as_str()
                {
                    out.insert("type".to_string(), Value::String(s.to_string()));
                    if had_null {
                        out.entry("nullable").or_insert_with(|| Value::Bool(true));
                    }
                    continue;
                }
                // Fallback: pick the first non-null string type.
                if let Some(first_str) = arr.iter().filter_map(Value::as_str).find(|t| *t != "null")
                {
                    out.insert("type".to_string(), Value::String(first_str.to_string()));
                    continue;
                }
                // All-null or empty list -> treat as object.
                out.insert("type".to_string(), Value::String("object".to_string()));
            }
            "properties" | "$defs" | "definitions" if value.is_object() => {
                let inner = value.as_object().unwrap();
                let mut new_inner = Map::new();
                for (sub_k, sub_v) in inner {
                    new_inner.insert(
                        sub_k.clone(),
                        sanitize_node(sub_v.clone(), &format!("{_path}.{key}.{sub_k}")),
                    );
                }
                out.insert(key, Value::Object(new_inner));
            }
            "items" | "additionalProperties" => {
                if value.is_boolean() {
                    out.insert(key, value);
                } else {
                    let sub_path = format!("{_path}.{key}");
                    out.insert(key, sanitize_node(value, &sub_path));
                }
            }
            "anyOf" | "oneOf" | "allOf" if value.is_array() => {
                let arr = value.as_array().unwrap();
                let new_arr: Vec<Value> = arr
                    .iter()
                    .enumerate()
                    .map(|(i, item)| sanitize_node(item.clone(), &format!("{_path}.{key}[{i}]")))
                    .collect();
                out.insert(key, Value::Array(new_arr));
            }
            k if NON_SCHEMA_LIST_KEYS.contains(&k) => {
                // Pass through literal lists (values are not schemas).
                out.insert(key, value);
            }
            _ => {
                if value.is_object() || value.is_array() {
                    let sub_path = format!("{_path}.{key}");
                    out.insert(key, sanitize_node(value, &sub_path));
                } else {
                    out.insert(key, value);
                }
            }
        }
    }

    // Object nodes without properties: inject empty properties dict.
    if out.get("type").and_then(Value::as_str) == Some("object")
        && !matches!(out.get("properties"), Some(Value::Object(_)))
    {
        out.insert("properties".to_string(), Value::Object(Map::new()));
    }

    // Prune `required` entries that don't exist in properties.
    if out.get("type").and_then(Value::as_str) == Some("object")
        && let Some(Value::Array(req)) = out.get("required").cloned()
    {
        let empty = Map::new();
        let props = out
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let valid: Vec<Value> = req
            .into_iter()
            .filter(|r| r.as_str().map(|s| props.contains_key(s)).unwrap_or(false))
            .collect();
        if valid.is_empty() {
            out.remove("required");
        } else {
            out.insert("required".to_string(), Value::Array(valid));
        }
    }

    Value::Object(out)
}

fn minimal_object_schema() -> Value {
    json!({"type": "object", "properties": {}})
}

/// Strip `pattern` and `format` JSON Schema keywords from tool schemas.
///
/// This is a *reactive* sanitizer invoked only when llama.cpp's
/// `json-schema-to-grammar` converter has rejected a tool schema with an HTTP
/// 400 grammar-parse error. llama.cpp's regex engine supports only a small
/// subset of ECMAScript regex — it rejects escape classes like `\d`, `\w`,
/// `\s` and most `format` values. Cloud providers accept these keywords fine
/// and rely on them as prompting hints, so we keep them in the default schema
/// and only strip on demand.
///
/// The strip operates on a sibling of `type` — a property literally *named*
/// `pattern` (e.g. the first arg of the built-in `search_files` tool) is not
/// affected because property names live in the `properties` dict.
///
/// Mutates `tools` in place; returns the number of keywords removed.
pub fn strip_pattern_and_format(tools: &mut [Value]) -> usize {
    if tools.is_empty() {
        return 0;
    }
    let mut stripped = 0usize;
    for tool in tools.iter_mut() {
        let Some(fn_obj) = tool
            .as_object_mut()
            .and_then(|o| o.get_mut("function"))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        if let Some(params) = fn_obj.get_mut("parameters")
            && params.is_object()
        {
            walk_strip(params, &mut stripped);
        }
    }
    stripped
}

fn walk_strip(node: &mut Value, stripped: &mut usize) {
    if let Some(map) = node.as_object_mut() {
        let is_schema_node = map.contains_key("type")
            || map.contains_key("anyOf")
            || map.contains_key("oneOf")
            || map.contains_key("allOf");
        if is_schema_node {
            for key in STRIP_ON_RECOVERY_KEYS {
                if map.remove(*key).is_some() {
                    *stripped += 1;
                }
            }
        }
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in keys {
            if let Some(v) = map.get_mut(&k) {
                walk_strip(v, stripped);
            }
        }
    } else if let Some(arr) = node.as_array_mut() {
        for v in arr.iter_mut() {
            walk_strip(v, stripped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_schema_passes_through_valid() {
        let schema = json!({
            "type": "object",
            "properties": {
                "cmd": {"type": "string"}
            },
            "required": ["cmd"]
        });
        let out = sanitize_schema(&schema);
        assert_eq!(out["type"], "object");
        assert_eq!(out["properties"]["cmd"]["type"], "string");
        assert_eq!(out["required"], json!(["cmd"]));
    }

    #[test]
    fn sanitize_schema_injects_empty_properties_for_object_without_props() {
        let schema = json!({"type": "object"});
        let out = sanitize_schema(&schema);
        assert_eq!(out["type"], "object");
        assert!(out["properties"].is_object());
        assert_eq!(out["properties"].as_object().unwrap().len(), 0);
    }

    #[test]
    fn sanitize_schema_collapses_array_type_with_null() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": ["string", "null"]}
            }
        });
        let out = sanitize_schema(&schema);
        assert_eq!(out["properties"]["name"]["type"], "string");
        assert_eq!(out["properties"]["name"]["nullable"], true);
    }

    #[test]
    fn sanitize_schema_recurses_into_items_and_oneof() {
        let schema = json!({
            "type": "object",
            "properties": {
                "list": {
                    "type": "array",
                    "items": {"type": ["integer", "null"]}
                },
                "union": {
                    "oneOf": [
                        {"type": "string"},
                        {"type": "integer"}
                    ]
                }
            }
        });
        let out = sanitize_schema(&schema);
        assert_eq!(out["properties"]["list"]["items"]["type"], "integer");
        // Non-nullable union preserved.
        assert!(out["properties"]["union"]["oneOf"].is_array());
        assert_eq!(
            out["properties"]["union"]["oneOf"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn sanitize_schema_replaces_bare_string_schema() {
        // Non-dict at top → minimal valid object.
        let schema = Value::String("object".to_string());
        let out = sanitize_schema(&schema);
        assert_eq!(out["type"], "object");
        assert!(out["properties"].is_object());
    }

    #[test]
    fn strip_nullable_unions_collapses_anyof_null() {
        let schema = json!({
            "anyOf": [{"type": "string"}, {"type": "null"}],
            "description": "optional name"
        });
        let out = strip_nullable_unions(&schema, true);
        assert_eq!(out["type"], "string");
        assert_eq!(out["nullable"], true);
        assert_eq!(out["description"], "optional name");
        assert!(out.get("anyOf").is_none());
    }

    #[test]
    fn strip_nullable_unions_leaves_meaningful_union_intact() {
        let schema = json!({
            "anyOf": [{"type": "string"}, {"type": "integer"}]
        });
        let out = strip_nullable_unions(&schema, true);
        assert!(out["anyOf"].is_array());
        assert_eq!(out["anyOf"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn sanitize_tool_schemas_strips_top_level_combinators() {
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "demo",
                "parameters": {
                    "type": "object",
                    "properties": {"x": {"type": "string"}},
                    "allOf": [{"required": ["x"]}]
                }
            }
        })];
        let out = sanitize_tool_schemas(&tools);
        assert!(out[0]["function"]["parameters"].get("allOf").is_none());
        assert_eq!(
            out[0]["function"]["parameters"]["properties"]["x"]["type"],
            "string"
        );
    }

    #[test]
    fn sanitize_tool_schemas_substitutes_missing_parameters() {
        let tools = vec![json!({
            "type": "function",
            "function": {"name": "noargs"}
        })];
        let out = sanitize_tool_schemas(&tools);
        assert_eq!(out[0]["function"]["parameters"]["type"], "object");
        assert!(out[0]["function"]["parameters"]["properties"].is_object());
    }

    #[test]
    fn strip_pattern_and_format_removes_schema_siblings() {
        let mut tools = vec![json!({
            "type": "function",
            "function": {
                "name": "demo",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "email": {"type": "string", "format": "email", "pattern": "^.+@.+$"},
                        "pattern": {"type": "string", "description": "literal property"}
                    }
                }
            }
        })];
        let count = strip_pattern_and_format(&mut tools);
        assert_eq!(count, 2);
        let email = &tools[0]["function"]["parameters"]["properties"]["email"];
        assert!(email.get("format").is_none());
        assert!(email.get("pattern").is_none());
        // Literal `pattern` property key preserved.
        assert!(
            tools[0]["function"]["parameters"]["properties"]
                .get("pattern")
                .is_some()
        );
    }

    #[test]
    fn sanitize_schema_prunes_invalid_required_entries() {
        let schema = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "required": ["a", "missing"]
        });
        let out = sanitize_schema(&schema);
        assert_eq!(out["required"], json!(["a"]));
    }

    #[test]
    fn sanitize_schema_keeps_enum_literals_intact() {
        // enum values are NOT schemas — must not be misinterpreted as bare-string schemas.
        let schema = json!({
            "type": "object",
            "properties": {
                "color": {"type": "string", "enum": ["object", "string", "red"]}
            }
        });
        let out = sanitize_schema(&schema);
        assert_eq!(
            out["properties"]["color"]["enum"],
            json!(["object", "string", "red"])
        );
    }
}
