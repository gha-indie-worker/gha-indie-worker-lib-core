# Lease authority fail-closed finite model

The production pure Rust lease policy is `LeaseAuthority` in `src/queue.rs`.
The only generated initial authority is the positive triple (1,1,1).
Reclaimed generations, fencing tokens and attempts must all remain strictly
positive and increment without overflow. A zero-valued fabricated authority
used to authorize an identical zero-valued presented triple; a zero-valued
persisted authority could also be reclaimed into apparently valid positive
values. Both operations now reject invalid zero triples.

The Rust finite test explores every combination of authority and presented
generation/fence/attempt from {0,1,2} (3^6 = 729 direct checks) plus bounded
exhaustion at maximum integer counters. This checks the actual production
pure functions, NOT the atomicity of SQL claims or network admission.

`src/atto/lease_fencing.atto` is a pure source-only policy projection,
not AttoLang guest execution. A reviewer must validate the storage-side
authority origin and exact-head stepful CI before promotion. Zero-job
workflows remain NOT_EXECUTED.
