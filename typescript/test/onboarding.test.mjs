import test from 'node:test';
import assert from 'node:assert/strict';

import {
  ORG_STATES, USER_STATES, ORG_EVENTS, USER_EVENTS,
  ORG_TRANSITIONS, USER_TRANSITIONS,
  advanceOrg, advanceUser, orgNextStates, userNextStates, orgMayRunWorkflows,
} from '../src/onboarding.mjs';

const reachable = (start, next) => {
  const seen = [start];
  for (let i = 0; i < seen.length; i += 1) {
    for (const state of next(seen[i])) if (!seen.includes(state)) seen.push(state);
  }
  return seen;
};

test('org edges are deterministic', () => {
  for (const [from, event] of ORG_TRANSITIONS) {
    const targets = ORG_TRANSITIONS.filter(([f, e]) => f === from && e === event);
    assert.equal(targets.length, 1, `${from} + ${event}`);
  }
});

test('user edges are deterministic', () => {
  for (const [from, event] of USER_TRANSITIONS) {
    const targets = USER_TRANSITIONS.filter(([f, e]) => f === from && e === event);
    assert.equal(targets.length, 1, `${from} + ${event}`);
  }
});

test('no self loops', () => {
  for (const [from, , to] of [...ORG_TRANSITIONS, ...USER_TRANSITIONS]) assert.notEqual(from, to);
});

test('every state is reachable from created', () => {
  assert.deepEqual(reachable('created', orgNextStates).sort(), [...ORG_STATES].sort());
  assert.deepEqual(reachable('created', userNextStates).sort(), [...USER_STATES].sort());
});

test('every edge names a known state and event', () => {
  for (const [from, event, to] of ORG_TRANSITIONS) {
    assert.ok(ORG_STATES.includes(from) && ORG_STATES.includes(to), `${from}->${to}`);
    assert.ok(ORG_EVENTS.includes(event), event);
  }
  for (const [from, event, to] of USER_TRANSITIONS) {
    assert.ok(USER_STATES.includes(from) && USER_STATES.includes(to), `${from}->${to}`);
    assert.ok(USER_EVENTS.includes(event), event);
  }
});

test('the happy path walks created to active', () => {
  let state = 'created';
  for (const event of ['profile-completed', 'billing-linked', 'github-app-installed', 'first-workflow-planned', 'activated']) {
    const result = advanceOrg(state, event);
    assert.equal(result.outcome, 'advanced', `${state} + ${event}`);
    state = result.state;
  }
  assert.equal(state, 'active');
  assert.ok(orgMayRunWorkflows(state));
});

test('replaying an event is idempotent, not an error', () => {
  assert.deepEqual(advanceOrg('org-profile', 'profile-completed'), { outcome: 'already-there', state: 'org-profile' });
});

test('skipping a step is rejected', () => {
  assert.equal(advanceOrg('created', 'activated').reason, 'not-allowed-here');
});

test('an event from the other machine is unknown', () => {
  assert.equal(advanceOrg('created', 'email-verified').reason, 'unknown-event');
  assert.equal(advanceUser('created', 'billing-linked').reason, 'unknown-event');
});

test('suspension is reachable from every org state except itself', () => {
  for (const state of ORG_STATES) {
    if (state === 'suspended') continue;
    assert.equal(advanceOrg(state, 'suspended').outcome, 'advanced', state);
  }
});

test('every state/event pair is decided and never throws', () => {
  for (const state of ORG_STATES) {
    for (const event of ORG_EVENTS) {
      assert.ok(['advanced', 'already-there', 'rejected'].includes(advanceOrg(state, event).outcome));
    }
  }
  for (const state of USER_STATES) {
    for (const event of USER_EVENTS) {
      assert.ok(['advanced', 'already-there', 'rejected'].includes(advanceUser(state, event).outcome));
    }
  }
});
