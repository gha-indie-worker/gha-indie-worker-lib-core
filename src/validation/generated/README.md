<!-- generated-policy: frozen -->

# `src/validation/generated/` — regenerated, never hand-written

This directory holds **machine-written validators**. Do not edit anything here.

## Where they come from

`gha-indie-worker-interfaces` owns the contract as two independent,
hand-authored authorities:

* `contracts/typespec/<slice>.tsp`
* `contracts/json-schema/<slice>.schema.json`

**Neither is generated from the other.** The runner lane regenerates this
directory from **both** of them, independently, and byte-compares the two
results:

```sh
# in the -interfaces checkout
npm ci
node scripts/contracts-slices.mjs generate      # writes generated/<slice>/**

# in this repository
scripts/sync-validators.sh ../gha-indie-worker-interfaces
```

`sync-validators.sh` copies `generated/<slice>/rust/types.rs` in under
`src/validation/generated/<slice>.rs`, once per slice, and refuses to write
anything unless the ores-contracts receipt for that slice says `status: passed`
— which is exactly the statement that the TypeSpec lane and the JSON Schema lane
produced identical bytes. A discrepancy is a finding with a stable fingerprint;
it stops the sync rather than picking a winner.

## What is hand-written, and why

`src/validation/json_schema.rs` and `src/validation/pattern.rs` are hand-written
and stay that way. They are the *engine* — a validator over the fixed keyword
subset — not the *contract*. The generated files here are the contract-shaped
part: the embedded schema documents and the typed models each slice validates
into. Keeping the engine hand-written means it can be reviewed, fuzzed and
optimised without regenerating anything, and means a contract change never
rewrites security-relevant matching code.

The same subset is implemented three more times in the fleet, on purpose, so
that a client and a server cannot disagree about whether a document is valid:

| implementation | where |
|---|---|
| Rust | `src/validation/json_schema.rs` (this crate) |
| Node, dependency-free | `gha-indie-worker-interfaces/scripts/validate-fixtures.mjs` |
| Node, ajv 2020-12 | the same script, `--engine ajv` |
| TypeScript/ESM | `typescript/src/validate.mjs` (this repository) |

Every one of them runs against the same fixture corpus in
`gha-indie-worker-interfaces/contracts/fixtures/`, and CI runs at least two
engines per language. If they ever disagree about a fixture, that is the bug.

## Until the first sync

This directory contains only this README. `src/contracts.rs` embeds the schema
documents through the `gha-indie-worker-interfaces` dependency in the meantime,
which is the same bytes by a shorter path; the generated typed models land here
when the lane first runs the sync.
