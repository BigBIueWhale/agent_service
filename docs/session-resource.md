# Session resource

Successful session state responses use the same `SessionBody` JSON object. `status` is
`running`, `completed`, or `cancelled`. A reader uses the explicitly present
`terminal` value to inspect ending evidence; it never interprets zero, false, an
empty string, or an empty array as “pending.” Nullable fields must be present and
spelled `null` when absent. Unknown fields and inconsistent evidence are refused
before a durable record can authorize a current resource operation.

A committed record from a superseded schema is **uninterpreted terminal evidence**.
The reader first establishes JSON syntax and the matching terminal identity, then
attempts the exact current schema on the original bytes. A schema refusal does not
establish corruption or identify a particular past schema. It establishes that the
reader cannot interpret the record. No old fields are translated, defaulted or
admitted into a current `SessionBody`; `deny_unknown_fields` remains enforced.

Startup preserves an uninterpreted record, its result directory, publication
names, acceptance/progress controls and corresponding raw tree without mutation.
Those bytes authorize neither recovery nor cleanup. Current records still require
complete semantic, storage and ownership validation. Invalid JSON syntax, an
identity/status mismatch, unsafe filesystem metadata and contradictory current
terminal evidence remain explicit refusals; a private draft cannot become a
committed resource merely because its JSON parses.

Individual reads, bundle requests, cancellation and deletion of uninterpreted
records return HTTP 409 with `kind: "uninterpreted_terminal_record"`, the session
identity and the schema refusal detail. The collection response always contains
both `sessions` (current resources) and `uninterpreted_records` (objects containing
only `session_id` and `detail`). Neither group is omitted when empty. Listing old
evidence cannot erase or block current resources, and uninterpreted entries never
claim status, token counts, outcomes or accepted bundles.

| Location | Fields | Meaning |
|---|---|---|
| Resource | `session_id`, `status`, `started_at_unix`, `model`, `context_window`, `max_session_turns`, `archive_bytes`, `archive_sha256`, `prompt_preview` | Identity and accepted request facts |
| Resource | `progress_revision`, `progress_at_unix_ms`, `progress_phase`, `progress_message`, `progress_events` | Durable lifecycle observations, including the complete ordered history |
| Resource | `staged_bytes`, `staged_entries`, `staged_regular_files`, `output_event_bytes`, `num_turns` | Observed work counters, retained at terminal; zero means no such work observed |
| Resource | `last_event_at_unix` | Event-file modification time in Unix seconds; `null` if no trustworthy event-file timestamp was observed |
| Resource | `observed_output_tokens`, `observed_reasoning_tokens`, `observed_subagent_scope_count`, `observed_unaccounted_records` | Snapshot observations retained at terminal; all `null` only when terminal storage could not be read |
| Resource | `terminal` | `null` while running; the complete ending object once terminal |

`num_turns` counts completed billed main turns while running. At terminal it also
honors the agent's reported started-turn count, which can include a final failed
invocation without billed output. These work counters do not substitute for the
strict token accounting. `observed_output_tokens` and `observed_reasoning_tokens`
include completed billed turns across the main and subagent scopes. An unaccounted
record means unknown evidence is absent from those observations. Zero unaccounted
records does not prove a complete stream; only `terminal.agent_result` certifies
the whole pinned protocol.

Captured event JSONL uses **LF-committed records with an incomplete tail**. Each
newline commits the preceding JSON object to the captured stream. A nonempty
trailing prefix without LF contributes exactly one `observed_unaccounted_records`,
even if its bytes form valid JSON; it cannot certify a result. Malformed complete
records and unreadable served usage are also explicitly unaccounted. Independently
readable records survive such damage. The reader takes one descriptor-anchored
snapshot for observations and complete-result certification, so finalization does
not reinterpret a second, potentially different scan.

