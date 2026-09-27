# Record response history before publishing a terminal chunk

GeminiChat owns the history decision for all physical responses attached to a
logical chat attempt. Once processing commits the assistant or its failed
generation, a terminal chunk carries that decision. The shared chat stream
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

Canonical resume reads runtime chat JSONL, not the stdout evidence file. This
change leaves canonical conversation content, acceptance, record shape, version
and active history order unchanged. It records physical response-history
evidence before allowing terminal output to escape. If that write fails after
canonical acceptance, the accepted assistant remains committed; the stream
reports failure and does not resample. Complete-record admission must continue
to refuse the missing evidence. This is an ordering guarantee, not a claim of
atomic filesystem transactions.

The independent frozen `d8f82de` baseline ran 16 source observation cases with
the actual SDK, pipeline, GeminiChat, Turn and stdout adapters. It established
that terminal function calls and Turn's Finished event could be published
before response-history persistence started. Holding canonical acceptance
already blocked terminal delivery correctly. Early ordinary text was available
without waiting for history settlement. These fixtures use in-memory provider
responses and controlled persistence callbacks; they do not prove disk
durability or deployed tool execution.

The independent candidate suite ran 18 source cases. It checks terminal
withholding, history-write failure, immediate consumer return, cancellation
during settlement, early text, canonical acceptance and physical request retry.
It also retains observations of the unresolved empty-output and usage-projection
gaps; those observations passing does not mean those gaps are fixed. Terminal
settlement rejection preserves the exact original error. A separate preterminal
parse failure combined with history refusal retains both distinct causes, but
the history error occurs twice inside nested aggregates on both the unchanged
baseline and this candidate. That existing behavior is recorded, not claimed
as repaired by this change.

The five affected permanent suites passed 446 source tests: GeminiChat 321,
Turn 66, ChatAttempt seven, response evidence 39 and the actual SDK pipeline
evidence suite 13. The five added regression cases are included in GeminiChat.
These runs use the existing source-test setup; they do not execute the complete
application, provider service or native readers.

Independent source review found no issue in the two changed source files,
their callers or the five added permanent test cases. The transformer framework
passed 38 tests and identifiers retain 35 semantic concerns. A fresh pinned
archive transformation matched all 1,062 declared final identities and exactly
the two intended changed client paths. All 6,913 review hunks matched both
coordinate systems; the 17 recorded candidate source identities matched the
sealed result. The existing generated test validator was not regenerated.
Unrelated authoritative edits and protected release inputs are unchanged.

Compilation, typechecking, native execution, application builds and owner gates
remain unverified and require the owner's build and gates. No build, release,
deployment or push was run for this correction.

Complete logical attempt accounting remains separate required work. Physical
response decisions do not by themselves bind every expected output fragment,
served usage, partial projection and tool call to the attempt. Empty billed
output, usage observed before a later parsing failure, output omission,
duplication, misattribution, runtime ownership and input-to-request accounting
remain open. This ordering boundary does not certify complete record sets.
