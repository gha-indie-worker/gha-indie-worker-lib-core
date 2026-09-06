#![forbid(unsafe_code)]

//! A runtime JSON Schema validator over the fixed subset the fleet's contract
//! authorities may use.
//!
//! Pure Rust. The only dependency is `serde_json` for the value model — there is
//! no regex crate, no schema compiler and no network. That matters because this
//! validator runs on the request path of every server and inside the desktop and
//! CLI clients.
//!
//! # The subset
//!
//! | keyword | notes |
//! |---|---|
//! | `$ref` | `#/$defs/<Name>` only |
//! | `type` | `object`, `array`, `string`, `integer`, `number`, `boolean`, `null`; a list means "any of" |
//! | `required`, `properties` | |
//! | `additionalProperties` | `false`, or a schema applied to every unlisted property |
//! | `enum`, `const` | |
//! | `items`, `minItems`, `maxItems` | |
//! | `minLength`, `maxLength` | counted in Unicode scalar values, like JSON Schema |
//! | `minimum`, `maximum` | |
//! | `pattern` | the pattern subset described below |
//! | `oneOf` | exactly one branch must match |
//! | `format` | `uuid`, `date-time`, `date`, `byte` |
//!
//! Annotations (`title`, `description`, `$comment`, `default`, `examples`,
//! `deprecated`, `$schema`, `$id`, `$defs`) are ignored, and so is any
//! `x-ores-*` vendor keyword. **Every other keyword is a violation**: the
//! validator fails closed rather than silently accepting a constraint it does
//! not implement.
//!
//! # The pattern subset
//!
//! `^` `$` anchors, literal characters, `.`, character classes `[a-z0-9_-]` and
//! `[^@ ]` (ranges, negation, a literal `-` at either edge), the escapes `\d`
//! `\D` `\w` `\W` `\s` `\S` and `\<punct>`, and the postfix quantifiers `*`,
//! `+`, `?`. No groups, no alternation, no `{n,m}` — the toolkit's TypeSpec
//! parser cannot carry braces inside a model body either, so the authorities
//! never use them. A pattern outside the subset produces an
//! `unsupported-pattern` violation on every instance, which is loud and safe.
//!
//! # Relationship to the other validators
//!
//! `gha-indie-worker-interfaces` ships the same subset twice more:
//! `scripts/validate-fixtures.mjs` (a dependency-free Node implementation, plus
//! an ajv engine) and `typescript/src/validate.mjs` here. The lane regenerates
//! `src/validation/generated/` from **both** contract authorities and
//! byte-compares the result; see that directory's README.

use std::fmt;

use serde_json::Value;

/// One reason an instance failed. Pointers are RFC 6901 JSON pointers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub pointer: String,
    pub rule: &'static str,
    pub message: String,
}

impl Violation {
    fn new(pointer: &str, rule: &'static str, message: impl Into<String>) -> Self {
        Self {
            pointer: if pointer.is_empty() {
                "/".to_owned()
            } else {
                pointer.to_owned()
            },
            rule,
            message: message.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}]: {}", self.pointer, self.rule, self.message)
    }
}

const ANNOTATIONS: [&str; 9] = [
    "title",
    "description",
    "$comment",
    "default",
    "examples",
    "deprecated",
    "$schema",
    "$id",
    "$defs",
];

const KEYWORDS: [&str; 17] = [
    "$ref",
    "type",
    "required",
    "properties",
    "additionalProperties",
    "enum",
    "const",
    "items",
    "minItems",
    "maxItems",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "pattern",
    "oneOf",
    "format",
];

/// Validate `instance` against `schema`, resolving `#/$defs/*` inside `schema`.
///
/// # Errors
///
/// Returns every violation found, in document order, so an API can render a full
/// problem document rather than one error at a time.
pub fn validate(schema: &Value, instance: &Value) -> Result<(), Vec<Violation>> {
    let mut out = Vec::new();
    node(schema, instance, schema, "", &mut out, 0);
    if out.is_empty() {
        Ok(())
    } else {
        Err(out)
    }
}

