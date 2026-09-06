#![forbid(unsafe_code)]

//! The org and user onboarding state machines, as pure functions.
//!
//! No I/O, no clock, no database. A caller supplies the current state and an
//! event; the machine returns the next state or a typed rejection. The servers,
//! the desktop app and `gha-indie-worker-pub-lib-core` all drive the same
//! tables, and the TypeScript and Dart siblings in this repository mirror them
//! value for value.
//!
//! The edge set is stated once, in [`OrgState::transitions`] and
//! [`UserState::transitions`], and everything else — `advance`, reachability,
//! terminality — is derived from it. `x-ores-transitions` in
//! `contracts/json-schema/onboarding.schema.json` states the same edges, and
//! `gha-indie-worker-interfaces` has the test that compares the two.

use core::fmt;

use serde::{Deserialize, Serialize};

/// Org onboarding states, in the order a healthy org passes through them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrgState {
    Created,
    OrgProfile,
    BillingLinked,
    GithubAppInstalled,
    FirstWorkflowPlanned,
    Active,
    Suspended,
}

/// User onboarding states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UserState {
    Created,
    EmailVerified,
    ProfileComplete,
    OrgJoined,
    Active,
    Dormant,
}

/// The events either machine accepts. One event set keeps the wire contract and
/// the audit log identical for both subjects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    ProfileCompleted,
    BillingLinked,
    GithubAppInstalled,
    FirstWorkflowPlanned,
    Activated,
    Suspended,
    Resumed,
    EmailVerified,
    OrgJoined,
    WentDormant,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Event::ProfileCompleted => "profile-completed",
            Event::BillingLinked => "billing-linked",
            Event::GithubAppInstalled => "github-app-installed",
            Event::FirstWorkflowPlanned => "first-workflow-planned",
            Event::Activated => "activated",
            Event::Suspended => "suspended",
            Event::Resumed => "resumed",
            Event::EmailVerified => "email-verified",
            Event::OrgJoined => "org-joined",
            Event::WentDormant => "went-dormant",
        };
        f.write_str(name)
    }
}

/// Why an advance did not move the machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rejection {
    /// The event is not defined for this machine at all.
    UnknownEvent,
    /// The event exists but has no edge out of the current state.
    NotAllowedHere,
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejection::UnknownEvent => f.write_str("event is not part of this state machine"),
            Rejection::NotAllowedHere => f.write_str("event has no transition from this state"),
        }
    }
}

impl std::error::Error for Rejection {}

/// The settled result of an advance. `AlreadyThere` is not an error: replaying
/// an idempotent request must be safe, and the caller records the same response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome<S> {
    Advanced(S),
    AlreadyThere(S),
    Rejected(Rejection),
}

impl<S: Copy> Outcome<S> {
    /// The state the subject is in after the advance, if it is still valid.
    #[must_use]
    pub const fn state(&self) -> Option<S> {
        match self {
            Outcome::Advanced(s) | Outcome::AlreadyThere(s) => Some(*s),
            Outcome::Rejected(_) => None,
        }
    }

    #[must_use]
    pub const fn moved(&self) -> bool {
        matches!(self, Outcome::Advanced(_))
    }
}

fn advance<S: Copy + PartialEq>(
    current: S,
    event: Event,
    edges: &[(S, Event, S)],
    events: &[Event],
) -> Outcome<S> {
    if !events.contains(&event) {
        return Outcome::Rejected(Rejection::UnknownEvent);
    }
    if let Some((_, _, next)) = edges
        .iter()
        .find(|(from, e, _)| *from == current && *e == event)
    {
        return Outcome::Advanced(*next);
    }
    // An event whose target is already the current state is a replay, not a fault.
    if edges.iter().any(|(_, e, to)| *e == event && *to == current) {
        return Outcome::AlreadyThere(current);
    }
    Outcome::Rejected(Rejection::NotAllowedHere)
}

impl OrgState {
    pub const ALL: [OrgState; 7] = [
        OrgState::Created,
        OrgState::OrgProfile,
        OrgState::BillingLinked,
        OrgState::GithubAppInstalled,
        OrgState::FirstWorkflowPlanned,
        OrgState::Active,
        OrgState::Suspended,
    ];

    /// The events this machine understands.
    pub const EVENTS: [Event; 6] = [
        Event::ProfileCompleted,
        Event::BillingLinked,
        Event::GithubAppInstalled,
        Event::FirstWorkflowPlanned,
        Event::Activated,
        Event::Suspended,
    ];

