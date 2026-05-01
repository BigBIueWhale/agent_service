#!/bin/bash
# Container CMD. Three jobs:
#   (1) start the agent in a detached tmux session so ttyd can mirror it;
#   (2) foreground ttyd so the human can attach a browser to view it;
#   (3) exit the container as soon as the agent's wrapper writes /output/.done,
#       so `docker wait` on the host returns promptly.
#
# Why tmux + ttyd: ttyd's default behaviour is "spawn the command on each new
# browser connection", which we don't want — the agent should run regardless
# of whether anyone is watching. tmux gives us a single, persistent session
# that ttyd attaches to (read-only), and that any number of browsers can
# attach to concurrently without re-spawning the agent.

set -euo pipefail

# /output is bind-mounted by the host. Defensive create in case the mount
# wasn't applied in dev/test scenarios.
mkdir -p /output

# Hard requirement: the wrapper script must exist where we expect it.
if [ ! -x /usr/local/bin/run_agent.sh ]; then
    echo "FATAL: /usr/local/bin/run_agent.sh missing or not executable" >&2
    echo 97 > /output/qwen-exit-code
    touch /output/.done
    exit 97
fi

# Start the agent in a detached tmux session named "agent".
#   -x/-y set the virtual terminal size; ttyd will reflow on connect anyway.
#   `remain-on-exit on` keeps the pane visible after the wrapper exits, so a
#   late-attaching browser viewer can still see the final output.
tmux new-session -d -s agent -x 220 -y 56 /usr/local/bin/run_agent.sh
tmux set-option -t agent remain-on-exit on

# Background poller: when the wrapper signals completion, give viewers ~5s
# to see the final state, then bring ttyd down so the container exits.
(
  while [ ! -f /output/.done ]; do
    sleep 1
  done
  sleep 5
  pkill -TERM -f 'ttyd ' 2>/dev/null || true
) &

# Foreground ttyd. Read-only attach (-r on tmux) so browser viewers cannot
# inject keystrokes into the agent's session.
exec ttyd \
    --port 7681 \
    --interface 0.0.0.0 \
    --writable=false \
    --max-clients=8 \
    --browser \
    -- tmux attach -t agent -r
