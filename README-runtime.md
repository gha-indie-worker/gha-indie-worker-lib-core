# `runtime` and `analysis` — the shared server core

Two module trees, both **dependency-free** (std only) and exhaustively unit-tested, so the rules
every service shares are decided in one place and provable without a network, a database or a clock.

## `runtime` — the rules every service shares

| module | what it decides |
|---|---|
| `plane` | Which plane a process belongs to, and the boot-time invariants that keep product and admin apart. An admin process given the product database refuses to start; a product process holding an admin connection string refuses to start; the two shared-auth issuers may not be equal. |
| `authz` | Dual-realm token projection. An RFC 7662 answer becomes a `VerifiedActor` only after issuer, audience, **realm**, expiry, assurance level, subject shape and scopes all check out. The realm check is what stops a valid *customer* token from satisfying the admin API. |
| `surface` | Host-based routing: `app.` `user.` `org.` `m.` `api.` `auth.` `admin.` `admin-api.` `www.`. Look-alike hosts (`app.indiebuild.dev.evil.test`) resolve to nothing, and the admin surfaces 404 from the product binary. |
| `tenancy` | B2B and B2C onboarding: roles as a total order, a seat ledger computed from rows rather than stored as a counter, invitations that reserve a seat, verified-domain joining (off by default), and the two changes that break an org — escalating past your own role, and removing the last owner. |
| `session` | One frame vocabulary over WebSocket **and** raw TCP: length-prefixed binary framing, credit-based backpressure, an explicit `Lagged` frame instead of a silent gap, heartbeat liveness that survives a half-open socket, and exact resumption by sequence. |

## `analysis` — search, correlation and discovery

| module | what it decides |
|---|---|
| `embedding` | One 4100-slot storage shape for every model, with zero padding into the tail — safe because the three supported metrics (dot, cosine, L2) are provably invariant under it, which is why the metric set is closed. A 12-field `ComparisonSpace` makes comparing an OpenAI vector to a Qwen vector a refusal rather than a plausible number. Every space ships **disabled**; enabling one requires six pieces of evidence, and an approximate index requires a seventh. |
| `regression` | Pearson and Spearman with exact t-distribution p-values, OLS with per-coefficient standard errors, Welch's unequal-variance test, CUSUM change-point detection, and Benjamini-Hochberg FDR control. `discover()` reports how many candidates it examined and which it could not test — because a list of "significant" features without its denominator is not a result. |

## Verification

- `cargo test` — 71 unit tests, including a byte-at-a-time stream split of the session codec, a
  property check that zero-padding changes none of the three metrics, and the full matrix of
  cross-realm token refusals.
- `oracle/` — an independent Python oracle. `xcheck.rs` prints every statistic; `crosscheck.py`
  recomputes all of them with scipy/numpy. **209 values agree to 1e-9 or better**, covering the
  incomplete beta at 1 and 7.3 degrees of freedom, OLS standard errors, Welch–Satterthwaite
  degrees of freedom, and the Benjamini-Hochberg step-up. Two independent implementations agreeing
  is the evidence that the statistics are usable for regression detection; run it in CI whenever
  `analysis/regression.rs` changes.

```sh
cargo test
rustc --edition 2021 -O oracle/xcheck.rs -o /tmp/xcheck && /tmp/xcheck > /tmp/rust_out.txt
python3 oracle/crosscheck.py /tmp/rust_out.txt
```
