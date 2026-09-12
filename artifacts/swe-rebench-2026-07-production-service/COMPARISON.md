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

**Secondary — compaction, which is where the mechanism bites.** Not throughput: how
soon and how often a run is forced to destroy its own history. Compaction is lossy
summarisation and succeeds about 83% of the time, so reaching it sooner costs both detail
and survival. Per run, record the number of compactions and the token position of each,
with turns and token totals beside them. That evidence already exists and is already
checked: every compaction writes a `{"type":"system","subtype":"compaction"}` record
whose shape the runtime contract enforces — `status`, `succeeded`, `originalTokenCount`,
`newTokenCount`, `triggerReason`, each drawn candidate's accounting and the transition's
frozen budget (`protocol/engine/src/runtime.rs:212-265`). `originalTokenCount` is the
position the count alone would not give. What is missing is only the summary: no field of
the session record carries the count, so it is read from the run's own events.

Measured on twelve synthetic agent turns against the live backend, preserved history
grows monotonically at ~1,660 tokens per turn while unpreserved resets at each injected
reminder — 21,717 tokens against 7,245 by turn 12. So the arms differ in *when* they
reach the 161,792-token trigger, by roughly threefold on the same trajectory.

**The stratum that answers the question.** Averaging the two arms over all tasks hides
the mechanism. The informative stratum is the set of pairs where the **preserved arm
compacted and the unpreserved arm did not**: there, and only there, the trade is actually
exercised — the model's own reasoning in view, against a history that was never
summarised away. If preserved wins that stratum, continuity is worth more than an intact
history; if it loses, compaction is doing more damage than the reasoning trace is worth.
Report that stratum separately, with its size, alongside the whole-plan result.

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
