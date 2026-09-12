/// The org and user onboarding state machines, mirroring `src/onboarding.rs`
/// and `typescript/src/onboarding.mjs`.
///
/// Pure: no I/O, no clock, no plugin channel. Every value is `const`.

/// Org onboarding states, in the order a healthy org passes through them.
enum OrgState {
  created('created'),
  orgProfile('org-profile'),
  billingLinked('billing-linked'),
  githubAppInstalled('github-app-installed'),
  firstWorkflowPlanned('first-workflow-planned'),
  active('active'),
  suspended('suspended');

  const OrgState(this.wire);

  /// The value both contract authorities use.
  final String wire;

  static OrgState? fromWire(String value) {
    for (final state in OrgState.values) {
      if (state.wire == value) return state;
    }
    return null;
  }
}

/// User onboarding states.
enum UserState {
  created('created'),
  emailVerified('email-verified'),
  profileComplete('profile-complete'),
  orgJoined('org-joined'),
  active('active'),
  dormant('dormant');

  const UserState(this.wire);

  final String wire;

  static UserState? fromWire(String value) {
    for (final state in UserState.values) {
      if (state.wire == value) return state;
    }
    return null;
  }
}

/// The events either machine accepts.
enum OnboardingEvent {
  profileCompleted('profile-completed'),
  billingLinked('billing-linked'),
  githubAppInstalled('github-app-installed'),
  firstWorkflowPlanned('first-workflow-planned'),
  activated('activated'),
  suspended('suspended'),
  resumed('resumed'),
  emailVerified('email-verified'),
  orgJoined('org-joined'),
  wentDormant('went-dormant');

  const OnboardingEvent(this.wire);

  final String wire;
}

/// Why an advance did not move the machine.
enum Rejection { unknownEvent, notAllowedHere }

/// The settled result of an advance.
sealed class Outcome<S> {
  const Outcome();
}

/// The machine moved.
final class Advanced<S> extends Outcome<S> {
  const Advanced(this.state);
  final S state;
}

/// A replay: the subject is already in the target state. Not an error.
final class AlreadyThere<S> extends Outcome<S> {
  const AlreadyThere(this.state);
  final S state;
}

/// The event was refused.
final class Rejected<S> extends Outcome<S> {
  const Rejected(this.reason);
  final Rejection reason;
}

/// One `(from, event, to)` edge.
typedef Edge<S> = (S from, OnboardingEvent event, S to);

/// The complete edge set of the org machine.
const List<Edge<OrgState>> orgTransitions = [
  (OrgState.created, OnboardingEvent.profileCompleted, OrgState.orgProfile),
  (OrgState.orgProfile, OnboardingEvent.billingLinked, OrgState.billingLinked),
  (OrgState.billingLinked, OnboardingEvent.githubAppInstalled, OrgState.githubAppInstalled),
  (OrgState.githubAppInstalled, OnboardingEvent.firstWorkflowPlanned, OrgState.firstWorkflowPlanned),
  (OrgState.firstWorkflowPlanned, OnboardingEvent.activated, OrgState.active),
  (OrgState.created, OnboardingEvent.suspended, OrgState.suspended),
  (OrgState.orgProfile, OnboardingEvent.suspended, OrgState.suspended),
  (OrgState.billingLinked, OnboardingEvent.suspended, OrgState.suspended),
  (OrgState.githubAppInstalled, OnboardingEvent.suspended, OrgState.suspended),
  (OrgState.firstWorkflowPlanned, OnboardingEvent.suspended, OrgState.suspended),
  (OrgState.active, OnboardingEvent.suspended, OrgState.suspended),
  (OrgState.suspended, OnboardingEvent.activated, OrgState.active),
];

/// The complete edge set of the user machine.
const List<Edge<UserState>> userTransitions = [
  (UserState.created, OnboardingEvent.emailVerified, UserState.emailVerified),
  (UserState.emailVerified, OnboardingEvent.profileCompleted, UserState.profileComplete),
  (UserState.profileComplete, OnboardingEvent.orgJoined, UserState.orgJoined),
  (UserState.orgJoined, OnboardingEvent.activated, UserState.active),
  (UserState.created, OnboardingEvent.wentDormant, UserState.dormant),
  (UserState.emailVerified, OnboardingEvent.wentDormant, UserState.dormant),
  (UserState.profileComplete, OnboardingEvent.wentDormant, UserState.dormant),
  (UserState.orgJoined, OnboardingEvent.wentDormant, UserState.dormant),
  (UserState.active, OnboardingEvent.wentDormant, UserState.dormant),
  (UserState.dormant, OnboardingEvent.resumed, UserState.active),
];

const Set<OnboardingEvent> orgEvents = {
  OnboardingEvent.profileCompleted,
  OnboardingEvent.billingLinked,
  OnboardingEvent.githubAppInstalled,
  OnboardingEvent.firstWorkflowPlanned,
  OnboardingEvent.activated,
  OnboardingEvent.suspended,
};

const Set<OnboardingEvent> userEvents = {
  OnboardingEvent.emailVerified,
  OnboardingEvent.profileCompleted,
  OnboardingEvent.orgJoined,
  OnboardingEvent.activated,
  OnboardingEvent.wentDormant,
  OnboardingEvent.resumed,
};

Outcome<S> _advance<S>(
  S current,
  OnboardingEvent event,
  List<Edge<S>> edges,
  Set<OnboardingEvent> events,
) {
  if (!events.contains(event)) return Rejected<S>(Rejection.unknownEvent);
  for (final edge in edges) {
    if (edge.$1 == current && edge.$2 == event) return Advanced<S>(edge.$3);
  }
  for (final edge in edges) {
    if (edge.$2 == event && edge.$3 == current) return AlreadyThere<S>(current);
  }
  return Rejected<S>(Rejection.notAllowedHere);
}

/// Apply one event to an org.
Outcome<OrgState> advanceOrg(OrgState state, OnboardingEvent event) =>
    _advance(state, event, orgTransitions, orgEvents);

/// Apply one event to a user.
Outcome<UserState> advanceUser(UserState state, OnboardingEvent event) =>
    _advance(state, event, userTransitions, userEvents);

/// States reachable from [state] in one step.
List<OrgState> orgNextStates(OrgState state) =>
    orgTransitions.where((e) => e.$1 == state).map((e) => e.$3).toSet().toList();

/// States reachable from [state] in one step.
List<UserState> userNextStates(UserState state) =>
    userTransitions.where((e) => e.$1 == state).map((e) => e.$3).toSet().toList();

/// Whether an org may submit runs.
bool orgMayRunWorkflows(OrgState state) => state == OrgState.active;

/// Whether a user may act inside an org.
bool userIsSettled(UserState state) => state == UserState.active;
