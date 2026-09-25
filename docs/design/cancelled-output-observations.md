# Delivered output and cancellation

A provider chunk already delivered to a consumer is an observation. Cancellation
stops subsequent generation and tool execution; it cannot erase that observation.
Turn projects the whole chunk, including reasoning, text, completed calls,
incomplete arguments, finish reason and served usage, before publishing
UserCancelled and closing the provider iterator. This also applies to redraws.

Headless main and queued-continuation loops consume those projected observations
through the shared output adapter. They finalize the partial message and route
cancellation before classifying the reply, starting another turn or dispatching
a tool. ACP's shared send consumer delivers an observed chunk to its callback
before settling cancellation; every ACP worker uses that same boundary. All four
ACP callers publish served usage before a stopped return, and the background
notification worker publishes its buffered response before its cancelled end-turn.

The interactive event consumer accepts attempt-start metadata and passes it to
the shared dual output adapter. It continues through the reply and handles
cancellation by flushing observed output and clearing queued tool requests. Local
UI cancellation still suppresses late rendering through its existing turn guard;
dual-output receives the observations first. Changing that rendering guard requires
proving ownership when a replacement turn starts.

Canonical history remains owned by GeminiChat's durable commit and versioned
recording. These consumer changes do not rewrite history or change the request
body, record schema, or resume projection. Accepted and abandoned decisions remain
facts recorded by their owner. Resume reads the canonical runtime recording,
not the service stdout copy at `output/events.jsonl`. Full and indexed restore
and fork reconstruction continue to project the same accepted history, with
request and response evidence structurally excluded from conversation turns.
The backend has already delivered these chunks; this omission is in shared client
consumption and requires no backend workaround.

Verify cancellation before delivery and between projected events, iterator
closure without another provider pull, exact terminal arguments and usage, main
and drain output, ordinary interactive replies, and ACP worker cancellation.
Cancelled tools must not execute. Source-only tests and no-emit typechecking are
permitted; builds and deployment remain reserved for the owner.

The source verification covers 180 headless tests, 614 ACP tests, 222 interactive
hook tests, and 73 core/attempt/canonical-history tests. The unchanged version 5
schema needs no fake-provider migration. Core no-emit checking passes; the wider
source/test graph has no diagnostics in the edited files and nine diagnostics in
unmodified dependencies, all also present against the baseline. The transformer
framework and all 35 semantic concerns remain checked.
