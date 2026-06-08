//! A4 — schema-validated agent output.
//!
//! Workflow steps may declare a named schema (`schema: Some("findings")`).
//! The schema *bodies* live in the workflow RON's `schemas` table as JSON
//! strings using a **minimal JSON-Schema subset** — enough to constrain a
//! sub-agent's structured return without pulling a heavy validator crate
//! into the build. [`WorkflowSchema::parse`] compiles one such JSON string
//! into the [`WorkflowSchema`] tree below; [`validate`] then checks an
//! agent's text output against it.
//!
//! ## Supported JSON-Schema subset
//!
//! ```json
//! { "type": "object",
//!   "required": ["findings"],
//!   "properties": {
//!     "findings": { "type": "array", "items": { "type": "string" } },
//!     "count":    { "type": "number" },
//!     "ok":       { "type": "boolean" },
//!     "nested":   { "type": "object", "properties": { "a": { "type": "string" } } }
//!   }
//! }
//! ```
//!
//! Recognised keywords:
//! - `type`: one of `object`, `array`, `string`, `number`, `integer`,
//!   `boolean`, `null` (the JSON primitive types).
//! - `properties` (object only): a map of field name → sub-schema.
//! - `required` (object only): field names that must be present.
//! - `items` (array only): a single sub-schema every element must match.
//!
//! Everything else (`enum`, `oneOf`, `$ref`, `pattern`, numeric bounds, …)
//! is **out of scope** and silently ignored — a deliberately small, correct
//! core rather than a partial-and-surprising full implementation. Unknown
//! `type` strings are rejected at compile time so an author typo surfaces as
//! a schema-definition error, not a silent pass.

use std::collections::BTreeMap;

use serde_json::Value;
use thiserror::Error;

/// The JSON primitive type a schema node constrains its value to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaType {
    Object,
    Array,
    String,
    /// Any JSON number (`number` or `integer` — we do not enforce the
    /// integral constraint, only "is a number").
    Number,
    /// Strictly an integral JSON number.
    Integer,
    Boolean,
    Null,
}

impl SchemaType {
    /// Human-readable name used in error messages (matches the JSON-Schema
    /// keyword the author wrote).
    fn label(self) -> &'static str {
        match self {
            SchemaType::Object => "object",
            SchemaType::Array => "array",
            SchemaType::String => "string",
            SchemaType::Number => "number",
            SchemaType::Integer => "integer",
            SchemaType::Boolean => "boolean",
            SchemaType::Null => "null",
        }
    }

    fn from_keyword(s: &str) -> Option<SchemaType> {
        Some(match s {
            "object" => SchemaType::Object,
            "array" => SchemaType::Array,
            "string" => SchemaType::String,
            "number" => SchemaType::Number,
            "integer" => SchemaType::Integer,
            "boolean" => SchemaType::Boolean,
            "null" => SchemaType::Null,
            _ => return None,
        })
    }
}

/// A compiled node of the minimal JSON-Schema subset.
///
/// Built once via [`WorkflowSchema::parse`] from the author's JSON string,
/// then reused across retries by [`validate`].
#[derive(Debug, Clone)]
pub struct WorkflowSchema {
    ty: SchemaType,
    /// `object` only: field name → sub-schema. A `BTreeMap` keeps property
    /// iteration deterministic so error messages are stable.
    properties: BTreeMap<String, WorkflowSchema>,
    /// `object` only: required field names.
    required: Vec<String>,
    /// `array` only: the schema every element must match (if declared).
    items: Option<Box<WorkflowSchema>>,
}

/// Failure compiling a schema *definition* (the author's JSON-Schema string),
/// as opposed to a value failing validation against it.
#[derive(Debug, Error)]
pub enum SchemaDefError {
    /// The schema body was not valid JSON.
    #[error("schema is not valid JSON: {0}")]
    NotJson(String),

    /// A schema node was not a JSON object (every node must be an object,
    /// e.g. `{ "type": "string" }`).
    #[error("schema node at `{path}` must be a JSON object")]
    NotAnObject { path: String },

    /// A schema node had no `type` keyword.
    #[error("schema node at `{path}` is missing a `type`")]
    MissingType { path: String },

    /// A schema node's `type` was not one of the supported keywords.
    #[error("schema node at `{path}` has unsupported type `{ty}`")]
    UnsupportedType { path: String, ty: String },

