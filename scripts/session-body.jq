# Validate the observation/terminal distinction before a shell reader consumes it.
def uint: type == "number" and . >= 0 and floor == .;
def observation_fields: [.observed_output_tokens, .observed_reasoning_tokens,
                  .observed_subagent_scope_count, .observed_unaccounted_records];
def valid_observations:
  (observation_fields | all(. == null))
  or ((observation_fields | all(uint)) and .observed_reasoning_tokens <= .observed_output_tokens);
def certified_observations:
  .terminal.agent_result == null or
  ((.terminal.agent_result | type) == "object"
   and (observation_fields | all(uint))
   and .observed_unaccounted_records == 0
   and (.terminal.agent_result.subagent_scopes | type) == "array"
   and .observed_subagent_scope_count == .terminal.agent_result.subagent_scope_count
   and .observed_output_tokens == (.terminal.agent_result.main_output_tokens + ([.terminal.agent_result.subagent_scopes[].output_tokens] | add // 0))
   and .observed_reasoning_tokens == (.terminal.agent_result.main_reasoning_tokens + ([.terminal.agent_result.subagent_scopes[].reasoning_tokens] | add // 0)));
if type != "object" or (has("terminal") | not)
   or ([has("observed_output_tokens"), has("observed_reasoning_tokens"),
        has("observed_subagent_scope_count"), has("observed_unaccounted_records")]
       | all | not) then
  error("session resource lacks explicit evidence fields")
elif .status == "running" then
  if .terminal == null and (observation_fields | all(uint)) and valid_observations then .
  else error("running resource must carry live observations and no terminal evidence") end
elif .status == "completed" or .status == "cancelled" then
  if (.terminal | type) == "object" and valid_observations and certified_observations
     and (.terminal | has("agent_result") and has("bundle"))
     and (.terminal.is_process_error | type) == "boolean"
     and (.terminal.raw_session_tree_retained | type) == "boolean"
     and (.terminal.response | type) == "string"
     and (.terminal.teardown_diagnostics | type) == "array" then .
  else error("terminal resource must carry an ending and a complete or absent observation group") end
else error("unrecognized session status") end
