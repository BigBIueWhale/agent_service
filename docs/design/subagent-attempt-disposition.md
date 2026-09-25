# Subagent attempt disposition

`GeminiChat` owns the decision to commit a generated turn to history. A
provider finish reason alone does not prove that decision: stream validation
and the durable assistant write must succeed first. Each processed chunk
therefore carries either no decision yet, `accepted`, or `abandoned`. The
terminal decision is emitted only after the history operation succeeds.

`AgentCore` accumulates one observation per streaming attempt. Until the chat
publishes its decision, the observation is abandoned if the consumer exits.
Retry clears the entire observation before starting another attempt, including
when loop detection stops on the retry boundary. Accepted and abandoned
observations retain their exact text, reasoning, usage, round and attempt.
A delivered terminal's finish reason and incomplete tool arguments are captured
before cancellation can return; retaining evidence never authorizes a tool.

The agent's observable transcript uses a distinct `generation_attempt` role
for abandoned output. This makes assistant-only report and arena selectors
incapable of promoting it to an accepted answer. Interactive rendering labels
the observation without changing its recorded content. Team reporting changes
only when an accepted round arrives. An interrupted or refused attempt cannot
replace the last accepted team report. Abnormal run results retain useful
partial text and their explicit stop reason; this fix does not discard work
when an ordinary session is cancelled.

This change belongs to the shared client runtime and applies to ordinary,
interactive, foreground, background, team and arena subagents. vLLM owns the
served bytes and backend generation accounting; it cannot know which output a
caller later commits. No backend configuration can supply this client decision.

The canonical child transcript remains commit-driven. Its generation-failure
records are evidence, while accepted assistant records are history. Resume
continues to read that canonical transcript, not observable runtime messages or
stdout. This step changes neither its format nor its history projection.

The stdout round serializer and the transient live child stream still need to
carry this shared decision and attempt identity. Main-agent retry output and
physical request-to-consumer attribution remain separate obligations under the
record-completeness goal. This runtime change does not certify those paths.

Verification covers a legal reasoning-only retry, failure after visible text,
output-limit refusal, cancellation, accepted-empty rounds, report preservation,
terminal publication after durable commit, and canonical child replay. Checks
run against source without builds or remote provider access.