/// Validate `instance` against `#/$defs/<name>` of `document`.
///
/// # Errors
///
/// Returns a single `unknown-definition` violation when `name` is not in
/// `$defs`, otherwise every violation the definition produced.
pub fn validate_def(document: &Value, name: &str, instance: &Value) -> Result<(), Vec<Violation>> {
    let Some(def) = document.get("$defs").and_then(|d| d.get(name)) else {
        return Err(vec![Violation::new(
            "",
            "unknown-definition",
            format!("schema has no $defs/{name}"),
        )]);
    };
    let mut out = Vec::new();
    node(def, instance, document, "", &mut out, 0);
    if out.is_empty() {
        Ok(())
    } else {
        Err(out)
    }
}

const MAX_DEPTH: usize = 64;

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_f64() && n.as_f64().is_some_and(|f| f.fract() != 0.0) {
                "number"
            } else {
                "integer"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(expected: &str, actual: &str) -> bool {
    expected == actual || (expected == "number" && actual == "integer")
}

fn resolve<'a>(
    schema: &'a Value,
    root: &'a Value,
    at: &str,
    out: &mut Vec<Violation>,
) -> Option<&'a Value> {
    let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
        return Some(schema);
    };
    let Some(name) = reference.strip_prefix("#/$defs/") else {
        out.push(Violation::new(
            at,
            "unsupported-ref",
            format!("only #/$defs/<Name> references are supported, got {reference}"),
        ));
        return None;
    };
    match root.get("$defs").and_then(|d| d.get(name)) {
        Some(target) => Some(target),
        None => {
            out.push(Violation::new(
                at,
                "unknown-definition",
                format!("$ref points at missing $defs/{name}"),
            ));
            None
        }
    }
}

#[allow(clippy::too_many_lines)]
fn node(
    schema: &Value,
    instance: &Value,
    root: &Value,
    at: &str,
    out: &mut Vec<Violation>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        out.push(Violation::new(at, "depth", "schema nesting is too deep"));
        return;
    }
    match schema {
        Value::Bool(true) => return,
        Value::Bool(false) => {
            out.push(Violation::new(at, "false-schema", "nothing validates here"));
            return;
        }
        Value::Object(_) => {}
        _ => {
            out.push(Violation::new(
                at,
                "malformed-schema",
                "schema must be an object or a boolean",
            ));
            return;
        }
    }

    let Some(schema) = resolve(schema, root, at, out) else {
        return;
    };
    let Some(map) = schema.as_object() else {
        out.push(Violation::new(
            at,
            "malformed-schema",
            "resolved schema is not an object",
        ));
        return;
    };

    for key in map.keys() {
        if !KEYWORDS.contains(&key.as_str())
            && !ANNOTATIONS.contains(&key.as_str())
            && !key.starts_with("x-ores-")
        {
            out.push(Violation::new(
                at,
                "unsupported-keyword",
                format!("{key} is outside the supported subset"),
            ));
        }
    }

    let actual = kind_of(instance);
    if let Some(expected) = map.get("type") {
        let ok = match expected {
            Value::String(t) => type_matches(t, actual),
            Value::Array(list) => list
                .iter()
                .filter_map(Value::as_str)
                .any(|t| type_matches(t, actual)),
            _ => {
                out.push(Violation::new(
                    at,
                    "malformed-schema",
                    "type must be a string or a list",
                ));
                return;
            }
        };
        if !ok {
            out.push(Violation::new(
                at,
                "type",
                format!("expected {expected}, got {actual}"),
            ));
            return;
        }
    }

    if let Some(expected) = map.get("const") {
        if expected != instance {
            out.push(Violation::new(at, "const", format!("expected {expected}")));
        }
    }
    if let Some(Value::Array(options)) = map.get("enum") {
        if !options.iter().any(|o| o == instance) {
            out.push(Violation::new(
                at,
                "enum",
                format!("{instance} is not one of {}", Value::Array(options.clone())),
            ));
        }
    }

    match instance {
        Value::String(text) => string_rules(map, text, at, out),
        Value::Number(_) => number_rules(map, instance, at, out),
        Value::Array(items) => array_rules(map, items, root, at, out, depth),
        Value::Object(_) => object_rules(map, instance, root, at, out, depth),
        _ => {}
    }

    if let Some(Value::Array(branches)) = map.get("oneOf") {
        let matched = branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| {
                let mut scratch = Vec::new();
                node(branch, instance, root, at, &mut scratch, depth + 1);
                scratch.is_empty()
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if matched.len() != 1 {
            out.push(Violation::new(
                at,
                "one-of",
                format!(
                    "expected exactly one of {} branches to match, {} did",
                    branches.len(),
                    matched.len()
                ),
            ));
        }
    }
}

