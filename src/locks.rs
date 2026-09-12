#![forbid(unsafe_code)]

//! The distributed-locking boundary.
//!
//! **This crate contains no lock implementation and never will.** It owns the
//! trait, the request/lease vocabulary, and the composition policy; the real
//! providers live in [`oresoftware/ores-locks-and-leases`](https://github.com/ORESoftware/ores-locks-and-leases),
//! which offers two of them:
//!
//! * **fiducia** — the fleet's coordination API (`/v1/locks/acquire`), which
//!   hands out a *monotonic fencing token*. Cross-process, cross-host, survives
//!   a replica dying. This is what serializes a build of one image across
//!   replicas.
//! * **PostgreSQL transaction-scoped advisory lock** — `pg_try_advisory_xact_lock`.
//!   Cheap, and released by the database when the transaction ends, so a crashed
//!   holder cannot wedge the lock. It only protects work that already sits
//!   inside one transaction against one database.
//!
//! They compose, and the composition is two booleans, so a deployment can enable
//! **one, both or neither**:
//!
//! | fiducia | pg advisory | meaning |
//! |---|---|---|
//! | off | off | no coordination; correct only for single-replica, idempotent work |
//! | off | on | one database, one transaction — cheapest correct option |
//! | on | off | cross-host serialization without a database round trip |
//! | on | on | fiducia fences the work, the advisory lock guards the write |
//!
//! With both on, order matters: fiducia is taken **first** (it is the slow,
//! externally visible claim) and the advisory lock **second, inside** the
//! transaction that does the write. Release is the reverse. [`CompositeLockProvider`]
//! encodes that, including releasing the outer lock when the inner one fails.
//!
//! Everything here is synchronous. The ores-locks-and-leases adapters are async;
//! the servers own that bridge, because this crate must stay usable from the CLI
//! and desktop app where there is no runtime.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a caller wants to hold.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockRequest {
    /// Stable, low-cardinality key. Never a raw token, IP or secret.
    pub key: String,
    /// How long the lease should be valid before it must be renewed.
    pub ttl: Duration,
    /// How long to wait for a contended lock before giving up. `Duration::ZERO`
    /// means try-once, which is what webhook dedup wants.
    pub wait: Duration,
    /// Who is asking, for the coordination API's audit trail.
    pub owner: String,
}

impl LockRequest {
    /// A try-once request with a 30 second lease.
    #[must_use]
    pub fn try_once(key: impl Into<String>, owner: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            ttl: Duration::from_secs(30),
            wait: Duration::ZERO,
            owner: owner.into(),
        }
    }
}

/// A granted lease.
///
/// `fence` is the monotonic token fiducia issues: a holder must send it with
/// every write it makes under the lock, and the receiving side rejects a token
/// lower than one it has already seen. That is what makes a paused-then-resumed
/// holder safe. Providers without fencing report `None`, and a caller that
/// requires fencing must check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockLease {
    pub key: String,
    pub owner: String,
    pub fence: Option<u64>,
    pub ttl: Duration,
}

impl LockLease {
    /// Whether this lease carries a fencing token.
    #[must_use]
    pub const fn is_fenced(&self) -> bool {
        self.fence.is_some()
    }
}

/// Why an acquisition or release failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockError {
    /// Someone else holds it and `wait` elapsed. Not a fault.
    Contended { key: String },
    /// The lease expired or was revoked before renew/release.
    Expired { key: String },
    /// The provider is unreachable or refused. Fail closed or fall back by policy.
    Unavailable { key: String, detail: String },
    /// The caller required a fencing token and the provider does not issue one.
    FencingRequired { key: String },
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Contended { key } => write!(f, "lock {key} is held by another owner"),
            LockError::Expired { key } => write!(f, "lease on {key} has expired"),
            LockError::Unavailable { key, detail } => {
                write!(f, "lock provider for {key} is unavailable: {detail}")
            }
            LockError::FencingRequired { key } => {
                write!(f, "lock {key} requires a fencing token and none was issued")
            }
        }
    }
}

impl std::error::Error for LockError {}

/// The boundary every lock provider implements.
///
/// Implemented in this crate only by [`InMemoryLockProvider`] (tests) and
/// [`CompositeLockProvider`] (composition). Production implementations come from
/// `oresoftware/ores-locks-and-leases`.
pub trait LockProvider: Send + Sync {
    /// A short name for logs and metrics, e.g. `fiducia` or `pg-advisory`.
    fn name(&self) -> &'static str;

