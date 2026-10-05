# Slipped-final-message handling

The policy is the owner's model-output handling brief: a turn that ends with no
tool call and no final answer is a slip, it is answered with a counted notice
twice, and the third consecutive slip ends the run.

## Obligations and where they are met

| Obligation | Implementation |
|---|---|
| Recognize observable slips without judging task completion | `packages/core/src/core/final-message-slip.ts` recognizes empty/whitespace-only visible text and the six exact served tool-markup spellings. It does not inspect reasoning or decide whether the assignment is finished. Trimming is used only to classify emptiness; it does not rewrite the message. |
| Never execute or silently repair a slip | Both no-tool branches in `packages/cli/src/nonInteractiveCli.ts` inspect the complete accumulated visible text after assistant-message finalization. The raw assistant message remains in history and reaches the next request as its own assistant message, one with no reasoning, text or call included (`packages/core/src/services/rendered-request-history.ts` curates no model turn away; the OpenAI converter sends it with empty content). There is no XML execution recovery or synthesized tool call. |
| Give factual, counted feedback | The common notice helper appends a user-role notice on slips one and two, states that nothing was executed or changed, and states the count and termination condition. Notices are emitted to the captured stream and recorded as mid-turn user input. A notice continues the same interaction as a ToolResult-type turn, preserving the existing identical-call detector. |
| End loudly on repetition | The shared session counter ends the third consecutive slip as `SLIPPED_FINAL_MESSAGE`, with exit status 1 and wire subtype `error_slipped_final_message`. A tool call or clean answer resets the counter. There is no third notice or unbounded empty-answer nudge. Every generation of the notification drain loop is admitted and counted at the top of its inner loop, so drain continuations consume the ordinary turn budget exactly as the main loop does. |
| Preserve external termination causes | CLI detection requires no calls and `FinishReason.STOP`. The subagent loop checks incomplete generation before classifying its no-call reply. Length, cancellation and API failures keep their own terminal handling. |
| Apply the same rule to subagents | `packages/core/src/agents/runtime/agent-core.ts` uses the shared classifier and limit. Notices have their own external-input transcript kind; the terminal records the slip kind and notice count. Parent-facing output marks the assignment unfinished and its report partial. |
| Admit the terminal throughout the protocol | `protocol/stream-contract-v19.json` declares the subtype. `protocol/engine/build.rs` generates `ERROR_SUBTYPES` from that schema; `src/result_parse.rs` consumes the generated set. The client validator is generated from the same contract. |
| Preserve other policy decisions | The next-speaker judgment is not reinstated, recovery notices do not reset the identical-call limit, malformed markup is never executed, an exhausted generation is not retried, and unknown-tool/invalid-argument feedback is unchanged. |

Both CLI main and drain loops call the same notice helper and apply the same
STOP/no-tools gate. A drain terminal is returned to its caller through the
awaited queue promise, cleanup included, so it is reported exactly once. The
semantic concern in `patches/source_patch_v1/contracts_qwen_code.py` checks
both gates, the shared module, terminal mappings, notices, recording and parent
labels, and the absence of the removed nudge and next-speaker check. These are
part of the reproducible landmark transformation, rather than edits to an
installed client.