    /// The complete edge set: `(from, event, to)`. Nothing else may move an org.
    #[must_use]
    pub const fn transitions() -> &'static [(OrgState, Event, OrgState)] {
        &[
            (
                OrgState::Created,
                Event::ProfileCompleted,
                OrgState::OrgProfile,
            ),
            (
                OrgState::OrgProfile,
                Event::BillingLinked,
                OrgState::BillingLinked,
            ),
            (
                OrgState::BillingLinked,
                Event::GithubAppInstalled,
                OrgState::GithubAppInstalled,
            ),
            (
                OrgState::GithubAppInstalled,
                Event::FirstWorkflowPlanned,
                OrgState::FirstWorkflowPlanned,
            ),
            (
                OrgState::FirstWorkflowPlanned,
                Event::Activated,
                OrgState::Active,
            ),
            (OrgState::Created, Event::Suspended, OrgState::Suspended),
            (OrgState::OrgProfile, Event::Suspended, OrgState::Suspended),
            (
                OrgState::BillingLinked,
                Event::Suspended,
                OrgState::Suspended,
            ),
            (
                OrgState::GithubAppInstalled,
                Event::Suspended,
                OrgState::Suspended,
            ),
            (
                OrgState::FirstWorkflowPlanned,
                Event::Suspended,
                OrgState::Suspended,
            ),
            (OrgState::Active, Event::Suspended, OrgState::Suspended),
            (OrgState::Suspended, Event::Activated, OrgState::Active),
        ]
    }

    /// Apply one event.
    #[must_use]
    pub fn advance(self, event: Event) -> Outcome<OrgState> {
        advance(self, event, Self::transitions(), &Self::EVENTS)
    }

    /// The states reachable in one step.
    #[must_use]
    pub fn next_states(self) -> Vec<OrgState> {
        let mut out: Vec<OrgState> = Self::transitions()
            .iter()
            .filter(|(from, _, _)| *from == self)
            .map(|(_, _, to)| *to)
            .collect();
        out.dedup();
        out
    }

    /// Whether the org may submit runs.
    #[must_use]
    pub const fn may_run_workflows(self) -> bool {
        matches!(self, OrgState::Active)
    }
}

impl UserState {
    pub const ALL: [UserState; 6] = [
        UserState::Created,
        UserState::EmailVerified,
        UserState::ProfileComplete,
        UserState::OrgJoined,
        UserState::Active,
        UserState::Dormant,
    ];

    pub const EVENTS: [Event; 6] = [
        Event::EmailVerified,
        Event::ProfileCompleted,
        Event::OrgJoined,
        Event::Activated,
        Event::WentDormant,
        Event::Resumed,
    ];

