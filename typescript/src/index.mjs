// gha-indie-worker-lib-core, TypeScript/ESM sibling.
//
// The subset of the Rust crate that a browser, a Node service or the Flutter web
// bridge needs: contract validation and the onboarding state machines. Zero
// dependencies, ESM only, `node --test` for tests.
//
// Not here, on purpose: anything that touches a database, a socket or a lock.
// Those live in the Rust crate and behind the servers.
export {
  validate,
  validateDef,
  asProblemViolations,
} from './validate.mjs';

export {
  ORG_STATES,
  USER_STATES,
  ORG_EVENTS,
  USER_EVENTS,
  ORG_TRANSITIONS,
  USER_TRANSITIONS,
  advanceOrg,
  advanceUser,
  orgNextStates,
  userNextStates,
  orgMayRunWorkflows,
  userIsSettled,
} from './onboarding.mjs';

export { MAX_FRAME_BYTES, encodeFrame, FrameDecoder } from './frames.mjs';
