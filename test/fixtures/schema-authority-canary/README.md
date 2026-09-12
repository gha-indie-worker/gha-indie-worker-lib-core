# Independent schema-authority canary

`main.tsp` and `authored.schema.json` are independently maintained peer authorities. Neither file is generated from, ranked below, or allowed to overwrite the other.

The CI gate (`ORESoftware/typespec-json-schema-validator`, pinned to an immutable commit) generates JSON Schema B from TypeSpec into `.typespec-json-schema-validator/generated/`, validates both JSON Schema lanes as Draft 2020-12, compares top-level declarations and normalized semantics, and executes bidirectional instance probes. The generated schema and deterministic receipt are evidence only.

Any unexplained mismatch returns `stopped_for_evaluation` and blocks promotion; execution failure returns `failed`. A passing canary proves this repository runs the gate. It does not certify unrelated domain contracts: those declarations must be authored in both lanes (TypeSpec and JSON Schema) and admitted separately before downstream generation, cross-language projection or release.
