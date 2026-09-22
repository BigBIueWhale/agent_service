# Session resource

Successful session state responses use the same `SessionBody` JSON object. `status` is
`running`, `completed`, or `cancelled`. A reader uses the explicitly present
`terminal` value to inspect ending evidence; it never interprets zero, false, an
empty string, or an empty array as “pending.” Nullable fields must be present and
spelled `null` when absent. Unknown fields and inconsistent evidence are refused
when durable records are read or written.

The service reads only the record formats it writes. It translates, defaults and
skips no other format, and it never moves or deletes a record it cannot read.
The records it writes live in the subtree named for their schema,
`<results-root>/schema-<n>/`, so a release that changes the schema begins on an
empty subtree of its own and the records written under earlier schemas stay
where they are, readable by the releases that wrote them. Startup first
completes interrupted deletions. Before acceptance recovery or any startup sweep
acts on that subtree, startup reads every result directory's committed terminal,
acceptance, progress and cancellation records with the service's strict
readers. A directory whose name is not a session
handle (`s-` followed by 64 lowercase hexadecimal characters), or whose records
do not read, makes startup refuse. One error names
every such directory, the reason, and any raw state tree beside it. The operator
removes those paths from the runtime directories (moving them elsewhere keeps
them) and starts again. After startup, a record that does not read is an
internal invariant violation: the request fails with HTTP 500 `internal`, naming
the record and the refusal. The collection response is `{"sessions": [...]}`.

| Location | Fields | Meaning |
|---|---|---|
| Resource | `session_id`, `status`, `started_at_unix`, `model`, `context_window`, `max_session_turns`, `archive_bytes`, `archive_sha256`, `prompt_preview` | Identity and accepted request facts |
| Resource | `release` | The release that accepted the session: `implementation_commit`, `images` (`agent`, `relay`, `capture`, `broker`, `service`) and `backend` (`image_id`, `launch_profile`) |
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

`release` is recorded in the acceptance record (schema version 4) when the
session is accepted and carried unchanged into every state of the resource,
the terminal record included; a terminal read refuses a terminal that names
another release than its acceptance. Its values are read at startup from the
locks the service validates, never typed: the implementation commit and all
five image IDs from `config/release.lock.json`, which must name, by hash, the
stack lock compiled into the service and pin the same agent, relay, capture
and broker images, and the backend image ID and launch profile from that stack
lock. The launch profile is `.backend.profile_label`, the profile label the
backend's container and cache volume carry, which the service checks on both;
it deliberately lags the backend image, whose own profile label no lock the
service validates records, so the record names the image by its ID and the
profile it carries as the launch profile. A service whose release lock does
not describe it refuses to start. A session recovered after a restart keeps
the release that accepted it. Version 3 records called the launch profile
`profile`; like every older format they are refused at startup, which names
each such result directory for the operator to move aside.

The `terminal` object has these required fields:

| Field | Type | Meaning |
|---|---|---|
| `finished_at_unix` | integer | Time the execution finalizer established the ending, in Unix seconds |
| `duration_wall_ms` | integer | Finalizer wall duration in milliseconds; restart recovery measures from durable acceptance to recovery |
| `container_exit_code` | integer or null | Actual Docker-wait exit status in 0..255, or no successful wait observation |
| `agent_exit_code` | integer or null | Trusted exit-sidecar readback in 0..255, or no valid sidecar; the same original observation as Docker wait, not independent corroboration |
| `is_process_error` | boolean | A completed session's exit disagrees with the exit the stream contract's terminal table gives the subtype its certified record carries, no terminal record was certified, or mandatory evidence handling failed; an ending the run recorded and exited with, whichever ending it is, is not one, a cancelled session's exit is the cancellation's and is not compared, and this does not judge task correctness |
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