    /// Whether this provider issues fencing tokens.
    fn fences(&self) -> bool {
        false
    }

    /// Take the lock.
    ///
    /// # Errors
    ///
    /// [`LockError::Contended`] when someone else holds it, or
    /// [`LockError::Unavailable`] when the provider cannot answer.
    fn acquire(&self, request: &LockRequest) -> Result<LockLease, LockError>;

    /// Extend a lease that is still held.
    ///
    /// # Errors
    ///
    /// [`LockError::Expired`] when the lease is already gone.
    fn renew(&self, lease: &LockLease) -> Result<LockLease, LockError>;

    /// Give the lock back. Releasing a lease you no longer hold is an error, not
    /// a no-op, because it means something has already taken your work.
    ///
    /// # Errors
    ///
    /// [`LockError::Expired`] when the lease is no longer held by this owner.
    fn release(&self, lease: &LockLease) -> Result<(), LockError>;
}

/// Which providers a deployment has enabled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LockPolicy {
    /// `GHA_INDIE_WORKER_LOCKS_FIDUCIA_ENABLED`
    pub fiducia_enabled: bool,
    /// `GHA_INDIE_WORKER_LOCKS_PG_ADVISORY_ENABLED`
    pub pg_advisory_enabled: bool,
    /// Refuse to proceed when no enabled provider issues a fencing token.
    /// `GHA_INDIE_WORKER_LOCKS_REQUIRE_FENCING`
    pub require_fencing: bool,
}

impl LockPolicy {
    /// Neither provider: only correct for single-replica, idempotent work.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            fiducia_enabled: false,
            pg_advisory_enabled: false,
            require_fencing: false,
        }
    }

    #[must_use]
    pub const fn is_coordinated(self) -> bool {
        self.fiducia_enabled || self.pg_advisory_enabled
    }
}

/// Composes an outer (fiducia) and an inner (pg advisory) provider per
/// [`LockPolicy`]. Either may be absent.
pub struct CompositeLockProvider {
    policy: LockPolicy,
    outer: Option<Arc<dyn LockProvider>>,
    inner: Option<Arc<dyn LockProvider>>,
}

impl fmt::Debug for CompositeLockProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompositeLockProvider")
            .field("policy", &self.policy)
            .field("outer", &self.outer.as_ref().map(|p| p.name()))
            .field("inner", &self.inner.as_ref().map(|p| p.name()))
            .finish()
    }
}

impl CompositeLockProvider {
    #[must_use]
    pub fn new(
        policy: LockPolicy,
        outer: Option<Arc<dyn LockProvider>>,
        inner: Option<Arc<dyn LockProvider>>,
    ) -> Self {
        Self {
            policy,
            outer: policy.fiducia_enabled.then_some(outer).flatten(),
            inner: policy.pg_advisory_enabled.then_some(inner).flatten(),
        }
    }

    /// Run `work` while holding whatever the policy requires. The locks are
    /// released in reverse order whether `work` succeeded or not.
    ///
    /// # Errors
    ///
    /// Any [`LockError`] from acquisition, or the error `work` produced.
    pub fn with_lock<T, E, F>(
        &self,
        request: &LockRequest,
        work: F,
    ) -> Result<Result<T, E>, LockError>
    where
        F: FnOnce(&[LockLease]) -> Result<T, E>,
    {
        let mut held: Vec<(Arc<dyn LockProvider>, LockLease)> = Vec::new();
        for provider in [self.outer.as_ref(), self.inner.as_ref()]
            .into_iter()
            .flatten()
        {
            match provider.acquire(request) {
                Ok(lease) => held.push((Arc::clone(provider), lease)),
                Err(e) => {
                    release_all(&mut held);
                    return Err(e);
                }
            }
        }
        if self.policy.require_fencing && !held.iter().any(|(_, lease)| lease.is_fenced()) {
            release_all(&mut held);
            return Err(LockError::FencingRequired {
                key: request.key.clone(),
            });
        }
        let leases: Vec<LockLease> = held.iter().map(|(_, lease)| lease.clone()).collect();
        let outcome = work(&leases);
        release_all(&mut held);
        Ok(outcome)
    }
}

