import 'package:gha_indie_worker_lib_core/gha_indie_worker_lib_core.dart';
import 'package:test/test.dart';

List<S> _reachable<S>(S start, List<S> Function(S) next) {
  final seen = <S>[start];
  for (var i = 0; i < seen.length; i++) {
    for (final state in next(seen[i])) {
      if (!seen.contains(state)) seen.add(state);
    }
  }
  return seen;
}

void main() {
  test('org edges are deterministic', () {
    for (final edge in orgTransitions) {
      final targets = orgTransitions
          .where((e) => e.$1 == edge.$1 && e.$2 == edge.$2)
          .toList();
      expect(targets.length, 1, reason: '${edge.$1} + ${edge.$2}');
    }
  });

  test('no self loops in either machine', () {
    for (final edge in orgTransitions) {
      expect(edge.$1, isNot(edge.$3));
    }
    for (final edge in userTransitions) {
      expect(edge.$1, isNot(edge.$3));
    }
  });

  test('every state is reachable from created', () {
    expect(
      _reachable(OrgState.created, orgNextStates).toSet(),
      OrgState.values.toSet(),
    );
    expect(
      _reachable(UserState.created, userNextStates).toSet(),
      UserState.values.toSet(),
    );
  });

  test('the happy path walks created to active', () {
    var state = OrgState.created;
    const path = [
      OnboardingEvent.profileCompleted,
      OnboardingEvent.billingLinked,
      OnboardingEvent.githubAppInstalled,
      OnboardingEvent.firstWorkflowPlanned,
      OnboardingEvent.activated,
    ];
    for (final event in path) {
      final outcome = advanceOrg(state, event);
      expect(outcome, isA<Advanced<OrgState>>(), reason: '$state + $event');
      state = (outcome as Advanced<OrgState>).state;
    }
    expect(state, OrgState.active);
    expect(orgMayRunWorkflows(state), isTrue);
  });

  test('replaying an event is idempotent, not an error', () {
    expect(
      advanceOrg(OrgState.orgProfile, OnboardingEvent.profileCompleted),
      isA<AlreadyThere<OrgState>>(),
    );
  });

  test('an event from the other machine is unknown', () {
    final outcome = advanceOrg(OrgState.created, OnboardingEvent.emailVerified);
    expect((outcome as Rejected<OrgState>).reason, Rejection.unknownEvent);
  });

  test('wire values match the contract authorities', () {
    expect(OrgState.githubAppInstalled.wire, 'github-app-installed');
    expect(UserState.profileComplete.wire, 'profile-complete');
    expect(OrgState.fromWire('billing-linked'), OrgState.billingLinked);
    expect(OrgState.fromWire('nope'), isNull);
  });
}