fn string_rules(
    map: &serde_json::Map<String, Value>,
    text: &str,
    at: &str,
    out: &mut Vec<Violation>,
) {
    let len = text.chars().count();
    if let Some(min) = map.get("minLength").and_then(Value::as_u64) {
        if (len as u64) < min {
            out.push(Violation::new(
                at,
                "min-length",
                format!("shorter than {min}"),
            ));
        }
    }
    if let Some(max) = map.get("maxLength").and_then(Value::as_u64) {
        if (len as u64) > max {
            out.push(Violation::new(
                at,
                "max-length",
                format!("longer than {max}"),
            ));
        }
    }
    if let Some(pattern) = map.get("pattern").and_then(Value::as_str) {
        match crate::validation::pattern::Pattern::parse(pattern) {
            Ok(compiled) => {
                if !compiled.is_match(text) {
                    out.push(Violation::new(
                        at,
                        "pattern",
                        format!("does not match {pattern}"),
                    ));
                }
            }
            Err(reason) => out.push(Violation::new(
                at,
                "unsupported-pattern",
                format!("{pattern}: {reason}"),
            )),
        }
    }
    if let Some(format) = map.get("format").and_then(Value::as_str) {
        let ok = match format {
            "uuid" => is_uuid(text),
            "date-time" => is_date_time(text),
            "date" => is_date(text),
            "byte" => is_base64(text),
            _ => {
                out.push(Violation::new(
                    at,
                    "unsupported-format",
                    format!("{format} is outside the supported subset"),
                ));
                return;
            }
        };
        if !ok {
            out.push(Violation::new(
                at,
                "format",
                format!("not a valid {format}"),
            ));
        }
    }
}

fn number_rules(
    map: &serde_json::Map<String, Value>,
    instance: &Value,
    at: &str,
    out: &mut Vec<Violation>,
) {
    let Some(value) = instance.as_f64() else {
        return;
    };
    if let Some(min) = map.get("minimum").and_then(Value::as_f64) {
        if value < min {
            out.push(Violation::new(at, "minimum", format!("below {min}")));
        }
    }
    if let Some(max) = map.get("maximum").and_then(Value::as_f64) {
        if value > max {
            out.push(Violation::new(at, "maximum", format!("above {max}")));
        }
    }
}

fn array_rules(
    map: &serde_json::Map<String, Value>,
    items: &[Value],
    root: &Value,
    at: &str,
    out: &mut Vec<Violation>,
    depth: usize,
) {
    if let Some(min) = map.get("minItems").and_then(Value::as_u64) {
        if (items.len() as u64) < min {
            out.push(Violation::new(
                at,
                "min-items",
                format!("fewer than {min} items"),
            ));
        }
    }
    if let Some(max) = map.get("maxItems").and_then(Value::as_u64) {
        if (items.len() as u64) > max {
            out.push(Violation::new(
                at,
                "max-items",
                format!("more than {max} items"),
            ));
        }
    }
    if let Some(item_schema) = map.get("items") {
        for (i, item) in items.iter().enumerate() {
            node(
                item_schema,
                item,
                root,
                &format!("{at}/{i}"),
                out,
                depth + 1,
            );
        }
    }
}

fn object_rules(
    map: &serde_json::Map<String, Value>,
    instance: &Value,
    root: &Value,
    at: &str,
    out: &mut Vec<Violation>,
    depth: usize,
) {
    let Some(object) = instance.as_object() else {
        return;
    };
    if let Some(Value::Array(required)) = map.get("required") {
        for name in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(name) {
                out.push(Violation::new(
                    &format!("{at}/{}", escape_pointer(name)),
                    "required",
                    format!("{name} is required"),
                ));
            }
        }
    }
    let properties = map.get("properties").and_then(Value::as_object);
    for (name, child) in object {
        let pointer = format!("{at}/{}", escape_pointer(name));
        match properties.and_then(|p| p.get(name)) {
            Some(child_schema) => node(child_schema, child, root, &pointer, out, depth + 1),
            None => match map.get("additionalProperties") {
                Some(Value::Bool(false)) => out.push(Violation::new(
                    &pointer,
                    "additional-properties",
                    format!("{name} is not an allowed property"),
                )),
                Some(other) => node(other, child, root, &pointer, out, depth + 1),
                None => {}
            },
        }
    }
}