fn release_all(held: &mut Vec<(Arc<dyn LockProvider>, LockLease)>) {
    while let Some((provider, lease)) = held.pop() {
        let _ = provider.release(&lease);
    }
}

/// A process-local provider for tests and single-replica deployments.
///
/// It is a real mutual exclusion within one process and nothing more. It never
/// pretends to fence, so `require_fencing` correctly refuses it.
#[derive(Clone, Debug, Default)]
pub struct InMemoryLockProvider {
    held: Arc<Mutex<HashMap<String, String>>>,
}

impl InMemoryLockProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `key` is currently held, for assertions.
    #[must_use]
    pub fn is_held(&self, key: &str) -> bool {
        self.held.lock().is_ok_and(|held| held.contains_key(key))
    }
}

impl LockProvider for InMemoryLockProvider {
    fn name(&self) -> &'static str {
        "in-memory"
    }

    fn acquire(&self, request: &LockRequest) -> Result<LockLease, LockError> {
        let mut held = self.held.lock().map_err(|_| LockError::Unavailable {
            key: request.key.clone(),
            detail: "in-memory lock table is poisoned".to_owned(),
        })?;
        if held.contains_key(&request.key) {
            return Err(LockError::Contended {
                key: request.key.clone(),
            });
        }
        held.insert(request.key.clone(), request.owner.clone());
        Ok(LockLease {
            key: request.key.clone(),
            owner: request.owner.clone(),
            fence: None,
            ttl: request.ttl,
        })
    }

    fn renew(&self, lease: &LockLease) -> Result<LockLease, LockError> {
        let held = self.held.lock().map_err(|_| LockError::Unavailable {
            key: lease.key.clone(),
            detail: "in-memory lock table is poisoned".to_owned(),
        })?;
        if held.get(&lease.key) == Some(&lease.owner) {
            Ok(lease.clone())
        } else {
            Err(LockError::Expired {
                key: lease.key.clone(),
            })
        }
    }

    fn release(&self, lease: &LockLease) -> Result<(), LockError> {
        let mut held = self.held.lock().map_err(|_| LockError::Unavailable {
            key: lease.key.clone(),
            detail: "in-memory lock table is poisoned".to_owned(),
        })?;
        match held.get(&lease.key) {
            Some(owner) if *owner == lease.owner => {
                held.remove(&lease.key);
                Ok(())
            }
            _ => Err(LockError::Expired {
                key: lease.key.clone(),
            }),
        }
    }
}

/// A test double that behaves like fiducia: it fences.
#[derive(Clone, Debug, Default)]
pub struct FencingTestProvider {
    inner: InMemoryLockProvider,
    next_fence: Arc<Mutex<u64>>,
}

