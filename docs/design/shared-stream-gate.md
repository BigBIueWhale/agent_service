# Shared captured-stream gate and producer contract

Addendum 2 authorizes one first bank for items 1 and 2, followed by an implementation/lock commit pair. The first bank establishes the emitted-wire contract and the actual service gate. Configuration authorization, provider-operation settlement and transactional continuation remain later obligations; the first certificate must not claim them.

The sole versioned JSON definition in `protocol/stream-contract-v5.json` owns the accepted top-level and nested event variants and their structural constraints. Build-time generators derive the Rust discriminators and TypeScript validators from that exact definition. The existing real CLI envelope remains the one production format. Initial `stream_event/goal_state` is retained. Unknown or malformed variants are refused with their bytes/evidence retained.

The production Rust captured-stream owner is shared by the service and `event_certifier`. It owns initialization, identity, scope/tool ancestry, partial ordering, served accounting and terminal interpretation. The descriptor reader supplies exact LF-framed bytes and independently established capture facts. Native parsing retains exact JSON numbers and refuses duplicate keys and invalid Unicode before host conversion; reverting to the older lossy serde/jsonschema bridge is not part of the correction. A certificate describes conforming captured output, not provider reclamation, configuration authorization or semantic fidelity of a model result.

Core's required goal admission precedes journaling, snapshots and optional renderers. JSON adapters pass their wire envelopes and partial ordering through the same Config-owned admission. A latched failure is rechecked after awaited turn/dispatch admission and immediately before later tool/hook entry. Generated validators consume authored JavaScript values; they do not claim recovery of exact source lexemes already lost by a separate reader. Producer and native certifier share the definition, while complete artifact tests establish their actual composition. The service's one-shot terminal rule must not be installed as the lifecycle of a persistent human/SDK process.

Remove the unshipped native-v2 private launch/journal/configuration grants and their new terminal DTO, full WASM delivery/package/type machinery and incomplete consumer imports. Preserve the capture, teardown, durable publication and complete SDK distribution repairs already required by the gate. The complete WIP was preserved and readback-verified before this correction; its receipt is `/tmp/codex-round7-approved-bank-preservation.json`. No historical session records or released schema versions are rewritten.

Qualification must include native contract and service tests, exact schema/generator/output identity checks, current source transformation and mandatory package test discovery, CLI and browser/declaration/package builds, and the complete final-image launcher/relay/capture/service composition. Preserve nonce/tool/recording/usage tests and the existing cancellation/capture/publication fault cases. A reused old image pass, expected rejection or component pass is not first-bank closure. Pin actual inputs/images, qualify the pair, then push. Future constrained compaction, retries, provider/recovery ownership, stored reasoning and metrics are not closed by this bank.

Request evidence is a separate `model_request` event in stream format 4. The init
record declares its journal and first sequence, and root results declare the
cumulative request count for that output window. The engine prepares replay and
hash validation before committing state; refusal retains the observed bytes.
Evidence records cannot create conversation turns or child scopes. A full request
starts each invocation and committed compaction segment; subsequent records retain
an unchanged message prefix and carry the replacement suffix and exact envelope.
The native reader and SDK validate physical order and reconstructed UTF-8 hashes.
Canonical history uses `recordingVersion: 6` and its own structurally disjoint
request record. Full and indexed restoration validate evidence before projecting
history; `output/events.jsonl` remains byte-exact stdout evidence.


Raw provider response evidence is a distinct `model_response` event tied to a
physical request. Ordered HTTP/body/end/outcome records retain the exact received bytes and the separate SDK processing decision,
including parser failures, and declare EOF versus failed or cancelled prefixes.
A live turn checkpoint names its open responses; the service's complete artifact
certifier refuses open or missing completions. Canonical readers validate physical
response prefixes before projecting history. Response records have no model
message and cannot advance a conversation branch. Transport completion never
claims semantic acceptance of a generation attempt.
