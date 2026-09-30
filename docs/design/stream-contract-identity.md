# Stream contract identity

The structural compiler reads the selected contract's root `$id`. It requires
the owned `urn:agent-service:stream-contract:` namespace and a positive decimal
revision without leading zeros. The revision is an identity string, not a
machine-sized counter. The generated binding and schema inventory preserve
that exact identity. Nested resource identities are unsupported. Exact
producer/reader agreement is bound to the SHA-256 of the contract bytes;
accepting a new definition at build time does not admit a different runtime
contract.

## Scope

This binding serves every consumer of the shared native engine, including the
service and certifier. Direct vLLM callers do not consume this client stream
contract, so the binding has no backend counterpart. It changes neither the
wire schema nor model input.
