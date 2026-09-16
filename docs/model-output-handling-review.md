# Review of slipped-final-message handling

Reviewed together: `62ff9a2b05abf46d18754620f809d9b4797a25bb` and
`ee896739284adaffc5f72eb03d459cb54f239d31`. Review date: 2026-09-16.
The policy is the owner's model-output handling brief, including its explicit
rule that the third consecutive slip ends the run. This resolves the earlier
policy draft's inconsistent phrase “after three notices”: there are two notices,
then termination on the third slip.

## Conclusion

The two commits implement the required classification, notices and terminal
vocabulary together, but the review found a drain-loop defect requiring a new
correction. The first commit alone is also incomplete: its terminal subtype was
absent from the shared wire schema. The second commit closes that integration
gap. Release requires both commits, the drain correction, and the corrected
backend parser; client handling cannot make a fabricated structured call safe.

## Defect found and corrected

In `62ff9a2`, `drainBatch` admitted and counted one turn before its inner
generation loop. The new slip branch continued that loop directly, so its
notice responses bypassed admission and counting. A four-generation run
(main answer, then three drain slips) reported two turns. A session with a
three-turn limit still generated its fourth reply and reported a slip terminal
instead of the exhausted turn budget.

The correction admits and counts each generation at the top of the inner
loop. The notification retains its single interaction ID, including through
tool-result and notice continuations. The same regression also exposed a
discarded promise from `p.finally`: a drain terminal produced an unhandled
rejection even though the original promise's error was handled. The queue now
stores and returns the promise including its cleanup, so the caller receives
the terminal once. Both changes are confined to `nonInteractiveCli.ts` and
its regression tests, with the existing semantic concern extended to enforce
these obligations.

## Findings against the policy

| Obligation | Reviewed implementation and result |
|---|---|
| Recognize observable slips without judging task completion | `packages/core/src/core/final-message-slip.ts` recognizes empty/whitespace-only visible text and the six exact served tool-markup spellings. It does not inspect reasoning or decide whether the assignment is finished. Trimming is used only to classify emptiness; it does not rewrite the message. |
| Never execute or silently repair a slip | Both no-tool branches in `packages/cli/src/nonInteractiveCli.ts` inspect the complete accumulated visible text after assistant-message finalization. The raw assistant message remains in history. There is no XML execution recovery or synthesized tool call. |
| Give factual, counted feedback | The common notice helper appends a user-role notice on slips one and two, states that nothing was executed or changed, and states the count and termination condition. Notices are emitted to the captured stream and recorded as mid-turn user input. A notice continues the same interaction as a ToolResult-type turn, preserving the existing identical-call detector. |
| End loudly on repetition | The shared session counter ends the third consecutive slip as `SLIPPED_FINAL_MESSAGE`, with exit status 1 and wire subtype `error_slipped_final_message`. A tool call or clean answer resets the counter. There is no third notice or unbounded empty-answer nudge. The review correction makes every drain continuation consume the ordinary turn budget, as the main loop already does. |
| Preserve external termination causes | CLI detection requires no calls and `FinishReason.STOP`. The subagent loop checks incomplete generation before classifying its no-call reply. Length, cancellation and API failures keep their own terminal handling. |
| Apply the same rule to subagents | `packages/core/src/agents/runtime/agent-core.ts` uses the shared classifier and limit. Notices have their own external-input transcript kind; the terminal records the slip kind and notice count. Parent-facing output marks the assignment unfinished and its report partial. |
| Admit the terminal throughout the protocol | `protocol/stream-contract-v1.json` declares the subtype. `protocol/engine/build.rs` generates `ERROR_SUBTYPES` from that schema; `src/result_parse.rs` consumes the generated set. The client validator is generated from the same contract. The named Rust regression preserves the subtype, detail and timing and rejects the subtype in a success envelope. |
| Preserve other policy decisions | The changes do not reinstate next-speaker judgment, reset the identical-call limit through recovery notices, execute malformed markup, retry an exhausted generation, or change unknown-tool/invalid-argument feedback. |

Both CLI main and drain loops call the same notice helper and apply the same
STOP/no-tools gate. The semantic concern in
`patches/source_patch_v1/contracts_qwen_code.py` checks both gates, the shared
module, terminal mappings, notices, recording and parent labels, and absence of
the removed nudge and next-speaker check. These are part of the reproducible
landmark transformation, rather than edits to an installed client.

## Verification

Tests use existing disposable Docker images with no network, read-only source
mounts, and scratch output. No listener, model, GPU or service was run.

- Exact source reconstruction and the 35 framework/contract tests passed after
  the unrelated retry change was reverted (`retry-revert-framework.log` and
  `retry-revert-reconstruction.log` under `/tmp/codex-fix`).
- The 754-test post-revert core run passed, including all 17 shared classifier
  tests. That run does not cover CLI or subagent loop execution.
- `cargo test --offline --locked -p runtime_contract`: 30 passed.
- `cargo test --offline --locked -p agent_service --lib result_parse::tests`:
  58 passed, two unrelated tests filtered out. This includes
  `a_run_the_model_ended_by_slipping_three_times_is_its_own_terminal_state`.
  The earlier binary-target invocation selected zero tests and is not evidence.

Two added drain regressions fail against the reviewed runtime: one asserts the
actual generation count, the other asserts refusal before exceeding the budget.
They also expose the discarded-cleanup-promise rejection. Both frozen tests pass
after the correction, with no unhandled rejections. The full
`nonInteractiveCli.test.ts` passes all 163 tests. The shared helper has 17 passing
tests; focused subagent coverage has five; original/new CLI slip and drain coverage
has 16. Pinned Prettier passes both touched files. Logs are
`fable-slip-drain-reproduction.log`, `fable-slip-drain-fixed-verification.log`,
`fable-slip-cli-full-final.log`, `fable-slip-helper-final.log`,
`fable-slip-subagent-final.log`, and `fable-slip-prettier-check.log`.

The scoped no-emit TypeScript check is **not passing**: it reports TS7006 in
`commands/channel/daemon-worker.ts:872` and TS7016 resolving installed
`@lydell/node-pty` declarations from `services/shellExecutionService.ts:13`.
Neither diagnostic names the changed runtime or test. These unrelated sources
and dependencies were not changed to make this review pass. The log is
`fable-slip-typescript-check.log`.

CPU tests establish control flow, history and wire behavior; they do not establish
behavior of a newly built or live deployed image. Complete independent test
details are in `/tmp/codex-fix/fable-slip-verification.md`.
