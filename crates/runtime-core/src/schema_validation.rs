//! Structural validation of a tool's structured output against its declared
//! JSON Schema (v1.3 §4.4).
//!
//! What this replaces: the previous check asked only whether every top-level
//! key of the schema object was *present* in the value. `{"findings": 3}`
//! satisfied `{"findings": {"type": "array"}}`, and a schema that declared
//! `required` or nested shapes was not consulted at all. Structured output that
//! becomes `PinnedOrigin::ToolFindings` is model-facing data produced by an
//! untrusted provider, so "the keys are there" is not a strong enough contract.
//!
//! What this is NOT: a complete JSON Schema implementation. It deliberately
//! avoids a new dependency in the authorization path and instead enforces the
//! keyword subset that expresses tool output shapes:
//!
//! | keyword                | enforced |
//! |------------------------|----------|
//! | `type`                 | yes (incl. union arrays) |
//! | `properties`           | yes, recursively |
//! | `required`             | yes |
//! | `additionalProperties` | yes, when `false` |
//! | `items`                | yes, recursively |
//! | `enum`                 | yes |
//! | `const`                | yes |
//! | `minimum` / `maximum`  | yes |
//! | `minItems` / `maxItems`| yes |
//! | `minLength`/`maxLength`| yes |
//!
//! Unrecognized keywords are IGNORED rather than treated as failures, so a
//! richer schema never produces a false rejection — the failure mode is
//! "accepts something a full validator would reject", never "rejects valid
//! output". Every rejection this does report is a genuine shape violation.

use serde_json::Value;

/// Why a structured result did not satisfy its declared schema. Carries the
/// JSON pointer to the offending location so a diagnostic can name it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaViolation {
    pub pointer: String,
    pub reason: String,
}

impl std::fmt::Display for SchemaViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pointer = if self.pointer.is_empty() {
            "(root)"
        } else {
            &self.pointer
        };
        write!(f, "{pointer}: {}", self.reason)
    }
}

/// Validate `value` against `schema`. `Ok(())` means the value satisfies every
/// enforced keyword.
pub fn validate(value: &Value, schema: &Value) -> Result<(), SchemaViolation> {
    validate_at(value, schema, String::new())
}

fn violation(pointer: &str, reason: impl Into<String>) -> SchemaViolation {
    SchemaViolation {
        pointer: pointer.to_owned(),
        reason: reason.into(),
    }
}

