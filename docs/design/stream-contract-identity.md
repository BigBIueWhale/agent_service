# Stream contract identity and verification limits

The structural compiler reads the selected contract's root `$id`. It requires
the owned `urn:agent-service:stream-contract:` namespace and a positive decimal
revision without leading zeros. The revision is an identity string, not a
machine-sized counter. The generated binding and schema inventory preserve
that exact identity. Nested resource identities remain unsupported. Exact
producer/reader agreement remains bound to the SHA-256 of the contract bytes;
accepting a new definition at build time does not admit a different runtime
contract.

## Evidence

Code reading at agent_service commit `60e08e2` found literal
`urn:agent-service:stream-contract:3` checks at lines 83 and 400 of
`protocol/engine/schema_compiler.rs`. In that same commit,
`protocol/engine/build.rs` reads `../stream-contract-v5.json`, whose `$id` ends
in `:5`, and calls `expect` on the compiler result. The two literals are removed
by this change. This comparison establishes a source inconsistency. It does
not establish a history of executed build failures, and no such claim is made.

A Python source inspection compared the checked-in contract alternatives with
the native match cases: all seven event kinds, thirteen system kinds and eight
partial kinds have textual cases. It also compared the 21 GeminiEventType
members with the committed interactive handler: all have explicit cases,
including AttemptStarted. Reading the shared TypeScript partial-stream switch
found its eight cases as well. These are source inspections, not compilation,
exhaustiveness proofs for the entire program, or executed native refusals.

Rust regression tests are included for the contract-derived identity, revision
changes, malformed or absent identities, foreign namespaces and nested
resources. **These tests have not been run. The change is unverified by a build
and requires the owner's build and native test gates.** No compiler, cargo
check, cargo test, build, image, release, deployment or push was run for this
change. `git diff --check` checks whitespace only.

Any earlier commit or report describing a native refusal without a recorded
execution must be read as implementation intent or a conclusion from source
inspection, not a verified runtime result. Focused TypeScript or Python test
results do not establish native engine behavior. The claim that the native
engine failed to compile for ten commits is not supported by execution evidence
and is not adopted here.

## Scope

This binding serves every consumer of the shared native engine, including the
service and certifier. Direct vLLM callers do not consume this client stream
contract; the change does not claim a backend effect. It changes neither the
wire schema nor model input.