    /// The schema definition nested deeper than
    /// [`super::limits::MAX_NESTING_DEPTH`]. Bounded so an attacker-supplied
    /// schema body cannot overflow the recursive [`WorkflowSchema::from_value`]
    /// compiler (an uncatchable abort).
    #[error("schema node at `{path}` is nested too deeply (limit {limit})")]
    TooDeep { path: String, limit: usize },
}

/// A value failed to validate against a [`WorkflowSchema`].
///
/// Carries the JSON-Pointer-style path to the first offending node plus the
/// expected/actual mismatch, so the runner can feed a precise correction back
/// to the sub-agent on retry.
#[derive(Debug, Error)]
pub enum SchemaError {
    /// The agent's output was not parseable as JSON at all.
    #[error("output is not valid JSON: {0}")]
    NotJson(String),

    /// A value's JSON type did not match the schema.
    #[error("at `{path}`: expected type `{expected}`, found `{actual}`")]
    TypeMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    /// A required object property was absent.
    #[error("at `{path}`: missing required property `{property}`")]
    MissingProperty { path: String, property: String },
}

impl WorkflowSchema {
    /// Compile a JSON-Schema-subset string into a [`WorkflowSchema`].
    ///
    /// Returns a [`SchemaDefError`] (with a path to the offending node) on any
    /// structural problem in the *definition* — never panics.
    pub fn parse(src: &str) -> Result<WorkflowSchema, SchemaDefError> {
        let root: Value =
            serde_json::from_str(src).map_err(|e| SchemaDefError::NotJson(e.to_string()))?;
        Self::from_value(&root, "", 0)
    }

    /// Recursively compile one schema node at JSON-Pointer `path`.
    ///
    /// `depth` is the current recursion depth; it errors past
    /// [`super::limits::MAX_NESTING_DEPTH`] instead of recursing unboundedly so
    /// an attacker-supplied schema body cannot overflow the stack.
    fn from_value(
        node: &Value,
        path: &str,
        depth: usize,
    ) -> Result<WorkflowSchema, SchemaDefError> {
        if depth > super::limits::MAX_NESTING_DEPTH {
            return Err(SchemaDefError::TooDeep {
                path: pointer_or_root(path),
                limit: super::limits::MAX_NESTING_DEPTH,
            });
        }
        let obj = node
            .as_object()
            .ok_or_else(|| SchemaDefError::NotAnObject {
                path: pointer_or_root(path),
            })?;

        let ty_str =
            obj.get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| SchemaDefError::MissingType {
                    path: pointer_or_root(path),
                })?;
        let ty =
            SchemaType::from_keyword(ty_str).ok_or_else(|| SchemaDefError::UnsupportedType {
                path: pointer_or_root(path),
                ty: ty_str.to_string(),
            })?;

        let mut properties = BTreeMap::new();
        if let Some(props) = obj.get("properties").and_then(Value::as_object) {
            for (name, sub) in props {
                let child_path = format!("{path}/properties/{name}");
                properties.insert(name.clone(), Self::from_value(sub, &child_path, depth + 1)?);
            }
        }

        let required = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let items = match obj.get("items") {
            Some(sub) => Some(Box::new(Self::from_value(
                sub,
                &format!("{path}/items"),
                depth + 1,
            )?)),
            None => None,
        };

        Ok(WorkflowSchema {
            ty,
            properties,
            required,
            items,
        })
    }
}

/// Validate an agent's text output against a compiled [`WorkflowSchema`].
///
/// Parses `text` as JSON, then walks the schema. Returns the parsed,
/// schema-conforming [`Value`] on success, or the **first** [`SchemaError`]
/// (with a field path + expected/actual) on mismatch.
pub fn validate(text: &str, schema: &WorkflowSchema) -> Result<Value, SchemaError> {
    let value: Value =
        serde_json::from_str(text.trim()).map_err(|e| SchemaError::NotJson(e.to_string()))?;
    check(&value, schema, "")?;
    Ok(value)
}

