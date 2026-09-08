# Validate the observation/terminal distinction before a shell reader consumes it.
def uint: type == "number" and . >= 0 and floor == .;
def live_fields: [.observed_output_tokens, .observed_reasoning_tokens,
                  .observed_subagent_scope_count, .observed_unaccounted_records];
if type != "object" or (has("terminal") | not)
   or ([has("observed_output_tokens"), has("observed_reasoning_tokens"),
        has("observed_subagent_scope_count"), has("observed_unaccounted_records")]
       | all | not) then
  error("session resource lacks explicit evidence fields")
elif .status == "running" then
  if .terminal == null and (live_fields | all(uint)) then .
  else error("running resource must carry live observations and no terminal evidence") end
elif .status == "completed" or .status == "cancelled" then
  if (.terminal | type) == "object" and (live_fields | all(. == null))
     and (.terminal | has("agent_result") and has("bundle"))
     and (.terminal.is_process_error | type) == "boolean"
     and (.terminal.raw_session_tree_retained | type) == "boolean"
     and (.terminal.response | type) == "string"
     and (.terminal.teardown_diagnostics | type) == "array" then .
  else error("terminal resource must carry an ending and no live observations") end
else error("unrecognized session status") end
