# `gleam/` — Gleam sibling

A skeleton, deliberately small: the onboarding state machines and nothing else.

Gleam is in the fleet for the parts that want exhaustive pattern matching and a
pipe-first style, and this package exists so the same transition table can be
compiled by a fourth type system. If the Rust, TypeScript, Dart and Gleam tables
ever disagree, the disagreement is a bug in whichever one moved last.

```sh
gleam test
gleam format --check src test
```

Same rules as the other siblings: no I/O, no database, no locks.
