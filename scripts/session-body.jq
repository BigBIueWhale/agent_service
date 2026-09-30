# Validate physical observations and terminal claims before a shell reader consumes them.
def uint: type == "number" and . >= 0 and floor == .;
# Usage and reported turns share the stream contract's exact integer domain.
def exact_uint: uint and . <= 9007199254740991;
def has_only($names): type == "object" and keys == ($names | sort);
def served_usage:
  has_only(["promptTokenCount", "candidatesTokenCount", "thoughtsTokenCount",
            "cachedContentTokenCount", "totalTokenCount"])
  and ([.[]] | all(exact_uint))
  and .promptTokenCount + .candidatesTokenCount == .totalTokenCount
  and .thoughtsTokenCount <= .candidatesTokenCount
  and .cachedContentTokenCount <= .promptTokenCount;
def request_usage:
  has_only(["requests", "usageReports", "unfinalizedRequests", "unreportedUsageRequests", "usage"])
  and ([.requests, .usageReports, .unfinalizedRequests, .unreportedUsageRequests] | all(exact_uint))
  and .requests == .usageReports + .unfinalizedRequests + .unreportedUsageRequests
  and (if .usageReports == 0 then .usage == null else (.usage | served_usage) end);
def empty_usage:
  {requests: 0, usageReports: 0, unfinalizedRequests: 0, unreportedUsageRequests: 0, usage: null};
def sum_request_scopes:
  reduce .[].usage as $s (empty_usage;
    .requests += $s.requests | .usageReports += $s.usageReports |
    .unfinalizedRequests += $s.unfinalizedRequests |
    .unreportedUsageRequests += $s.unreportedUsageRequests |
    if $s.usage == null then .
    elif .usage == null then .usage = $s.usage
    else .usage |= {
      promptTokenCount: (.promptTokenCount + $s.usage.promptTokenCount),
      candidatesTokenCount: (.candidatesTokenCount + $s.usage.candidatesTokenCount),
      thoughtsTokenCount: (.thoughtsTokenCount + $s.usage.thoughtsTokenCount),
      cachedContentTokenCount: (.cachedContentTokenCount + $s.usage.cachedContentTokenCount),
      totalTokenCount: (.totalTokenCount + $s.usage.totalTokenCount)}
    end);
def nonempty_string: type == "string" and length > 0;
# The accepted deliverables: paths the service admitted under its one rule, so a
# reader needs only their shape -- distinct non-empty strings -- to rely on them.
def declared_paths:
  type == "array" and all(.[]; nonempty_string) and length == (unique | length);
# Whether every entry of $part appears in $whole, in the same order.
def ordered_subset($part; $whole):
  reduce $part[] as $wanted ({rest: $whole, ok: true};
    if .ok then
      (.rest | index([$wanted])) as $at
      | if $at == null then .ok = false else .rest = .rest[$at + 1:] end
    else . end)
  | .ok;
def valid_child_scope:
  has_only(["tool_use_id", "tool_name", "reported_num_turns", "is_error", "subtype", "error_message"])
  and (.tool_use_id | nonempty_string) and (.tool_name | nonempty_string)
  and (if .reported_num_turns == null then
    .is_error == null and .subtype == null and .error_message == null
  else (.reported_num_turns | exact_uint) and (.is_error | type == "boolean")
    and (.subtype | nonempty_string)
    and (if .is_error then (.error_message | nonempty_string)
         else .error_message == null or (.error_message | nonempty_string) end)
  end);
def valid_observations:
  (has("num_turns") and has("observed_usage") and has("observed_subagent_scope_count")
   and has("observed_unaccounted_records"))
  and (if .observed_usage == null then
    .num_turns == null and .observed_subagent_scope_count == null and .observed_unaccounted_records == null
  else (.observed_usage | request_usage)
    and (.num_turns == null or (.num_turns | exact_uint))
    and (.observed_subagent_scope_count | uint) and (.observed_unaccounted_records | uint)
  end);
def certified_observations:
  .terminal.agent_result == null or
  (.terminal.agent_result as $r |
   ($r | type == "object") and (.observed_usage | request_usage)
   and .observed_unaccounted_records == 0
   and ($r.num_turns | exact_uint) and .num_turns == $r.num_turns
   and ($r.main_kv_scope | nonempty_string)
   and ($r.usage | request_usage) and $r.usage.unfinalizedRequests == 0
   and .observed_usage == $r.usage
   and ($r.request_scopes | type == "array")
   and ($r.request_scopes | all(has_only(["kv_scope", "usage"])
        and (.kv_scope | nonempty_string) and (.usage | request_usage) and .usage.requests > 0))
   and ([$r.request_scopes[].kv_scope] | length == (unique | length))
   and ($r.request_scopes | sum_request_scopes) == $r.usage
   and ($r.subagent_scopes | type == "array") and ($r.subagent_scopes | all(valid_child_scope))
   and ([$r.subagent_scopes[].tool_use_id] | length == (unique | length))
   and ($r.subagent_scopes | length) == $r.subagent_scope_count
   and ([$r.subagent_scopes[] | select(.is_error == true)] | length) == $r.subagent_error_count
   and .observed_subagent_scope_count == $r.subagent_scope_count
   and ($r | has("missing_deliverables"))
   and ($r.missing_deliverables == null or
        (($r.missing_deliverables | type == "array" and length > 0)
         and ordered_subset($r.missing_deliverables; .deliverables)))
   and (if (.progress_events | type) == "array" and (.progress_events | length) > 0 then
     .progress_events[-1].counters.physical_requests <= $r.usage.requests
   else true end));
if type != "object" or (has("terminal") | not) or (valid_observations | not) then
  error("session resource lacks consistent physical observations or reported turns; inspect the resource with its matching release")
elif (has("deliverables") | not) or (.deliverables | declared_paths | not) then
  error("session resource lacks its accepted deliverables list; inspect the resource with its matching release")
elif .status == "running" then
  if .terminal == null and .observed_usage != null then .
  else error("running resource must carry live observations and no terminal evidence") end
elif .status == "ended" or .status == "cancelled" then
  if (.terminal | type) == "object" and certified_observations
     and (.terminal | has("agent_result") and has("bundle"))
     and (.terminal.is_process_error | type) == "boolean"
     and (.terminal.raw_session_tree_retained | type) == "boolean"
     and (.terminal.response | type) == "string"
     and (.terminal.teardown_diagnostics | type) == "array" then .
  else error("terminal resource must carry an ending with consistent physical evidence; inspect its capture and progress records") end
else error("unrecognized session status; use the reader matching this release") end
