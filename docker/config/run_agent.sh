#!/bin/bash
# Launched inside the tmux session by agent_init.sh.
#
# Reads the prompt from the read-only control mount at /run/agent/prompt.txt,
# invokes Qwen Code CLI in stream-json headless mode, and tees the JSONL
# event stream to /output/events.jsonl. The host parses the last `result`
# event from that file once docker reports the container has exited.

set -uo pipefail

cd /workspace
mkdir -p /output

PROMPT_FILE=/run/agent/prompt.txt
if [ ! -f "$PROMPT_FILE" ]; then
    echo "FATAL: $PROMPT_FILE missing — host should have bind-mounted /run/agent" >&2
    echo 96 > /output/qwen-exit-code
    touch /output/.done
    exit 96
fi
PROMPT="$(cat "$PROMPT_FILE")"

PROMPT_BYTES=$(wc -c < "$PROMPT_FILE" | tr -d ' ')
WORKSPACE_ENTRIES=$(find /workspace -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')

cat <<EOF
============================================================
qwen3.6 agent run
  Model:         ${OPENAI_MODEL:-<unset>}
  Endpoint:      ${OPENAI_BASE_URL:-<unset>}
  Approval:      yolo (every tool call auto-approved)
  Max turns:     ${AGENT_MAX_TURNS:-200}
  Workspace:     /workspace ($WORKSPACE_ENTRIES top-level entries)
  Artifacts:     /artifacts (empty; bundled and returned at end of run)
  Prompt bytes:  $PROMPT_BYTES
============================================================

----- Prompt -----
$PROMPT
------------------

EOF

# Sanity-check that the model server is reachable before we spend a turn
# on a request that will just time out. We don't fail-fast on a non-200
# health response — vLLM might not expose a /health, and a flaky proxy
# might return 502 transiently.
echo "Probing model server at ${OPENAI_BASE_URL:-<unset>} ..."
if curl --max-time 5 -fsS "${OPENAI_BASE_URL:-http://invalid:0}/models" -o /tmp/models.json 2>/dev/null; then
    echo "  reachable; advertised models: $(jq -r '.data[].id' /tmp/models.json 2>/dev/null | tr '\n' ',' | sed 's/,$//')"
else
    echo "  WARNING: /v1/models probe failed; the agent will try anyway."
fi
echo

EXIT=0
qwen --approval-mode yolo \
     --max-session-turns "${AGENT_MAX_TURNS:-200}" \
     --output-format stream-json \
     --include-partial-messages \
     -p "$PROMPT" \
  | tee /output/events.jsonl
EXIT="${PIPESTATUS[0]}"

EVENTS_LINES=$(wc -l < /output/events.jsonl 2>/dev/null | tr -d ' ' || echo 0)

cat <<EOF

============================================================
qwen exited with code: $EXIT
events captured:       $EVENTS_LINES lines
events.jsonl path:     /output/events.jsonl
============================================================
EOF

# Surface the last result line for the human watcher's convenience —
# the host parses the same file programmatically.
if [ -f /output/events.jsonl ]; then
    LAST_RESULT="$(grep '"type":"result"' /output/events.jsonl | tail -1)"
    if [ -n "$LAST_RESULT" ]; then
        echo
        echo "----- Final result event -----"
        echo "$LAST_RESULT" | jq -r '. as $r | "is_error: \($r.is_error)\nresult:\n\($r.result // ($r.error.message // "<no message>"))"'
        echo "------------------------------"
    fi
fi

echo "$EXIT" > /output/qwen-exit-code
touch /output/.done
exit "$EXIT"
