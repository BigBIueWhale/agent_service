# Session resource

Every session endpoint returns the same `SessionBody` JSON object. `status` is
`running`, `completed`, or `cancelled`. A reader uses the explicitly present
`terminal` value to inspect ending evidence; it never interprets zero, false, an
empty string, or an empty array as “pending.” Nullable fields must be present and
spelled `null` when absent. Unknown fields and inconsistent evidence are refused
when durable records are read or written.

| Location | Fields | Meaning |
|---|---|---|
| Resource | `session_id`, `status`, `started_at_unix`, `model`, `context_window`, `max_session_turns`, `archive_bytes`, `archive_sha256`, `prompt_preview` | Identity and accepted request facts |
| Resource | `progress_revision`, `progress_at_unix_ms`, `progress_phase`, `progress_message`, `progress_events` | Durable lifecycle observations, including the complete ordered history |
| Resource | `staged_bytes`, `staged_entries`, `staged_regular_files`, `output_event_bytes`, `num_turns` | Observed work counters, retained at terminal; zero means no such work observed |
| Resource | `last_event_at_unix` | Event-file modification time in Unix seconds; `null` if no trustworthy event-file timestamp was observed |
| Resource | `observed_output_tokens`, `observed_reasoning_tokens`, `observed_subagent_scope_count`, `observed_unaccounted_records` | Integer live observations while running; all `null` once terminal |
| Resource | `terminal` | `null` while running; the complete ending object once terminal |

`num_turns` counts completed billed main turns while running. At terminal it also
honors the agent's reported started-turn count, which can include a final failed
invocation without billed output. These work counters do not substitute for the
strict token accounting. `observed_output_tokens` and `observed_reasoning_tokens`
include completed billed turns across the main and subagent scopes. An unaccounted
record means their totals are short by an unknown amount.

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
record cannot carry any ending object. A terminal record cannot carry any live
observation, including a live zero. The same checks apply to private terminal
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