/// Recursive validation of `value` against `schema` at JSON-Pointer `path`.
fn check(value: &Value, schema: &WorkflowSchema, path: &str) -> Result<(), SchemaError> {
    if !type_matches(value, schema.ty) {
        return Err(SchemaError::TypeMismatch {
            path: pointer_or_root(path),
            expected: schema.ty.label().to_string(),
            actual: json_type_label(value).to_string(),
        });
    }

    match schema.ty {
        SchemaType::Object => {
            // Normally unreachable: `type_matches` above already verified this
            // value is an object. Propagate a `TypeMismatch` instead of
            // panicking should that invariant ever be violated.
            let map = value.as_object().ok_or_else(|| SchemaError::TypeMismatch {
                path: pointer_or_root(path),
                expected: schema.ty.label().to_string(),
                actual: json_type_label(value).to_string(),
            })?;
            for req in &schema.required {
                if !map.contains_key(req) {
                    return Err(SchemaError::MissingProperty {
                        path: pointer_or_root(path),
                        property: req.clone(),
                    });
                }
            }
            // Validate declared properties that are present. Extra properties
            // (not in `properties`) are permitted — this subset has no
            // `additionalProperties: false`.
            for (name, sub) in &schema.properties {
                if let Some(child) = map.get(name) {
                    check(child, sub, &format!("{path}/{name}"))?;
                }
            }
        }
        SchemaType::Array => {
            if let Some(item_schema) = &schema.items {
                // Normally unreachable: `type_matches` above already verified
                // this value is an array. Propagate a `TypeMismatch` instead of
                // panicking should that invariant ever be violated.
                let arr = value.as_array().ok_or_else(|| SchemaError::TypeMismatch {
                    path: pointer_or_root(path),
                    expected: schema.ty.label().to_string(),
                    actual: json_type_label(value).to_string(),
                })?;
                for (i, elem) in arr.iter().enumerate() {
                    check(elem, item_schema, &format!("{path}/{i}"))?;
                }
            }
        }
        // Scalars: the type check is the whole constraint.
        SchemaType::String
        | SchemaType::Number
        | SchemaType::Integer
        | SchemaType::Boolean
        | SchemaType::Null => {}
    }

    Ok(())
}

/// Whether a JSON value satisfies a schema's declared primitive type.
fn type_matches(value: &Value, ty: SchemaType) -> bool {
    match ty {
        SchemaType::Object => value.is_object(),
        SchemaType::Array => value.is_array(),
        SchemaType::String => value.is_string(),
        // `number` accepts any JSON number (integral or fractional).
        SchemaType::Number => value.is_number(),
        // `integer` requires an integral number (i64/u64, not a fraction).
        SchemaType::Integer => value.is_i64() || value.is_u64(),
        SchemaType::Boolean => value.is_boolean(),
        SchemaType::Null => value.is_null(),
    }
}

/// The JSON-Schema type keyword describing a concrete value (for error text).
fn json_type_label(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::Bool(_) => "boolean",
        Value::Null => "null",
    }
}

