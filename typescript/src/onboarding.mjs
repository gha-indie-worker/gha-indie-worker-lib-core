// The org and user onboarding state machines, mirroring src/onboarding.rs.
//
// Same states, same events, same edges, same three outcomes. The Rust crate is
// the reference; this file exists so a browser or Node client can grey out a
// button without asking the server, and `onboarding.test.mjs` asserts the tables
// have not drifted apart.
//
// Zero dependencies. Everything here is a pure function over frozen data.

export const ORG_STATES = Object.freeze([
  'created',
  'org-profile',
  'billing-linked',
  'github-app-installed',
  'first-workflow-planned',
  'active',
  'suspended',
]);

export const USER_STATES = Object.freeze([
  'created',
  'email-verified',
  'profile-complete',
  'org-joined',
  'active',
  'dormant',
]);

export const ORG_EVENTS = Object.freeze([
  'profile-completed',
  'billing-linked',
  'github-app-installed',
  'first-workflow-planned',
  'activated',
  'suspended',
]);

export const USER_EVENTS = Object.freeze([
  'email-verified',
  'profile-completed',
  'org-joined',
  'activated',
  'went-dormant',
  'resumed',
]);

/** `[from, event, to]`, the complete edge set of the org machine. */
export const ORG_TRANSITIONS = Object.freeze([
  ['created', 'profile-completed', 'org-profile'],
  ['org-profile', 'billing-linked', 'billing-linked'],
  ['billing-linked', 'github-app-installed', 'github-app-installed'],
  ['github-app-installed', 'first-workflow-planned', 'first-workflow-planned'],
  ['first-workflow-planned', 'activated', 'active'],
  ['created', 'suspended', 'suspended'],
  ['org-profile', 'suspended', 'suspended'],
  ['billing-linked', 'suspended', 'suspended'],
  ['github-app-installed', 'suspended', 'suspended'],
  ['first-workflow-planned', 'suspended', 'suspended'],
  ['active', 'suspended', 'suspended'],
  ['suspended', 'activated', 'active'],
].map(Object.freeze));

/** `[from, event, to]`, the complete edge set of the user machine. */
export const USER_TRANSITIONS = Object.freeze([
  ['created', 'email-verified', 'email-verified'],
  ['email-verified', 'profile-completed', 'profile-complete'],
  ['profile-complete', 'org-joined', 'org-joined'],
  ['org-joined', 'activated', 'active'],
  ['created', 'went-dormant', 'dormant'],
  ['email-verified', 'went-dormant', 'dormant'],
  ['profile-complete', 'went-dormant', 'dormant'],
  ['org-joined', 'went-dormant', 'dormant'],
  ['active', 'went-dormant', 'dormant'],
  ['dormant', 'resumed', 'active'],
].map(Object.freeze));

function advance(state, event, transitions, events) {
  if (!events.includes(event)) {
    return Object.freeze({ outcome: 'rejected', reason: 'unknown-event', state });
  }
  const edge = transitions.find(([from, e]) => from === state && e === event);
  if (edge) return Object.freeze({ outcome: 'advanced', state: edge[2] });
  // an event whose target is already the current state is a replay, not a fault
  if (transitions.some(([, e, to]) => e === event && to === state)) {
    return Object.freeze({ outcome: 'already-there', state });
  }
  return Object.freeze({ outcome: 'rejected', reason: 'not-allowed-here', state });
}

/** Apply one event to an org. */
export const advanceOrg = (state, event) => advance(state, event, ORG_TRANSITIONS, ORG_EVENTS);

/** Apply one event to a user. */
export const advanceUser = (state, event) => advance(state, event, USER_TRANSITIONS, USER_EVENTS);

/** States reachable from `state` in one step. */
export const orgNextStates = (state) =>
  Object.freeze([...new Set(ORG_TRANSITIONS.filter(([from]) => from === state).map(([, , to]) => to))]);

/** States reachable from `state` in one step. */
export const userNextStates = (state) =>
  Object.freeze([...new Set(USER_TRANSITIONS.filter(([from]) => from === state).map(([, , to]) => to))]);

/** Whether an org may submit runs. */
export const orgMayRunWorkflows = (state) => state === 'active';

/** Whether a user may act inside an org. */
export const userIsSettled = (state) => state === 'active';
