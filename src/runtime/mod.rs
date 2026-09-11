//! Server-side runtime: the rules every `gha-indie-worker` service shares.
//!
//! Everything in here is a pure function or a plain value. The effectful bindings — the HTTP
//! client that introspects a token, the SeaORM pool, the axum router — live in the server crates,
//! so the rules can be exhaustively tested without a network, a database or a clock.

pub mod authz;
pub mod plane;
pub mod session;
pub mod surface;
pub mod tenancy;
