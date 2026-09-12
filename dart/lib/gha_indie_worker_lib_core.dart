/// Dart sibling of `gha-indie-worker-lib-core`.
///
/// The subset the Flutter clients need: the onboarding state machines, so a
/// screen can decide what to show without asking the server. The Rust crate is
/// the reference; `test/onboarding_test.dart` asserts the tables match.
///
/// Deliberately absent: anything touching a database, a socket or a lock.
library;

export 'src/onboarding.dart';