fn validate_at(value: &Value, schema: &Value, pointer: String) -> Result<(), SchemaViolation> {
    // `true`/`{}` accept anything; `false` accepts nothing.
    let object = match schema {
        Value::Bool(true) => return Ok(()),
        Value::Bool(false) => {
            return Err(violation(&pointer, "schema `false` rejects every value"));
        }
        Value::Object(object) => object,
        _ => return Ok(()),
    };

    if let Some(expected) = object.get("const")
        && value != expected
    {
        return Err(violation(&pointer, format!("must equal {expected}")));
    }
    if let Some(Value::Array(options)) = object.get("enum")
        && !options.contains(value)
    {
        return Err(violation(
            &pointer,
            "value is not one of the allowed `enum`",
        ));
    }
    if let Some(declared) = object.get("type")
        && !type_matches(value, declared)
    {
        return Err(violation(
            &pointer,
            format!("expected type {declared}, found {}", type_name(value)),
        ));
    }

    match value {
        Value::Object(members) => {
            if let Some(Value::Array(required)) = object.get("required") {
                for name in required.iter().filter_map(Value::as_str) {
                    if !members.contains_key(name) {
                        return Err(violation(
                            &pointer,
                            format!("required property `{name}` is missing"),
                        ));
                    }
                }
            }
            let properties = object.get("properties").and_then(Value::as_object);
            if let Some(properties) = properties {
                for (name, subschema) in properties {
                    if let Some(member) = members.get(name) {
                        validate_at(member, subschema, format!("{pointer}/{name}"))?;
                    }
                }
            }
            if object.get("additionalProperties") == Some(&Value::Bool(false)) {
                let declared = properties;
                for name in members.keys() {
                    if !declared.is_some_and(|properties| properties.contains_key(name)) {
                        return Err(violation(
                            &pointer,
                            format!("`{name}` is not an allowed property"),
                        ));
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(minimum) = object.get("minItems").and_then(Value::as_u64)
                && (items.len() as u64) < minimum
            {
                return Err(violation(
                    &pointer,
                    format!("needs at least {minimum} items"),
                ));
            }
            if let Some(maximum) = object.get("maxItems").and_then(Value::as_u64)
                && (items.len() as u64) > maximum
            {
                return Err(violation(
                    &pointer,
                    format!("allows at most {maximum} items"),
                ));
            }
            if let Some(subschema) = object.get("items") {
                for (index, item) in items.iter().enumerate() {
                    validate_at(item, subschema, format!("{pointer}/{index}"))?;
                }
            }
        }
        Value::String(text) => {
            if let Some(minimum) = object.get("minLength").and_then(Value::as_u64)
                && (text.chars().count() as u64) < minimum
            {
                return Err(violation(
                    &pointer,
                    format!("needs at least {minimum} characters"),
                ));
            }
            if let Some(maximum) = object.get("maxLength").and_then(Value::as_u64)
                && (text.chars().count() as u64) > maximum
            {
                return Err(violation(
                    &pointer,
                    format!("allows at most {maximum} characters"),
                ));
            }
        }
        Value::Number(number) => {
            let as_f64 = number.as_f64().unwrap_or_default();
            if let Some(minimum) = object.get("minimum").and_then(Value::as_f64)
                && as_f64 < minimum
            {
                return Err(violation(&pointer, format!("must be >= {minimum}")));
            }
            if let Some(maximum) = object.get("maximum").and_then(Value::as_f64)
                && as_f64 > maximum
            {
                return Err(violation(&pointer, format!("must be <= {maximum}")));
            }
        }
        _ => {}
    }
    Ok(())
}

fn type_matches(value: &Value, declared: &Value) -> bool {
    match declared {
        Value::String(name) => matches_named_type(value, name),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|name| matches_named_type(value, name)),
        _ => true,
    }
}

fn matches_named_type(value: &Value, name: &str) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        // JSON has one number type; `integer` additionally requires no
        // fractional part.
        "integer" => value.is_i64() || value.is_u64(),
        _ => true,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_old_key_presence_check_is_no_longer_enough() {
        // The exact case the previous implementation accepted: the key is
        // present, but it is the wrong type entirely.
        let schema = json!({
            "type": "object",
            "properties": { "findings": { "type": "array" } },
            "required": ["findings"]
        });
        assert!(validate(&json!({ "findings": [] }), &schema).is_ok());
        let error = validate(&json!({ "findings": 3 }), &schema).unwrap_err();
        assert_eq!(error.pointer, "/findings");
        assert!(error.reason.contains("expected type"));
    }

    #[test]
    fn required_properties_are_enforced() {
        let schema = json!({ "type": "object", "required": ["severity"] });
        assert!(validate(&json!({ "severity": "high" }), &schema).is_ok());
        assert!(validate(&json!({ "other": 1 }), &schema).is_err());
    }

    #[test]
    fn nested_shapes_and_arrays_are_validated_recursively() {
        let schema = json!({
            "type": "object",
            "properties": {
                "findings": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "required": ["file", "line"],
                        "properties": {
                            "file": { "type": "string" },
                            "line": { "type": "integer", "minimum": 1 }
                        }
                    }
                }
            }
        });
        assert!(
            validate(
                &json!({ "findings": [{ "file": "a.rs", "line": 12 }] }),
                &schema
            )
            .is_ok()
        );
        // Wrong nested type.
        let error = validate(
            &json!({ "findings": [{ "file": "a.rs", "line": "twelve" }] }),
            &schema,
        )
        .unwrap_err();
        assert_eq!(error.pointer, "/findings/0/line");
        // Missing nested required key.
        assert!(validate(&json!({ "findings": [{ "file": "a.rs" }] }), &schema).is_err());
        // Empty array violates minItems.
        assert!(validate(&json!({ "findings": [] }), &schema).is_err());
    }

    #[test]
    fn enum_const_and_bounds() {
        let schema = json!({ "enum": ["low", "high"] });
        assert!(validate(&json!("low"), &schema).is_ok());
        assert!(validate(&json!("medium"), &schema).is_err());
        assert!(validate(&json!(5), &json!({ "type": "number", "maximum": 4 })).is_err());
        assert!(validate(&json!("abc"), &json!({ "maxLength": 2 })).is_err());
        assert!(validate(&json!("v1"), &json!({ "const": "v1" })).is_ok());
    }

    #[test]
    fn additional_properties_false_rejects_extras() {
        let schema = json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "additionalProperties": false
        });
        assert!(validate(&json!({ "ok": true }), &schema).is_ok());
        assert!(validate(&json!({ "ok": true, "sneaky": 1 }), &schema).is_err());
    }

    #[test]
    fn unknown_keywords_never_cause_a_false_rejection() {
        // A schema using keywords this subset does not implement must still
        // accept conforming data rather than failing closed on the keyword.
        let schema = json!({
            "type": "object",
            "patternProperties": { "^x-": { "type": "string" } },
            "properties": { "id": { "type": "string", "format": "uuid" } }
        });
        assert!(validate(&json!({ "id": "not-a-uuid" }), &schema).is_ok());
    }

    #[test]
    fn integer_type_rejects_fractional_numbers() {
        assert!(validate(&json!(3), &json!({ "type": "integer" })).is_ok());
        assert!(validate(&json!(3.5), &json!({ "type": "integer" })).is_err());
    }
}