/// Render an empty path as `/` (the document root) for readable messages.
fn pointer_or_root(path: &str) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(src: &str) -> WorkflowSchema {
        WorkflowSchema::parse(src).expect("schema definition should compile")
    }

    #[test]
    fn object_with_required_and_typed_properties_passes() {
        let s = schema(
            r#"{
                "type": "object",
                "required": ["findings", "count"],
                "properties": {
                    "findings": { "type": "array", "items": { "type": "string" } },
                    "count": { "type": "number" }
                }
            }"#,
        );
        let out = validate(r#"{ "findings": ["a", "b"], "count": 2 }"#, &s)
            .expect("conforming object should validate");
        assert_eq!(out["count"], serde_json::json!(2));
    }

    #[test]
    fn extra_properties_are_allowed() {
        let s = schema(r#"{ "type": "object", "properties": { "a": { "type": "string" } } }"#);
        validate(r#"{ "a": "x", "b": 99 }"#, &s).expect("extra props are permitted");
    }

    #[test]
    fn missing_required_property_reports_path_and_name() {
        let s = schema(r#"{ "type": "object", "required": ["needed"], "properties": {} }"#);
        let err = validate(r#"{ "other": 1 }"#, &s).expect_err("missing required must fail");
        match err {
            SchemaError::MissingProperty { path, property } => {
                assert_eq!(path, "/");
                assert_eq!(property, "needed");
            }
            other => panic!("expected MissingProperty, got {other:?}"),
        }
    }

    #[test]
    fn top_level_type_mismatch_reports_expected_and_actual() {
        let s = schema(r#"{ "type": "object" }"#);
        let err = validate(r#"["not", "an", "object"]"#, &s).expect_err("array is not object");
        match err {
            SchemaError::TypeMismatch {
                path,
                expected,
                actual,
            } => {
                assert_eq!(path, "/");
                assert_eq!(expected, "object");
                assert_eq!(actual, "array");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn nested_property_mismatch_reports_full_path() {
        let s = schema(
            r#"{
                "type": "object",
                "properties": {
                    "inner": { "type": "object", "properties": { "n": { "type": "number" } } }
                }
            }"#,
        );
        let err = validate(r#"{ "inner": { "n": "not a number" } }"#, &s)
            .expect_err("nested type mismatch must fail");
        match err {
            SchemaError::TypeMismatch { path, expected, .. } => {
                assert_eq!(path, "/inner/n");
                assert_eq!(expected, "number");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn array_item_mismatch_reports_index() {
        let s = schema(r#"{ "type": "array", "items": { "type": "string" } }"#);
        let err = validate(r#"["ok", 7]"#, &s).expect_err("number element should fail");
        match err {
            SchemaError::TypeMismatch { path, .. } => assert_eq!(path, "/1"),
            other => panic!("expected TypeMismatch at index 1, got {other:?}"),
        }
    }

    #[test]
    fn integer_rejects_fractional_number() {
        let s = schema(r#"{ "type": "integer" }"#);
        assert!(validate("3", &s).is_ok());
        let err = validate("3.5", &s).expect_err("fractional is not integer");
        assert!(matches!(err, SchemaError::TypeMismatch { .. }));
    }

    #[test]
    fn number_accepts_both_integral_and_fractional() {
        let s = schema(r#"{ "type": "number" }"#);
        assert!(validate("3", &s).is_ok());
        assert!(validate("3.5", &s).is_ok());
    }

    #[test]
    fn non_json_output_is_rejected() {
        let s = schema(r#"{ "type": "object" }"#);
        let err = validate("this is prose, not json", &s).expect_err("prose is not JSON");
        assert!(matches!(err, SchemaError::NotJson(_)));
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        let s = schema(r#"{ "type": "object" }"#);
        validate("\n  { \"a\": 1 }\n", &s).expect("leading/trailing whitespace is trimmed");
    }

    #[test]
    fn unsupported_schema_type_is_a_definition_error() {
        let err = WorkflowSchema::parse(r#"{ "type": "tuple" }"#)
            .expect_err("unknown type keyword must be rejected");
        match err {
            SchemaDefError::UnsupportedType { ty, .. } => assert_eq!(ty, "tuple"),
            other => panic!("expected UnsupportedType, got {other:?}"),
        }
    }

    #[test]
    fn schema_node_missing_type_is_a_definition_error() {
        let err = WorkflowSchema::parse(r#"{ "properties": {} }"#)
            .expect_err("a node without `type` must be rejected");
        assert!(matches!(err, SchemaDefError::MissingType { .. }));
    }

    #[test]
    fn schema_definition_must_be_json() {
        let err = WorkflowSchema::parse("not json").expect_err("bad JSON must be rejected");
        assert!(matches!(err, SchemaDefError::NotJson(_)));
    }

    #[test]
    fn deeply_nested_schema_body_errors_typed_without_overflow() {
        // A body nested far past the depth cap must error with a TYPED error
        // (never overflow the recursive `from_value` compiler). `serde_json`'s
        // own recursion limit may reject the string as `NotJson` before our
        // `from_value` depth guard runs — both are typed and non-panicking, which
        // is the invariant under test. The next test exercises our guard directly.
        let depth = super::super::limits::MAX_NESTING_DEPTH + 5;
        let mut body = String::new();
        for _ in 0..depth {
            body.push_str(r#"{ "type": "object", "properties": { "a": "#);
        }
        body.push_str(r#"{ "type": "string" }"#);
        for _ in 0..depth {
            body.push_str(" } }");
        }
        match WorkflowSchema::parse(&body) {
            Err(SchemaDefError::TooDeep { .. }) | Err(SchemaDefError::NotJson(_)) => {}
            other => panic!("expected a typed depth/json error, got {other:?}"),
        }
    }

    #[test]
    fn from_value_depth_guard_fires_before_overflow() {
        // Drive `from_value` directly with a pre-built `Value` (bypassing
        // serde's string-parser recursion limit) nested past our cap via the
        // `items` keyword, so OUR `TooDeep` guard is the one that fires.
        let limit = super::super::limits::MAX_NESTING_DEPTH;
        let mut node = serde_json::json!({ "type": "string" });
        for _ in 0..(limit + 5) {
            node = serde_json::json!({ "type": "array", "items": node });
        }
        match WorkflowSchema::from_value(&node, "", 0) {
            Err(SchemaDefError::TooDeep { limit: l, .. }) => assert_eq!(l, limit),
            other => panic!("expected TooDeep from the from_value guard, got {other:?}"),
        }
    }
}
