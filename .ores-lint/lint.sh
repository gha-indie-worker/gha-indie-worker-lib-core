#!/bin/sh
# One entry point for every linter this repository runs.
#
#   .ores-lint/lint.sh            # everything available on this machine
#   .ores-lint/lint.sh rust       # rustfmt + clippy, all feature combinations
#   .ores-lint/lint.sh js         # eslint + node --test
#   .ores-lint/lint.sh dart       # dart analyze + dart test
#   .ores-lint/lint.sh gleam      # gleam format --check + gleam test
#
# The polyglot siblings are skipped with a note when their toolchain is absent,
# so this is runnable on a laptop that has only cargo.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
what=${1:-all}
run() { echo "[lint] $*"; "$@"; }

if [ "$what" = all ] || [ "$what" = rust ]; then
  for f in rustfmt.toml clippy.toml; do
    diff -q ".ores-lint/$f" "$f" >/dev/null 2>&1 || {
      echo "[lint] $f differs from .ores-lint/$f — copy the canonical file over it" >&2
      exit 1
    }
  done
  run cargo fmt --all -- --check
  run cargo clippy --all-targets -- -D warnings
  run cargo clippy --all-targets --features db -- -D warnings
  run cargo clippy --all-targets --no-default-features --features read-only -- -D warnings
fi

if [ "$what" = all ] || [ "$what" = js ]; then
  if command -v node >/dev/null 2>&1; then
    run npx --no-install eslint --config .ores-lint/eslint.config.mjs typescript
    ( cd typescript && run npm test )
    run npx --no-install markdownlint-cli2 --config .ores-lint/.markdownlint.jsonc "**/*.md" "!node_modules"
  else
    echo "[lint] node not found, skipping the TypeScript sibling"
  fi
fi

if [ "$what" = all ] || [ "$what" = dart ]; then
  if command -v dart >/dev/null 2>&1; then
    ( cd dart && run dart pub get && run dart analyze --fatal-infos && run dart test )
  else
    echo "[lint] dart not found, skipping the Dart sibling"
  fi
fi

if [ "$what" = all ] || [ "$what" = gleam ]; then
  if command -v gleam >/dev/null 2>&1; then
    ( cd gleam && run gleam format --check src test && run gleam test )
  else
    echo "[lint] gleam not found, skipping the Gleam sibling"
  fi
fi

echo "[lint] ok"
