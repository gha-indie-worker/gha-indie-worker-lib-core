#![forbid(unsafe_code)]

//! The contract documents, embedded and ready to validate against.
//!
//! Available under the `embedded-schemas` feature, which is **on by default**.
//! Turn it off (`--no-default-features --features read-only`) for a build that
//! must not depend on `gha-indie-worker-interfaces` at all.
//!
//! # Why the `include_str!` is not here
//!
//! The bytes come from `gha_indie_worker_interfaces::v1::schemas`, where the
//! `include_str!` calls sit next to the files they read. That keeps one copy of
//! each document, makes `cargo` rebuild this crate when a schema changes, and
//! avoids a relative path from one repository into another — which would break
//! the moment either is checked out on its own. This module re-exports them and
//! adds the parsed form plus the validation entry points.
//!
//! # What is authoritative
//!
//! JSON Schema is the *runtime* authority: nothing parses TypeSpec at runtime.
//! That does not make it the senior authority. `contracts/typespec/<slice>.tsp`
//! is its independent peer at authoring time, and
//! `npx ores-contracts check` is what proves the two agree. **Neither is
//! generated from the other.**

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::validation::{validate_def, Violation};

pub use gha_indie_worker_interfaces::v1::schemas::{
    schema_for as raw_schema_for, Slice, ALL as SLICES, CHAT, EMBEDDINGS, ERRORS, IDENTITY,
    ONBOARDING, RUNS, SYNC, TRANSPORT, WEBHOOKS, WORKERS,
};

/// Why a document could not be checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    /// No slice by that name is embedded.
    UnknownSlice(String),
    /// The embedded document is not parseable JSON. Only reachable if a schema
    /// was corrupted in the dependency, which the interfaces crate's own tests
    /// would have caught first.
    Unparseable { slice: String, detail: String },
    /// The instance failed validation.
    Invalid {
        slice: String,
        model: String,
        violations: Vec<Violation>,
    },
}

impl core::fmt::Display for ContractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ContractError::UnknownSlice(name) => write!(f, "no embedded contract slice {name}"),
            ContractError::Unparseable { slice, detail } => {
                write!(f, "embedded schema {slice} is not valid JSON: {detail}")
            }
            ContractError::Invalid {
                slice,
                model,
                violations,
            } => write!(
                f,
                "{slice}.{model} is invalid: {}",
                violations
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        }
    }
}

impl std::error::Error for ContractError {}

fn parsed() -> &'static HashMap<&'static str, Value> {
    static PARSED: OnceLock<HashMap<&'static str, Value>> = OnceLock::new();
    PARSED.get_or_init(|| {
        SLICES
            .iter()
            .filter_map(|slice| {
                serde_json::from_str::<Value>(slice.schema)
                    .ok()
                    .map(|doc| (slice.name, doc))
            })
            .collect()
    })
}

/// The parsed JSON Schema document for a slice.
///
/// Parsed once per process and cached; the cost of a validation is the walk, not
/// the parse.
///
/// # Errors
///
/// [`ContractError::UnknownSlice`] when the name is not one of the ten, and
/// [`ContractError::Unparseable`] if the embedded bytes are not JSON.
pub fn document(slice: &str) -> Result<&'static Value, ContractError> {
    if raw_schema_for(slice).is_none() {
        return Err(ContractError::UnknownSlice(slice.to_owned()));
    }
    parsed()
        .get(slice)
        .ok_or_else(|| ContractError::Unparseable {
            slice: slice.to_owned(),
            detail: "document did not parse at first use".to_owned(),
        })
}

/// Validate `instance` against `<slice>` / `$defs/<model>`.
///
/// This is the call a server makes at its HTTP, websocket, TCP or NATS edge
/// before it trusts a body.
///
/// # Errors
///
/// [`ContractError::UnknownSlice`], [`ContractError::Unparseable`], or
/// [`ContractError::Invalid`] carrying every violation.
pub fn validate(slice: &str, model: &str, instance: &Value) -> Result<(), ContractError> {
    let document = document(slice)?;
    validate_def(document, model, instance).map_err(|violations| ContractError::Invalid {
        slice: slice.to_owned(),
        model: model.to_owned(),
        violations,
    })
}