fn escape_pointer(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn is_uuid(text: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = text.split('-');
    for expected in groups {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.len() != expected || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
    }
    parts.next().is_none()
}

fn digits(text: &str, count: usize) -> bool {
    text.len() == count && text.bytes().all(|b| b.is_ascii_digit())
}

fn is_date(text: &str) -> bool {
    let mut parts = text.split('-');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(y), Some(m), Some(d), None) if digits(y, 4) && digits(m, 2) && digits(d, 2)
    )
}

fn is_time_offset(text: &str) -> bool {
    if text == "Z" || text == "z" {
        return true;
    }
    let Some(rest) = text.strip_prefix(['+', '-']) else {
        return false;
    };
    let mut parts = rest.split(':');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(h), Some(m), None) if digits(h, 2) && digits(m, 2)
    )
}

fn is_date_time(text: &str) -> bool {
    let mut halves = text.splitn(2, ['T', 't']);
    let (Some(date), Some(rest)) = (halves.next(), halves.next()) else {
        return false;
    };
    if !is_date(date) {
        return false;
    }
    let split = rest.find(['Z', 'z', '+']).or_else(|| {
        // a '-' in the time half can only be the offset sign
        rest.rfind('-').filter(|i| *i > 0)
    });
    let Some(split) = split else {
        return false;
    };
    let (clock, offset) = rest.split_at(split);
    let mut fractioned = clock.splitn(2, '.');
    let hms = fractioned.next().unwrap_or_default();
    if let Some(fraction) = fractioned.next() {
        if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let mut parts = hms.split(':');
    let ok = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(h), Some(m), Some(s), None) if digits(h, 2) && digits(m, 2) && digits(s, 2)
    );
    ok && is_time_offset(offset)
}