impl FencingTestProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl LockProvider for FencingTestProvider {
    fn name(&self) -> &'static str {
        "fencing-test"
    }

    fn fences(&self) -> bool {
        true
    }

    fn acquire(&self, request: &LockRequest) -> Result<LockLease, LockError> {
        let mut lease = self.inner.acquire(request)?;
        let mut next = self.next_fence.lock().map_err(|_| LockError::Unavailable {
            key: request.key.clone(),
            detail: "fence counter is poisoned".to_owned(),
        })?;
        *next += 1;
        lease.fence = Some(*next);
        Ok(lease)
    }

    fn renew(&self, lease: &LockLease) -> Result<LockLease, LockError> {
        self.inner.renew(lease).map(|mut l| {
            l.fence = lease.fence;
            l
        })
    }

    fn release(&self, lease: &LockLease) -> Result<(), LockError> {
        self.inner.release(lease)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(key: &str, owner: &str) -> LockRequest {
        LockRequest::try_once(key, owner)
    }

    #[test]
    fn a_second_owner_is_contended() {
        let provider = InMemoryLockProvider::new();
        let lease = provider.acquire(&request("image:web", "a")).unwrap();
        assert_eq!(
            provider.acquire(&request("image:web", "b")).unwrap_err(),
            LockError::Contended {
                key: "image:web".to_owned()
            }
        );
        provider.release(&lease).unwrap();
        assert!(provider.acquire(&request("image:web", "b")).is_ok());
    }

    #[test]
    fn releasing_a_lease_you_no_longer_hold_is_an_error() {
        let provider = InMemoryLockProvider::new();
        let lease = provider.acquire(&request("k", "a")).unwrap();
        provider.release(&lease).unwrap();
        assert_eq!(
            provider.release(&lease).unwrap_err(),
            LockError::Expired {
                key: "k".to_owned()
            }
        );
    }

    #[test]
    fn neither_provider_enabled_runs_the_work_unlocked() {
        let composite = CompositeLockProvider::new(LockPolicy::none(), None, None);
        let outcome = composite
            .with_lock(&request("k", "a"), |leases| {
                assert!(leases.is_empty());
                Ok::<_, ()>(7)
            })
            .unwrap();
        assert_eq!(outcome, Ok(7));
    }

    #[test]
    fn both_providers_are_taken_and_released_in_order() {
        let outer = Arc::new(FencingTestProvider::new());
        let inner = Arc::new(InMemoryLockProvider::new());
        let composite = CompositeLockProvider::new(
            LockPolicy {
                fiducia_enabled: true,
                pg_advisory_enabled: true,
                require_fencing: true,
            },
            Some(outer.clone()),
            Some(inner.clone()),
        );
        let seen = composite
            .with_lock(&request("image:web", "a"), |leases| {
                assert_eq!(leases.len(), 2);
                assert!(inner.is_held("image:web"));
                Ok::<_, ()>(leases.iter().filter(|l| l.is_fenced()).count())
            })
            .unwrap()
            .unwrap();
        assert_eq!(seen, 1, "only the fiducia-like provider fences");
        assert!(!inner.is_held("image:web"), "inner lock was released");
    }

    #[test]
    fn a_failed_inner_acquisition_releases_the_outer_lock() {
        let outer = Arc::new(FencingTestProvider::new());
        let inner = Arc::new(InMemoryLockProvider::new());
        // someone else already holds the inner lock
        let squatter = inner.acquire(&request("image:web", "squatter")).unwrap();
        let composite = CompositeLockProvider::new(
            LockPolicy {
                fiducia_enabled: true,
                pg_advisory_enabled: true,
                require_fencing: false,
            },
            Some(outer.clone()),
            Some(inner.clone()),
        );
        let err = composite
            .with_lock(&request("image:web", "a"), |_| Ok::<(), ()>(()))
            .unwrap_err();
        assert!(matches!(err, LockError::Contended { .. }));
        // the outer lock must not be left dangling
        assert!(outer.acquire(&request("image:web", "b")).is_ok());
        inner.release(&squatter).unwrap();
    }

    #[test]
    fn require_fencing_refuses_an_unfenced_composition() {
        let inner = Arc::new(InMemoryLockProvider::new());
        let composite = CompositeLockProvider::new(
            LockPolicy {
                fiducia_enabled: false,
                pg_advisory_enabled: true,
                require_fencing: true,
            },
            None,
            Some(inner.clone()),
        );
        let err = composite
            .with_lock(&request("k", "a"), |_| Ok::<(), ()>(()))
            .unwrap_err();
        assert_eq!(
            err,
            LockError::FencingRequired {
                key: "k".to_owned()
            }
        );
        assert!(
            !inner.is_held("k"),
            "the inner lock was released on refusal"
        );
    }

    #[test]
    fn a_disabled_provider_is_dropped_even_if_supplied() {
        let inner = Arc::new(InMemoryLockProvider::new());
        let composite = CompositeLockProvider::new(LockPolicy::none(), None, Some(inner.clone()));
        composite
            .with_lock(&request("k", "a"), |leases| {
                assert!(leases.is_empty());
                assert!(!inner.is_held("k"));
                Ok::<(), ()>(())
            })
            .unwrap()
            .unwrap();
    }

    #[test]
    fn the_work_error_is_returned_and_the_lock_still_released() {
        let inner = Arc::new(InMemoryLockProvider::new());
        let composite = CompositeLockProvider::new(
            LockPolicy {
                fiducia_enabled: false,
                pg_advisory_enabled: true,
                require_fencing: false,
            },
            None,
            Some(inner.clone()),
        );
        let outcome = composite
            .with_lock(&request("k", "a"), |_| Err::<(), _>("boom"))
            .unwrap();
        assert_eq!(outcome, Err("boom"));
        assert!(!inner.is_held("k"));
    }
}