/// Every model name in a slice, sorted. Enums are excluded.
///
/// # Errors
///
/// As [`document`].
pub fn models(slice: &str) -> Result<Vec<String>, ContractError> {
    let document = document(slice)?;
    let mut names: Vec<String> = document
        .get("$defs")
        .and_then(Value::as_object)
        .map(|defs| {
            defs.iter()
                .filter(|(_, def)| def.get("enum").is_none())
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    Ok(names)
}

/// The names of every embedded slice, in contract order.
#[must_use]
pub fn slices() -> Vec<&'static str> {
    SLICES.iter().map(|s| s.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn all_ten_slices_are_embedded_and_parse() {
        assert_eq!(slices().len(), 10);
        for name in slices() {
            let document = document(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(document.get("$defs").is_some(), "{name}");
        }
    }

    #[test]
    fn an_unknown_slice_is_a_typed_error() {
        assert_eq!(
            validate("nope", "Org", &json!({})).unwrap_err(),
            ContractError::UnknownSlice("nope".to_owned())
        );
    }

    #[test]
    fn a_real_org_validates_and_a_broken_one_does_not() {
        let org = json!({
            "id": "00000001-1111-4222-8333-444455556666",
            "slug": "indie-labs",
            "displayName": "Indie Labs",
            "seatLimit": 25,
            "createdAt": "2026-03-14T09:26:53Z"
        });
        assert!(validate("identity", "Org", &org).is_ok());

        let mut broken = org.clone();
        broken.as_object_mut().unwrap().remove("displayName");
        let err = validate("identity", "Org", &broken).unwrap_err();
        let ContractError::Invalid { violations, .. } = err else {
            panic!("expected Invalid");
        };
        assert_eq!(violations[0].rule, "required");
    }

    #[test]
    fn sealed_models_reject_unknown_properties() {
        let seat = json!({
            "id": "00000005-1111-4222-8333-444455556666",
            "orgId": "00000001-1111-4222-8333-444455556666",
            "status": "active",
            "tier": "gold"
        });
        let err = validate("identity", "Seat", &seat).unwrap_err();
        assert!(err.to_string().contains("additional-properties"));
    }

    #[test]
    fn transport_unions_are_enforced_by_one_of() {
        let base = json!({
            "id": "00000080-1111-4222-8333-444455556666",
            "kind": "subscribe-run-logs",
            "connectionId": "00000081-1111-4222-8333-444455556666",
            "correlationId": "00000082-1111-4222-8333-444455556666",
            "clientSequence": 1,
            "sentAt": "2026-03-14T09:26:53Z"
        });
        // subscribe-run-logs requires runId
        assert!(validate("transport", "WsCommand", &base).is_err());
        let mut with_run = base.clone();
        with_run.as_object_mut().unwrap().insert(
            "runId".into(),
            json!("00000021-1111-4222-8333-444455556666"),
        );
        assert!(validate("transport", "WsCommand", &with_run).is_ok());
    }

    #[test]
    fn model_lists_come_from_the_embedded_document() {
        let names = models("errors").unwrap();
        assert_eq!(names, vec!["Problem", "ProblemViolation"]);
        assert!(!models("embeddings").unwrap().is_empty());
    }

    #[test]
    fn every_embedded_model_is_reachable_by_name() {
        for name in slices() {
            for model in models(name).unwrap() {
                // an empty object fails validation, but it must fail with
                // violations rather than UnknownSlice/Unparseable
                match validate(name, &model, &json!({})) {
                    Err(ContractError::Invalid { .. }) | Ok(()) => {}
                    other => panic!("{name}.{model}: {other:?}"),
                }
            }
        }
    }
}
