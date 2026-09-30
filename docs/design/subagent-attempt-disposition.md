# Subagent attempt disposition

`GeminiChat` owns the decision to commit a generated turn to history. A
provider finish reason alone does not prove that decision: stream validation
and the durable assistant write must succeed first. Each processed chunk
therefore carries either no decision yet, `accepted`, `refused` (the turn
reached its output limit and is drawn again), or `abandoned`. Only `accepted`
enters history. The terminal decision is emitted only after the history
operation succeeds.

`AgentCore` accumulates one observation per streaming attempt. Until the chat
publishes its decision, the observation is abandoned if the consumer exits.
Retry clears the entire observation before starting another attempt, including
when loop detection stops on the retry boundary. Accepted and abandoned
observations retain their exact text, reasoning, usage, round and attempt.
A delivered terminal's finish reason and incomplete tool arguments are captured
before cancellation can return; retaining evidence never authorizes a tool.

The agent's observable transcript uses a distinct `generation_attempt` role
for every attempt whose output did not enter history. This makes assistant-only report and arena selectors
incapable of promoting it to an accepted answer. Interactive rendering labels
the observation without changing its recorded content. Team reporting changes
only when an accepted round arrives. An interrupted or refused attempt cannot
replace the last accepted team report. Abnormal run results retain useful
partial text and their explicit stop reason, so cancelling an ordinary session
does not discard work.

This belongs to the shared client runtime and applies to ordinary,
interactive, foreground, background, team and arena subagents. vLLM owns the
served bytes and backend generation accounting; it cannot know which output a
caller later commits. No backend configuration can supply this client decision.

The canonical child transcript is commit-driven. Its accepted assistant records
are history; its other generations are `model_generation` evidence. Resume
reads that canonical transcript, not observable runtime messages or stdout.

Tests cover a legal reasoning-only retry, failure after visible text,
output-limit refusal, cancellation, accepted-empty rounds, report preservation,
terminal publication after durable commit, and canonical child replay.
