# How the two thinking-retention arms are compared

Decided before the data exists, so the conclusion is not chosen after seeing it.

## The unit is the task pair, not the run

Each task is run twice under one release, once per arm, and the pair is the observation.
Pairing inside the task removes the dominant source of variance — tasks differ from each
other far more than arms differ within a task — and it is why the driver has always run
two attempts rather than two passes.

A pass that covers part of the plan is still readable: pairs are independent, so a partial
pass is a smaller sample, not a biased one. Report as it goes.

## What is measured

**Primary — resolution.** `outcome.resolved`, decided by the task's own immutable evaluator
image, never by the agent's self-report. Only **discordant** pairs carry information: a task
both arms resolve, or neither resolves, says nothing about the arms. With `b` pairs resolved
only unpreserved and `c` only preserved, the question is whether `b` and `c` differ by more
than chance, which is the exact binomial (McNemar) test on `b + c` trials. State `b`, `c`
and the two-sided exact p, not a bare accuracy difference over all tasks — that difference
is dominated by the concordant pairs and will look small however large the effect is.

**Secondary — cost, which is the reason the default is what it is.** Per run: turns,
prompt and completion tokens, cached-prompt tokens, wall clock, and the number of
compactions with the token position of each. The last of those is already in the
evidence and already checked: every compaction writes a
`{"type":"system","subtype":"compaction"}` record whose shape the runtime contract
enforces — `status`, `succeeded`, `originalTokenCount`, `newTokenCount`,
`triggerReason`, each drawn candidate's accounting and the transition's frozen budget
(`protocol/engine/src/runtime.rs:212-265`). `originalTokenCount` is the token position
the count alone would not give. What is missing is only the summary: no field of the
session record carries the count, so it is read from the run's own events. Preserved thinking spends window on history;
the claim that retention off "allows the single thread to continue for longer" is a
prediction about compaction count and tokens per turn, and it is either visible in these
numbers or it is not true.

**The covariate — how much thinking each unpreserved run actually shed.** Retention under
`preserve_thinking: false` drops thinking from assistant turns at or before
`ns.last_query_index`, which the client's active-todo reminder advances every third tool
turn. So the dose is set by the trajectory: a run that never calls `todo_write` sheds
nothing and is, in effect, a preserved run wearing the other label. Record the reminder
injections, the `todo_write` calls and where the final cut fell, and report the
distribution. A comparison whose treatment arm contains untreated runs is not a comparison
until that is known, and knowing it costs three counts per run.

## What would count as a result

- A difference in resolution that the exact test does not explain by chance, **and** a cost
  difference in the direction the mechanism predicts. Either alone is a finding worth
  stating plainly as partial.
- No difference in resolution with a clear cost difference is the outcome most consistent
  with the deployment's own reasoning: the same work done inside a smaller window.
- Anything that moves only in runs which shed nothing is not an arm effect. That is what
  the covariate is there to catch.

## What is not compared

Runs from different releases, different environment-image digests, or different prompts.
Every run records its release identity, the environment tarball digest and the prompt
SHA-256 for exactly this reason; a pair whose two halves disagree on any of them is
reported as infrastructure, not as evidence.