    /// The complete edge set: `(from, event, to)`.
    #[must_use]
    pub const fn transitions() -> &'static [(UserState, Event, UserState)] {
        &[
            (
                UserState::Created,
                Event::EmailVerified,
                UserState::EmailVerified,
            ),
            (
                UserState::EmailVerified,
                Event::ProfileCompleted,
                UserState::ProfileComplete,
            ),
            (
                UserState::ProfileComplete,
                Event::OrgJoined,
                UserState::OrgJoined,
            ),
            (UserState::OrgJoined, Event::Activated, UserState::Active),
            (UserState::Created, Event::WentDormant, UserState::Dormant),
            (
                UserState::EmailVerified,
                Event::WentDormant,
                UserState::Dormant,
            ),
            (
                UserState::ProfileComplete,
                Event::WentDormant,
                UserState::Dormant,
            ),
            (UserState::OrgJoined, Event::WentDormant, UserState::Dormant),
            (UserState::Active, Event::WentDormant, UserState::Dormant),
            (UserState::Dormant, Event::Resumed, UserState::Active),
        ]
    }

    #[must_use]
    pub fn advance(self, event: Event) -> Outcome<UserState> {
        advance(self, event, Self::transitions(), &Self::EVENTS)
    }

    #[must_use]
    pub fn next_states(self) -> Vec<UserState> {
        let mut out: Vec<UserState> = Self::transitions()
            .iter()
            .filter(|(from, _, _)| *from == self)
            .map(|(_, _, to)| *to)
            .collect();
        out.dedup();
        out
    }

    /// Whether the user may act inside an org.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(self, UserState::Active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reachable_org() -> Vec<OrgState> {
        let mut seen = vec![OrgState::Created];
        let mut i = 0;
        while i < seen.len() {
            for next in seen[i].next_states() {
                if !seen.contains(&next) {
                    seen.push(next);
                }
            }
            i += 1;
        }
        seen
    }

    #[test]
    fn org_edges_are_deterministic() {
        for (from, event, _) in OrgState::transitions() {
            let targets: Vec<_> = OrgState::transitions()
                .iter()
                .filter(|(f, e, _)| f == from && e == event)
                .collect();
            assert_eq!(
                targets.len(),
                1,
                "{from:?} + {event} has {} targets",
                targets.len()
            );
        }
    }

    #[test]
    fn user_edges_are_deterministic() {
        for (from, event, _) in UserState::transitions() {
            let targets: Vec<_> = UserState::transitions()
                .iter()
                .filter(|(f, e, _)| f == from && e == event)
                .collect();
            assert_eq!(targets.len(), 1, "{from:?} + {event}");
        }
    }

    #[test]
    fn no_self_loops_in_either_machine() {
        for (from, _, to) in OrgState::transitions() {
            assert_ne!(from, to);
        }
        for (from, _, to) in UserState::transitions() {
            assert_ne!(from, to);
        }
    }

    #[test]
    fn every_org_state_is_reachable_from_created() {
        let seen = reachable_org();
        for state in OrgState::ALL {
            assert!(seen.contains(&state), "{state:?} is unreachable");
        }
    }

    #[test]
    fn every_user_state_is_reachable_from_created() {
        let mut seen = vec![UserState::Created];
        let mut i = 0;
        while i < seen.len() {
            for next in seen[i].next_states() {
                if !seen.contains(&next) {
                    seen.push(next);
                }
            }
            i += 1;
        }
        for state in UserState::ALL {
            assert!(seen.contains(&state), "{state:?} is unreachable");
        }
    }

    #[test]
    fn every_state_event_pair_is_decided() {
        // The table is total: for every (state, event) the machine returns one of
        // three answers, and never panics.
        for state in OrgState::ALL {
            for event in OrgState::EVENTS {
                let outcome = state.advance(event);
                match outcome {
                    Outcome::Advanced(next) => assert_ne!(next, state),
                    Outcome::AlreadyThere(next) => assert_eq!(next, state),
                    Outcome::Rejected(_) => {}
                }
            }
        }
        for state in UserState::ALL {
            for event in UserState::EVENTS {
                let _ = state.advance(event);
            }
        }
    }

    #[test]
    fn the_happy_path_walks_created_to_active() {
        let mut state = OrgState::Created;
        for event in [
            Event::ProfileCompleted,
            Event::BillingLinked,
            Event::GithubAppInstalled,
            Event::FirstWorkflowPlanned,
            Event::Activated,
        ] {
            let outcome = state.advance(event);
            assert!(outcome.moved(), "{state:?} + {event}");
            state = outcome.state().unwrap();
        }
        assert_eq!(state, OrgState::Active);
        assert!(state.may_run_workflows());
    }

    #[test]
    fn replaying_an_event_is_idempotent_not_an_error() {
        let outcome = OrgState::OrgProfile.advance(Event::ProfileCompleted);
        assert_eq!(outcome, Outcome::AlreadyThere(OrgState::OrgProfile));
        assert!(!outcome.moved());
    }

    #[test]
    fn skipping_a_step_is_rejected() {
        assert_eq!(
            OrgState::Created.advance(Event::Activated),
            Outcome::Rejected(Rejection::NotAllowedHere)
        );
    }

    #[test]
    fn an_event_from_the_other_machine_is_unknown() {
        assert_eq!(
            OrgState::Created.advance(Event::EmailVerified),
            Outcome::Rejected(Rejection::UnknownEvent)
        );
        assert_eq!(
            UserState::Created.advance(Event::BillingLinked),
            Outcome::Rejected(Rejection::UnknownEvent)
        );
    }

    #[test]
    fn suspension_is_reachable_from_every_org_state_except_itself() {
        for state in OrgState::ALL {
            if state == OrgState::Suspended {
                continue;
            }
            assert!(
                state.advance(Event::Suspended).moved(),
                "{state:?} cannot be suspended"
            );
        }
    }
}
