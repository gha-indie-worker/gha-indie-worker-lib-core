# gha-indie-worker-lib-core

Shared client and server code for the GHA Indie Worker control plane: a
GitHub-Actions-compatible worker (`gha-indie-worker.rs`) and a fail-closed
workflow planner (`gha-clone-server.rs`).

**This crate is client-safe by default.** Nothing in the default feature set
opens a socket, touches a database or needs an async runtime, so the CLI, the
desktop app and the WASM front end can link it. Servers opt into `db`.

## lib-core vs orm-core vs pub-lib-core

Three repositories are easy to confuse. The line between them is what may
*depend* on each, and what each is allowed to *do*.

| repository | who depends on it | what it owns | what it must never do |
|---|---|---|---|
| **`gha-indie-worker-lib-core`** (this one) | every server, the CLI, the desktop app, the WASM front end | runtime validation, the onboarding state machines, the frame codecs, the shared runtime config, the locking boundary, query **builders** | own entities, execute SQL, run migrations, ship to an untrusted browser context |
| `gha-indie-worker-orm-core` | servers only | SeaORM entities and the reviewed write operations | expose a raw connection, or accept ad-hoc SQL from a caller |
| `gha-indie-worker-pub-lib-core` | anything public — the marketing site, unauthenticated widgets | the subset that is safe with no actor at all | know a database URL, an internal host, or any private contract slice |

Two more boundaries complete the picture:

* `declarative-migrations/declarative-migrations` is the schema authority.
  Services **never run DDL at boot** — that is a fleet contract, not a
  preference. This crate has query builders and no migrations, on purpose.
* `gha-indie-worker-interfaces` owns the contract as two independent
  authorities. This crate embeds the JSON Schema half and validates against it.

`admin-orm/` inside this repository is its own workspace: the named
administrative surface for the isolated admin plane. It is unaffected by
everything above.

## Modules

| module | feature | what it is |
|---|---|---|
| `validation` | always | a JSON Schema validator over a fixed keyword subset, plus the small pattern matcher it uses. Pure Rust, no regex crate, no allocations beyond the walk. |
| `contracts` | `embedded-schemas` (default **on**) | the ten contract slices, embedded through `gha-indie-worker-interfaces`, and `validate(slice, model, value)` |
| `onboarding` | always | the org and user state machines as total functions over an explicit edge table |
| `protocol` | always | length-prefixed JSON framing for the TCP avenue, and the websocket sequencing and resume rules |
| `runtime_config` | always | every `GHA_INDIE_WORKER_*` variable, with a `Debug` that redacts every secret |
| `locks` | always | the `LockProvider` boundary that `oresoftware/ores-locks-and-leases` fulfils, and the fiducia + pg-advisory composition |
| `query` | `db` (default **off**) | read builders returning `sea_orm::Statement` and `Select` fragments for orm-core entities |

## Features

```toml
# a client: state machines, codecs, validation — no database, no async runtime
gha-indie-worker-lib-core = { git = "…" }

# a server
gha-indie-worker-lib-core = { git = "…", features = ["db", "read-write"] }

# a build that must not depend on the interfaces crate at all
gha-indie-worker-lib-core = { git = "…", default-features = false, features = ["read-only"] }
```

| feature | default | effect |
|---|---|---|
| `read-only` | ✅ | the read-only posture of `config` / `connection` |
| `read-write` | | write-capable posture; implies `read-only` |
| `migrate` | | implies `read-write`. Still never runs DDL at boot. |
| `embedded-schemas` | ✅ | the `contracts` module; pulls in `gha-indie-worker-interfaces` |
| `db` | | the `query` module; pulls in SeaORM |

## Contract validation

JSON Schema is the *runtime* authority — nothing parses TypeSpec at runtime.
That does not make it senior: `contracts/typespec/<slice>.tsp` in the interfaces
repository is its independent peer at authoring time, and **neither is generated
from the other**. `npx ores-contracts check` is what proves they agree.

```rust
use gha_indie_worker_lib_core::contracts;
use serde_json::json;

let org = json!({
    "id": "00000001-1111-4222-8333-444455556666",
    "slug": "indie-labs",
    "displayName": "Indie Labs",
    "seatLimit": 25,
    "createdAt": "2026-03-14T09:26:53Z"
});
contracts::validate("identity", "Org", &org)?;
# Ok::<(), contracts::ContractError>(())
```

The same keyword subset is implemented four times across the fleet — here, twice
in `gha-indie-worker-interfaces/scripts/validate-fixtures.mjs` (dependency-free
and ajv), and once in `typescript/src/validate.mjs`. That redundancy is the
point: a client and a server must never disagree about whether a document is
valid, and `tests/fixture_agreement.rs` runs this validator over the whole
fixture corpus to prove it.

## Polyglot siblings

Small on purpose, and real: each one carries the onboarding transition table so a
UI can reason about it without a round trip, and each has tests that fail if the
table drifts from `src/onboarding.rs`.

| directory | runtime | tests |
|---|---|---|
| `typescript/` | ESM, zero dependencies | `node --test test/*.test.mjs` |
| `dart/` | Dart 3.5+, for the Flutter clients | `dart test` |
| `gleam/` | Erlang target | `gleam test` |

`typescript/` also carries the validator and the frame codec, because the web and
Node clients need both.

## Development

```sh
cargo test                                    # client-safe surface
cargo test --features db                      # plus the query builders
cargo test --no-default-features --features read-only
.ores-lint/lint.sh                            # every linter available on this machine

# regenerate src/validation/generated/ from BOTH contract authorities
scripts/sync-validators.sh ../gha-indie-worker-interfaces
```
