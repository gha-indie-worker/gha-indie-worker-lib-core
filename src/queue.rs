//! Pure durable-queue policy used by the pull-worker control plane.
//!
//! This module deliberately contains no database or transport code. It defines
//! deterministic eligibility, ordering, lifecycle, and lease-fencing semantics
//! that database adapters must preserve when they implement atomic claims.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TrustTier {
    Untrusted,
    Standard,
    Trusted,
    Privileged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueState {
    Queued,
    Claimed,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

impl QueueState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
        )
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Queued,
                Self::Claimed | Self::Cancelled | Self::Expired
            ) | (
                Self::Claimed,
                Self::Running | Self::Queued | Self::Cancelled | Self::Expired
            ) | (
                Self::Running,
                Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
            )
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRequirements {
    pub os: String,
    pub arch: String,
    pub runtime: Option<String>,
    pub profile: String,
    pub features: Vec<String>,
    pub minimum_trust_tier: TrustTier,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerSnapshot {
    pub worker_id: String,
    pub os: String,
    pub arch: String,
    pub runtime: Option<String>,
    pub profile: String,
    pub features: Vec<String>,
    pub trust_tier: TrustTier,
    pub max_concurrency: u16,
    pub active_jobs: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EligibilityError {
    CapacityExhausted,
    OsMismatch,
    ArchMismatch,
    RuntimeMismatch,
    ProfileMismatch,
    MissingFeature,
    InsufficientTrust,
}

pub fn check_eligibility(
    requirements: &JobRequirements,
    worker: &WorkerSnapshot,
) -> Result<(), EligibilityError> {
    if worker.max_concurrency == 0 || worker.active_jobs >= worker.max_concurrency {
        return Err(EligibilityError::CapacityExhausted);
    }
    if requirements.os != worker.os {
        return Err(EligibilityError::OsMismatch);
    }
    if requirements.arch != worker.arch {
        return Err(EligibilityError::ArchMismatch);
    }
    if let Some(required_runtime) = requirements.runtime.as_deref() {
        if worker.runtime.as_deref() != Some(required_runtime) {
            return Err(EligibilityError::RuntimeMismatch);
        }
    }
    if requirements.profile != worker.profile {
        return Err(EligibilityError::ProfileMismatch);
    }
    if requirements
        .features
        .iter()
        .any(|required| !worker.features.contains(required))
    {
        return Err(EligibilityError::MissingFeature);
    }
    if worker.trust_tier < requirements.minimum_trust_tier {
        return Err(EligibilityError::InsufficientTrust);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulingPolicy {
    pub age_step_seconds: u64,
    pub max_age_boost: u8,
}

impl Default for SchedulingPolicy {
    fn default() -> Self {
        Self {
            age_step_seconds: 300,
            max_age_boost: 20,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueCandidate {
    pub job_id: String,
    pub base_priority: u8,
    pub enqueued_at_seconds: u64,
    pub available_at_seconds: u64,
    pub requirements: JobRequirements,
}

pub fn effective_priority(
    candidate: &QueueCandidate,
    now_seconds: u64,
    policy: SchedulingPolicy,
) -> u16 {
    let age_seconds = now_seconds.saturating_sub(candidate.enqueued_at_seconds);
    let age_step = policy.age_step_seconds.max(1);
    let raw_boost = age_seconds / age_step;
    let boost = raw_boost.min(u64::from(policy.max_age_boost));
    u16::from(candidate.base_priority) + boost as u16
}

/// Ordering where `Less` means `a` should be scheduled before `b`.
pub fn compare_candidates(
    a: &QueueCandidate,
    b: &QueueCandidate,
    now_seconds: u64,
    policy: SchedulingPolicy,
) -> Ordering {
    effective_priority(b, now_seconds, policy)
        .cmp(&effective_priority(a, now_seconds, policy))
        .then_with(|| a.available_at_seconds.cmp(&b.available_at_seconds))
        .then_with(|| a.job_id.cmp(&b.job_id))
}

pub fn select_next<'a>(
    candidates: &'a [QueueCandidate],
    worker: &WorkerSnapshot,
    now_seconds: u64,
    policy: SchedulingPolicy,
) -> Option<&'a QueueCandidate> {
    candidates
        .iter()
        .filter(|candidate| candidate.available_at_seconds <= now_seconds)
        .filter(|candidate| check_eligibility(&candidate.requirements, worker).is_ok())
        .min_by(|a, b| compare_candidates(a, b, now_seconds, policy))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseAuthority {
    pub generation: u64,
    pub fencing_token: u64,
    pub attempt: u32,
}

impl LeaseAuthority {
    pub const fn initial() -> Self {
        Self {
            generation: 1,
            fencing_token: 1,
            attempt: 1,
        }
    }

    pub fn reclaim(self) -> Option<Self> {
        Some(Self {
            generation: self.generation.checked_add(1)?,
            fencing_token: self.fencing_token.checked_add(1)?,
            attempt: self.attempt.checked_add(1)?,
        })
    }

    pub const fn authorizes(self, presented: Self) -> bool {
        self.generation == presented.generation
            && self.fencing_token == presented.fencing_token
            && self.attempt == presented.attempt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirements() -> JobRequirements {
        JobRequirements {
            os: "linux".into(),
            arch: "x86_64".into(),
            runtime: Some("podman".into()),
            profile: "rust-ci".into(),
            features: vec!["rust".into(), "oci".into()],
            minimum_trust_tier: TrustTier::Standard,
        }
    }

    fn worker() -> WorkerSnapshot {
        WorkerSnapshot {
            worker_id: "worker-a".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            runtime: Some("podman".into()),
            profile: "rust-ci".into(),
            features: vec!["oci".into(), "rust".into(), "cache".into()],
            trust_tier: TrustTier::Trusted,
            max_concurrency: 4,
            active_jobs: 1,
        }
    }

    fn candidate(id: &str, priority: u8, enqueued: u64, available: u64) -> QueueCandidate {
        QueueCandidate {
            job_id: id.into(),
            base_priority: priority,
            enqueued_at_seconds: enqueued,
            available_at_seconds: available,
            requirements: requirements(),
        }
    }

    #[test]
    fn exact_capabilities_are_eligible() {
        assert_eq!(check_eligibility(&requirements(), &worker()), Ok(()));
    }

    #[test]
    fn capacity_is_checked_before_claiming() {
        let mut full = worker();
        full.active_jobs = full.max_concurrency;
        assert_eq!(
            check_eligibility(&requirements(), &full),
            Err(EligibilityError::CapacityExhausted)
        );
    }

    #[test]
    fn runtime_profile_and_feature_mismatches_fail_closed() {
        let mut w = worker();
        w.runtime = Some("docker".into());
        assert_eq!(
            check_eligibility(&requirements(), &w),
            Err(EligibilityError::RuntimeMismatch)
        );

        let mut w = worker();
        w.profile = "generic".into();
        assert_eq!(
            check_eligibility(&requirements(), &w),
            Err(EligibilityError::ProfileMismatch)
        );

        let mut w = worker();
        w.features.retain(|feature| feature != "oci");
        assert_eq!(
            check_eligibility(&requirements(), &w),
            Err(EligibilityError::MissingFeature)
        );
    }

    #[test]
    fn insufficient_trust_fails_closed() {
        let mut w = worker();
        w.trust_tier = TrustTier::Untrusted;
        assert_eq!(
            check_eligibility(&requirements(), &w),
            Err(EligibilityError::InsufficientTrust)
        );
    }

    #[test]
    fn age_boost_is_bounded() {
        let job = candidate("a", 10, 0, 0);
        let policy = SchedulingPolicy {
            age_step_seconds: 10,
            max_age_boost: 7,
        };
        assert_eq!(effective_priority(&job, 10_000, policy), 17);
    }

    #[test]
    fn age_boost_can_break_a_small_priority_gap() {
        let older = candidate("older", 40, 0, 0);
        let newer = candidate("newer", 45, 1_000, 0);
        let policy = SchedulingPolicy {
            age_step_seconds: 100,
            max_age_boost: 10,
        };
        assert_eq!(
            compare_candidates(&older, &newer, 1_000, policy),
            Ordering::Less
        );
    }

    #[test]
    fn explicit_priority_wins_beyond_bounded_aging() {
        let old_low = candidate("old", 10, 0, 0);
        let new_high = candidate("new", 90, 10_000, 0);
        let policy = SchedulingPolicy {
            age_step_seconds: 1,
            max_age_boost: 20,
        };
        assert_eq!(
            compare_candidates(&old_low, &new_high, 10_000, policy),
            Ordering::Greater
        );
    }

    #[test]
    fn deterministic_ties_use_available_time_then_job_id() {
        let later = candidate("a", 50, 0, 20);
        let earlier = candidate("z", 50, 0, 10);
        assert_eq!(
            compare_candidates(&earlier, &later, 100, SchedulingPolicy::default()),
            Ordering::Less
        );

        let a = candidate("a", 50, 0, 10);
        let b = candidate("b", 50, 0, 10);
        assert_eq!(
            compare_candidates(&a, &b, 100, SchedulingPolicy::default()),
            Ordering::Less
        );
    }

    #[test]
    fn selection_filters_unavailable_and_ineligible_jobs_first() {
        let future = candidate("future", 100, 0, 1_000);
        let mut wrong_os = candidate("wrong-os", 99, 0, 0);
        wrong_os.requirements.os = "windows".into();
        let eligible = candidate("eligible", 20, 0, 0);
        let jobs = vec![future, wrong_os, eligible];

        assert_eq!(
            select_next(&jobs, &worker(), 100, SchedulingPolicy::default())
                .map(|job| job.job_id.as_str()),
            Some("eligible")
        );
    }

    #[test]
    fn reclaim_increments_attempt_generation_and_fence() {
        let first = LeaseAuthority::initial();
        let second = first.reclaim().expect("lease counters have room");
        assert_eq!(second.generation, 2);
        assert_eq!(second.fencing_token, 2);
        assert_eq!(second.attempt, 2);
    }

    #[test]
    fn stale_lease_cannot_publish_after_reclaim() {
        let first = LeaseAuthority::initial();
        let second = first.reclaim().expect("lease counters have room");
        assert!(!second.authorizes(first));
        assert!(second.authorizes(second));
    }

    #[test]
    fn terminal_queue_states_never_transition() {
        for state in [
            QueueState::Succeeded,
            QueueState::Failed,
            QueueState::Cancelled,
            QueueState::Expired,
        ] {
            assert!(state.is_terminal());
            assert!(!state.can_transition_to(QueueState::Queued));
            assert!(!state.can_transition_to(QueueState::Running));
        }
    }

    #[test]
    fn claimed_jobs_can_be_requeued_only_through_reclaim_semantics() {
        assert!(QueueState::Claimed.can_transition_to(QueueState::Queued));
        assert!(!QueueState::Running.can_transition_to(QueueState::Queued));
    }
}