The transport is a Unix byte stream: one application write is ordered, but is not
an atomic record transaction. Complete capture proves which bytes reached the
capture process, not that every application-queued byte was sent. Orderly shutdown
must await output write callbacks; forced termination can still leave an
incomplete tail. This rule applies to captured wire output. Canonical session
journals used to restore or mutate history retain their own strict durability
and framing requirements.

The `terminal` object has these required fields:

| Field | Type | Meaning |
|---|---|---|
| `finished_at_unix` | integer | Time the execution finalizer established the ending, in Unix seconds |
| `duration_wall_ms` | integer | Finalizer wall duration in milliseconds; restart recovery measures from durable acceptance to recovery |
| `container_exit_code` | integer or null | Actual Docker-wait exit status in 0..255, or no successful wait observation |
| `agent_exit_code` | integer or null | Trusted exit-sidecar readback in 0..255, or no valid sidecar; the same original observation as Docker wait, not independent corroboration |
| `is_process_error` | boolean | Execution or mandatory evidence-handling failure; does not judge task correctness |
| `response` | string | Final service response, including any failure explanation; an empty string is a real empty answer |
| `agent_result` | object or null | Strictly certified agent result, or no certified result |
| `bundle` | object or null | Accepted bundle metadata, or no accepted bundle |
| `raw_session_tree_retained` | boolean | Final raw-tree retention decision; retained raw state can coexist with an accepted bundle |
| `teardown_diagnostics` | array of strings | Finalization diagnostics; an empty array means no diagnostics were recorded |

An `agent_result` object always contains integer `agent_duration_ms`,
`agent_api_duration_ms`, `main_output_tokens`, `main_reasoning_tokens`,
`subagent_scope_count`, and `subagent_error_count`; string `agent_result_subtype`;
and array `subagent_scopes`. A failed/unproved parse supplies no object, so it
cannot assert zero tokens or zero subagents. The subtype is the verbatim closed
terminal vocabulary documented in the README. Scope rows retain the parser's
per-scope identity, billed-turn usage, and terminal evidence; a scope's own missing
terminal report is not synthesized into an error.

A `bundle` object always contains string `sha256` and integer `compressed_bytes`,
`uncompressed_bytes`, `file_count`, and `artifacts_file_count`. Counts describe the
accepted archive, never a directory still being written. `sha256` is exactly 64
lowercase hexadecimal characters and `compressed_bytes` is positive. Bundle
absence, zero artifact files, and no bundle decision yet have three distinct shapes:

```json
{"terminal": null}
```

```json
{"terminal": {"bundle": null}}
```

```json
{"terminal": {"bundle": {"sha256": "1111111111111111111111111111111111111111111111111111111111111111", "compressed_bytes": 100, "uncompressed_bytes": 50, "file_count": 7, "artifacts_file_count": 0}}}
```

These examples show only the relevant fields, not complete resources. A running
record cannot carry any ending object. A terminal record retains the final snapshot observations, even if its
`agent_result` is null. A complete observation group and an entirely absent group
are distinct; partially populated groups are invalid. The same checks apply to private terminal
drafts, committed records, and startup sweeps before cleanup can use their claims.

To inspect final evidence from a downloaded resource:

```sh
jq '.terminal' session.json
jq '.terminal.agent_result' session.json
jq '.terminal.bundle' session.json
```

`session.sh` validates and displays the resource. `bundle.sh` distinguishes a
pending decision from a terminal refusal, then validates accepted bundle metadata
and the downloaded byte commitment. Neither script turns absent evidence into zero.

The result archive contains the final workspace, artifacts, prompt/budget controls,
response, event stream, and available output sidecars. `output/qwen-exit-code`
exists only when Docker wait supplied an actual exit status; its canonical decimal
integer ends with one newline. No recovery path manufactures that file. An absent
sidecar means no recorded exit observation and does not prevent forensic bundling.
The accepted archive's own hash and final retention/teardown decisions are served
in the terminal resource; they cannot be embedded as a self-referential hash in
the archive they describe.
