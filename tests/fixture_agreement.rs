//! Cross-implementation agreement: the Rust validator in this crate must reach
//! the same verdict as the JSON Schema validators in `gha-indie-worker-interfaces`
//! on every fixture in the contract corpus.
//!
//! The corpus lives in the other repository, so this test needs a checkout of it.
//! It looks in this order and **skips** (prints and returns) when it finds
//! nothing, so a solo `cargo test` in this repository still passes:
//!
//! 1. `$GHA_INDIE_WORKER_INTERFACES_DIR`
//! 2. `../gha-indie-worker-interfaces` (the monorepo / side-by-side layout)
//!
//! The rules, which are the same three the interfaces repository asserts from
//! the other side:
//!
//! | directory | this validator |
//! |---|---|
//! | `valid/` | must accept |
//! | `invalid/` | must reject |
//! | `invalid/schema-only/` | must reject — these break a value bound, which is exactly what a JSON Schema validator is for even though serde lets them through |
//!
//! Requires the `embedded-schemas` feature (on by default) because the schemas
//! come from the dependency, not from the checkout: that is the point. If the
//! embedded copy has drifted from the checked-out authority, this test fails,
//! which is the drift alarm.

#![cfg(feature = "embedded-schemas")]

use std::fs;
use std::path::{Path, PathBuf};

use gha_indie_worker_lib_core::contracts;

fn interfaces_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("GHA_INDIE_WORKER_INTERFACES_DIR") {
        let path = PathBuf::from(dir);
        if path.join("contracts/fixtures").is_dir() {
            return Some(path);
        }
    }
    let sibling = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("gha-indie-worker-interfaces");
    sibling
        .join("contracts/fixtures")
        .is_dir()
        .then_some(sibling)
}

fn json_files(dir: &Path, recurse: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recurse {
                out.extend(json_files(&path, true));
            }
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn model_of(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('.').next())
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn the_rust_validator_agrees_with_the_contract_corpus() {
    let Some(root) = interfaces_dir() else {
        println!(
            "skipping: no gha-indie-worker-interfaces checkout \
             (set GHA_INDIE_WORKER_INTERFACES_DIR or put it beside this repository)"
        );
        return;
    };
    let fixtures = root.join("contracts/fixtures");
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut failures = Vec::new();

    for slice in contracts::slices() {
        let dir = fixtures.join(slice);
        assert!(
            dir.is_dir(),
            "the embedded contract has a {slice} slice but the corpus has no {}",
            dir.display()
        );

        for path in json_files(&dir.join("valid"), false) {
            let raw = fs::read_to_string(&path).unwrap();
            let instance: serde_json::Value = serde_json::from_str(&raw).unwrap();
            match contracts::validate(slice, &model_of(&path), &instance) {
                Ok(()) => accepted += 1,
                Err(e) => failures.push(format!("{} should be VALID: {e}", path.display())),
            }
        }

        // `invalid/` and `invalid/schema-only/` are both rejections for a JSON
        // Schema validator; only serde treats them differently.
        for path in json_files(&dir.join("invalid"), true) {
            let raw = fs::read_to_string(&path).unwrap();
            let instance: serde_json::Value = serde_json::from_str(&raw).unwrap();
            match contracts::validate(slice, &model_of(&path), &instance) {
                Err(_) => rejected += 1,
                Ok(()) => failures.push(format!("{} should be INVALID but passed", path.display())),
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        accepted >= 20 && rejected >= 20,
        "expected a real corpus, saw {accepted} accepted and {rejected} rejected"
    );
    println!("[agreement] {accepted} valid + {rejected} invalid fixtures agree");
}
