# Java CLI SDK record framing and failure ownership

The Java CLI transport owns stdout byte interpretation for every single-response
and multi-response caller. Records are committed by LF and decoded with strict
UTF-8 after framing. A buffered I/O read is not a record-size limit. Terminated
ASCII blank lines and CRLF are accepted; any nonempty EOF suffix and malformed
UTF-8 are refused with the physical line and a diagnostic action. Stdin uses an
explicit UTF-8 encoder so host defaults cannot change prompts.

The shared concurrency helper must propagate callback, timeout and interruption
failures to its caller. It must preserve the actual exception cause, restore the
waiting thread's interrupt flag and cancel its owned task. A failed prompt cannot
return successfully because a helper logged its error. The transport latches the
first failure, closes write admission and stops its process/read ownership before
subsequent operations can consume or emit more protocol data. A successful record
callback remains a normal boundary in a persistent multi-turn process.

The transport owns the read task's wait and publishes failure before interrupting
that task. A callback that handles interruption cannot consume the next record.
Shared executor dispatch refuses inline execution, which would bypass timed waits
when a caller-runs queue is full, for both default and configured executors. Start
and close serialize process ownership; rejected stderr-reader admission closes
the process created by that start. These are runtime ownership rules, not executor
configuration workarounds.

This work belongs in shared Java CLI interpretation and benefits ordinary and
long vLLM-backed sessions. A backend cannot repair stdout bytes replaced or lost
by the SDK. It cannot benefit direct vLLM clients that do not use this SDK. No
model request shape, stdout record shape, canonical runtime recording, or resume
replay is changed. The Java ACP client has a separate JSON-RPC transport and is
not the reader of this CLI evidence stream.

Framing and error propagation are necessary parts of the full Java repair.
Versioned schema admission, model request/response replay and complete delivery
remain required. The current Session projects recognized messages into Java POJOs
and routes evidence through a generic callback; this does not establish evidence
interpretation. A future admission change must bind to the authoritative v5
schema before state changes and controls, preserve evidence structurally outside
history, and distinguish root/child results and live input ownership. A framing
fix cannot be used as proof of full record completeness.

Verification must respect the owner's no-build instruction. Use source-level
JDK execution and exact-source inspection without Maven/package/application
builds, and retain permanent Java regression tests for the release gate. Source
method execution with stubbed dependencies is narrower than SDK integration and
must be reported as such. Exercise exact UTF-8 across byte boundaries, large
records, malformed middle records with preserved prefix, every nonempty EOF
suffix, callback failure propagation, timeout/interruption and ordinary multiple
successful operations. Two clean final self-review passes and independent
verification are required before committing.
