//// Gleam sibling of `gha-indie-worker-lib-core`.
////
//// The onboarding state machines, mirroring `src/onboarding.rs`,
//// `typescript/src/onboarding.mjs` and `dart/lib/src/onboarding.dart`.
//// Pure: no I/O, no clock, no process.

import gleam/list

/// Org onboarding states.
pub type OrgState {
  Created
  OrgProfile
  BillingLinked
  GithubAppInstalled
  FirstWorkflowPlanned
  Active
  Suspended
}

/// User onboarding states.
pub type UserState {
  UserCreated
  EmailVerified
  ProfileComplete
  OrgJoined
  UserActive
  Dormant
}

/// The events either machine accepts.
pub type Event {
  ProfileCompleted
  BillingWasLinked
  GithubAppWasInstalled
  FirstWorkflowWasPlanned
  Activated
  WasSuspended
  Resumed
  EmailWasVerified
  JoinedOrg
  WentDormant
}

/// Why an advance did not move the machine.
pub type Rejection {
  UnknownEvent
  NotAllowedHere
}

/// The settled result of an advance.
pub type Outcome(state) {
  Advanced(state)
  AlreadyThere(state)
  Rejected(Rejection)
}

/// The wire value both contract authorities use for an org state.
pub fn org_state_to_wire(state: OrgState) -> String {
  case state {
    Created -> "created"
    OrgProfile -> "org-profile"
    BillingLinked -> "billing-linked"
    GithubAppInstalled -> "github-app-installed"
    FirstWorkflowPlanned -> "first-workflow-planned"
    Active -> "active"
    Suspended -> "suspended"
  }
}

/// The wire value both contract authorities use for a user state.
pub fn user_state_to_wire(state: UserState) -> String {
  case state {
    UserCreated -> "created"
    EmailVerified -> "email-verified"
    ProfileComplete -> "profile-complete"
    OrgJoined -> "org-joined"
    UserActive -> "active"
    Dormant -> "dormant"
  }
}

/// Every org state, for exhaustiveness checks in tests.
pub fn org_states() -> List(OrgState) {
  [
    Created,
    OrgProfile,
    BillingLinked,
    GithubAppInstalled,
    FirstWorkflowPlanned,
    Active,
    Suspended,
  ]
}

/// Every user state.
pub fn user_states() -> List(UserState) {
  [UserCreated, EmailVerified, ProfileComplete, OrgJoined, UserActive, Dormant]
}

/// The events the org machine understands.
pub fn org_events() -> List(Event) {
  [
    ProfileCompleted,
    BillingWasLinked,
    GithubAppWasInstalled,
    FirstWorkflowWasPlanned,
    Activated,
    WasSuspended,
  ]
}

/// The complete edge set of the org machine.
pub fn org_transitions() -> List(#(OrgState, Event, OrgState)) {
  [
    #(Created, ProfileCompleted, OrgProfile),
    #(OrgProfile, BillingWasLinked, BillingLinked),
    #(BillingLinked, GithubAppWasInstalled, GithubAppInstalled),
    #(GithubAppInstalled, FirstWorkflowWasPlanned, FirstWorkflowPlanned),
    #(FirstWorkflowPlanned, Activated, Active),
    #(Created, WasSuspended, Suspended),
    #(OrgProfile, WasSuspended, Suspended),
    #(BillingLinked, WasSuspended, Suspended),
    #(GithubAppInstalled, WasSuspended, Suspended),
    #(FirstWorkflowPlanned, WasSuspended, Suspended),
    #(Active, WasSuspended, Suspended),
    #(Suspended, Activated, Active),
  ]
}

/// The complete edge set of the user machine.
pub fn user_transitions() -> List(#(UserState, Event, UserState)) {
  [
    #(UserCreated, EmailWasVerified, EmailVerified),
    #(EmailVerified, ProfileCompleted, ProfileComplete),
    #(ProfileComplete, JoinedOrg, OrgJoined),
    #(OrgJoined, Activated, UserActive),
    #(UserCreated, WentDormant, Dormant),
    #(EmailVerified, WentDormant, Dormant),
    #(ProfileComplete, WentDormant, Dormant),
    #(OrgJoined, WentDormant, Dormant),
    #(UserActive, WentDormant, Dormant),
    #(Dormant, Resumed, UserActive),
  ]
}

/// The events the user machine understands.
pub fn user_events() -> List(Event) {
  [
    EmailWasVerified,
    ProfileCompleted,
    JoinedOrg,
    Activated,
    WentDormant,
    Resumed,
  ]
}

fn advance(
  current: state,
  event: Event,
  edges: List(#(state, Event, state)),
  events: List(Event),
) -> Outcome(state) {
  case list.contains(events, event) {
    False -> Rejected(UnknownEvent)
    True ->
      case
        list.find(edges, fn(edge) { edge.0 == current && edge.1 == event })
      {
        Ok(edge) -> Advanced(edge.2)
        Error(_) ->
          case
            list.any(edges, fn(edge) { edge.1 == event && edge.2 == current })
          {
            True -> AlreadyThere(current)
            False -> Rejected(NotAllowedHere)
          }
      }
  }
}

/// Apply one event to an org.
pub fn advance_org(state: OrgState, event: Event) -> Outcome(OrgState) {
  advance(state, event, org_transitions(), org_events())
}

/// Apply one event to a user.
pub fn advance_user(state: UserState, event: Event) -> Outcome(UserState) {
  advance(state, event, user_transitions(), user_events())
}

/// States reachable from `state` in one step.
pub fn org_next_states(state: OrgState) -> List(OrgState) {
  org_transitions()
  |> list.filter(fn(edge) { edge.0 == state })
  |> list.map(fn(edge) { edge.2 })
  |> list.unique
}

/// Whether an org may submit runs.
pub fn org_may_run_workflows(state: OrgState) -> Bool {
  state == Active
}
