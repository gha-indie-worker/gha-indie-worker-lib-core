#![forbid(unsafe_code)]

pub mod analysis;
pub mod build_logs;
pub mod config;
pub mod connection;
#[cfg(feature = "embedded-schemas")]
pub mod contracts;
pub mod error;
pub mod flavor;
pub mod locks;
pub mod onboarding;
pub mod protocol;
#[cfg(feature = "db")]
pub mod query;
pub mod queue;
pub mod runtime;
pub mod runtime_config;
pub mod schema;
pub mod validation;

pub use build_logs::{
    plan_buffer_action, BufferAction, BufferPolicy, BufferPolicyError, LogAdmission, LogCursor,
    LogCursorError, LogPosition,
};
pub use config::CoreConfig;
pub use connection::CorePool;
pub use error::CoreError;
pub use flavor::DatabaseFlavor;
pub use queue::{
    check_eligibility, compare_candidates, effective_priority, select_next, EligibilityError,
    JobRequirements, LeaseAuthority, QueueCandidate, QueueState, SchedulingPolicy, TrustTier,
    WorkerSnapshot,
};
pub use schema::SCHEMA_REVISION;