fn is_base64(text: &str) -> bool {
    let body = text.trim_end_matches('=');
    text.len() - body.len() <= 2
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "$defs": {
                "Role": { "type": "string", "enum": ["owner", "admin"] },
                "Org": {
                    "type": "object",
                    "additionalProperties": false,
                    "x-ores-table": "orgs",
                    "required": ["id", "slug", "seatLimit"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "slug": { "type": "string", "maxLength": 8, "pattern": "^[a-z0-9-]+$" },
                        "seatLimit": { "type": "integer", "minimum": 1, "maximum": 10 },
                        "role": { "$ref": "#/$defs/Role" },
                        "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 2 },
                        "createdAt": { "type": "string", "format": "date-time" }
                    }
                }
            }
        })
    }

    fn org() -> Value {
        json!({
            "id": "00000001-1111-4222-8333-444455556666",
            "slug": "indie",
            "seatLimit": 5
        })
    }

    fn rules(result: Result<(), Vec<Violation>>) -> Vec<&'static str> {
        result
            .err()
            .unwrap_or_default()
            .iter()
            .map(|v| v.rule)
            .collect()
    }

    #[test]
    fn a_valid_instance_passes() {
        assert!(validate_def(&schema(), "Org", &org()).is_ok());
    }

    #[test]
    fn missing_required_property_is_reported_with_a_pointer() {
        let mut instance = org();
        instance.as_object_mut().unwrap().remove("slug");
        let violations = validate_def(&schema(), "Org", &instance).unwrap_err();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "required");
        assert_eq!(violations[0].pointer, "/slug");
    }

    #[test]
    fn sealed_objects_reject_extra_properties() {
        let mut instance = org();
        instance
            .as_object_mut()
            .unwrap()
            .insert("tier".into(), json!("gold"));
        assert_eq!(
            rules(validate_def(&schema(), "Org", &instance)),
            vec!["additional-properties"]
        );
    }

    #[test]
    fn scalar_rules_are_enforced() {
        let mut instance = org();
        let object = instance.as_object_mut().unwrap();
        object.insert("slug".into(), json!("WAY-TOO-LONG"));
        object.insert("seatLimit".into(), json!(99));
        let found = rules(validate_def(&schema(), "Org", &instance));
        assert!(found.contains(&"max-length"));
        assert!(found.contains(&"pattern"));
        assert!(found.contains(&"maximum"));
    }

    #[test]
    fn refs_resolve_into_defs() {
        let mut instance = org();
        instance
            .as_object_mut()
            .unwrap()
            .insert("role".into(), json!("member"));
        assert_eq!(
            rules(validate_def(&schema(), "Org", &instance)),
            vec!["enum"]
        );
        instance
            .as_object_mut()
            .unwrap()
            .insert("role".into(), json!("admin"));
        assert!(validate_def(&schema(), "Org", &instance).is_ok());
    }

    #[test]
    fn arrays_check_items_and_bounds() {
        let mut instance = org();
        instance
            .as_object_mut()
            .unwrap()
            .insert("tags".into(), json!(["a", 2, "c"]));
        let found = rules(validate_def(&schema(), "Org", &instance));
        assert!(found.contains(&"type"));
        assert!(found.contains(&"max-items"));
    }

    #[test]
    fn unsupported_keywords_fail_closed() {
        let schema = json!({ "type": "string", "contentEncoding": "base64" });
        assert_eq!(
            rules(validate(&schema, &json!("hi"))),
            vec!["unsupported-keyword"]
        );
    }

    #[test]
    fn one_of_selects_exactly_one_branch() {
        let schema = json!({
            "type": "object",
            "properties": { "kind": { "type": "string" }, "runId": { "type": "string" } },
            "oneOf": [
                { "properties": { "kind": { "const": "subscribe" } }, "required": ["kind", "runId"] },
                { "properties": { "kind": { "const": "heartbeat" } }, "required": ["kind"] }
            ]
        });
        assert!(validate(&schema, &json!({ "kind": "heartbeat" })).is_ok());
        assert!(validate(&schema, &json!({ "kind": "subscribe", "runId": "r" })).is_ok());
        assert_eq!(
            rules(validate(&schema, &json!({ "kind": "subscribe" }))),
            vec!["one-of"]
        );
    }

    #[test]
    fn integers_and_numbers_are_distinguished() {
        assert!(validate(&json!({ "type": "integer" }), &json!(3)).is_ok());
        assert!(validate(&json!({ "type": "integer" }), &json!(3.5)).is_err());
        assert!(validate(&json!({ "type": "number" }), &json!(3)).is_ok());
        assert!(validate(&json!({ "type": "number" }), &json!(3.5)).is_ok());
    }

    #[test]
    fn formats_are_checked() {
        let uuid = json!({ "type": "string", "format": "uuid" });
        assert!(validate(&uuid, &json!("00000001-1111-4222-8333-444455556666")).is_ok());
        assert!(validate(&uuid, &json!("not-a-uuid")).is_err());

        let stamp = json!({ "type": "string", "format": "date-time" });
        assert!(validate(&stamp, &json!("2026-03-14T09:26:53Z")).is_ok());
        assert!(validate(&stamp, &json!("2026-03-14T09:26:53.123+01:00")).is_ok());
        assert!(validate(&stamp, &json!("2026-03-14 09:26:53")).is_err());
        assert!(validate(&stamp, &json!("2026-03-14T09:26:53")).is_err());

        let day = json!({ "type": "string", "format": "date" });
        assert!(validate(&day, &json!("2026-03-14")).is_ok());
        assert!(validate(&day, &json!("2026-3-14")).is_err());
    }

    #[test]
    fn additional_properties_may_be_a_schema() {
        let schema = json!({
            "type": "object",
            "additionalProperties": { "type": "integer", "minimum": 0 }
        });
        assert!(validate(&schema, &json!({ "api-1": 2 })).is_ok());
        assert!(validate(&schema, &json!({ "api-1": -1 })).is_err());
    }

    #[test]
    fn unknown_definition_is_a_violation_not_a_panic() {
        let violations = validate_def(&schema(), "Nope", &org()).unwrap_err();
        assert_eq!(violations[0].rule, "unknown-definition");
    }
}
