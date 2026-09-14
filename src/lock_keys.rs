//! Canonical distributed lock identities for GHA Indie Worker.
//!
//! The durable job queue owns claim state and fencing generations. These keys
//! are for cross-process orchestration around that state, and use the fleet
//! `ores-locks-and-leases::LockKey` type instead of inventing another identity
//! format in this repository.

use ores_locks_and_leases::LockKey;

fn key(domain: &str, name: &str) -> LockKey {
    LockKey::new(format!("gha-indie-worker/{domain}/{name}"))
        .expect("validated worker lock identity must fit LockKey")
}

/// Fence orchestration for one exact immutable job/revision identity.
pub fn job_execution(job_id: &str, revision_sha: &str) -> LockKey {
    key("jobs", &format!("execute:{job_id}:{revision_sha}"))
}

/// Serialize mutation of one local ores-compose session generation.
pub fn compose_session(session_id: &str) -> LockKey {
    key("sessions", &format!("compose:{session_id}"))
}

/// Single fleet maintenance/reaper operation.
pub fn singleton_job(job: &str) -> LockKey {
    key("maintenance", &format!("singleton:{job}"))
}

/// One schema migration runner for the durable queue/control-plane database.
pub fn migration() -> LockKey {
    key("migrations", "apply")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_revision_is_part_of_execution_identity() {
        let a = job_execution("job-7", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = job_execution("job-7", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("gha-indie-worker/jobs/execute:job-7:"));
    }
}
