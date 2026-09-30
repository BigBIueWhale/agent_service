# Record response history before publishing a terminal chunk

GeminiChat owns the history decision for all physical responses attached to a
logical chat attempt. Once processing has decided the attempt (accepted,
refused or abandoned), a terminal chunk carries that decision. The shared chat stream
awaits response-history recording before publishing this terminal chunk to any
consumer. Early text and reasoning remain available before a terminal decision.

The per-attempt settlement Promise is shared by terminal publication and the
stream's cleanup. This calls the deliberately single-use `ChatAttempt.finish`
exactly once, including when a consumer returns immediately after receiving the
terminal. Cleanup before any terminal decision records abandonment. An abort
while an accepted decision is being recorded retains that accepted decision;
it cannot undo an assistant already committed to canonical history.

A non-null terminal decision also closes the retry boundary. A recording
failure must not trigger a new generation against already committed history,
even if its error resembles a retryable network or rate-limit failure and the
generation contained only a tool call or no visible content. Preterminal
provider failures retain their existing retry rules. A settlement failure
reaches the consumer without exposing the terminal chunk. Reusing the same
Promise retains the original failure instead of adding a second-settlement
error.

This boundary serves all GeminiChat consumers: root and child agents, interactive
and noninteractive sessions, and other client frontends. vLLM does not own the
client's canonical history decision or its response-history journal, so this
ordering belongs in the shared client rather than the backend. It is independent
of context size and benchmark configuration.

Canonical resume reads runtime chat JSONL, not the stdout evidence file. The
ordering leaves canonical conversation content, acceptance, record shape and
active history order as they are; it records physical response-history
evidence before terminal output can escape. If that write fails after
canonical acceptance, the accepted assistant remains committed; the stream
reports failure and does not resample, and complete-record admission refuses
the missing evidence. This is an ordering guarantee, not a claim of atomic
filesystem transactions.
