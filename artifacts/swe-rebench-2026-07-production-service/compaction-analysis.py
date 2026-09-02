#!/usr/bin/env python3
"""Per-run context-trajectory analysis over suite result bundles.

The agent's event stream records per-turn ``input_tokens`` on assistant
events. That trajectory is the only context-size signal the stream
carries: Qwen Code 0.21.12 emits no explicit compression event in
stream-json, so a strict drop between consecutive usage-bearing turns is
the sole observable that the rendered prompt got smaller, and the value
it dropped from is the context size it shrank from.

What a drop is and is not. A drop is a measurement: the rendered prompt
for turn N was smaller than for turn N-1. It is not a labelled cause.
The stream records no reason for a shrink, and this script does not
infer one -- it reports every drop, regardless of size, so nothing is
silently classified away. Attributing a drop to any particular mechanism
requires evidence this trajectory does not contain.

For every run this reports: number of graded turns carrying usage, peak
and final context size (absolute and as a fraction of the 262144
window), and each drop as ``{usage_index, input_tokens_before,
input_tokens_after, cumulative_input_tokens_before}``.
"""
import json
import pathlib
import sys

CONTEXT_WINDOW = 262144

def analyze_events(path: pathlib.Path) -> dict:
    trajectory = []
    with path.open() as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            event = json.loads(line)
            if event.get("type") != "assistant":
                continue
            usage = (event.get("message") or {}).get("usage") or {}
            tokens = usage.get("input_tokens") or 0
            if tokens > 0:
                trajectory.append(tokens)
    drops = []
    cumulative = 0
    for index, tokens in enumerate(trajectory):
        previous = trajectory[index - 1] if index >= 1 else None
        if previous is not None and tokens < previous:
            drops.append({
                "usage_index": index,
                "input_tokens_before": previous,
                "input_tokens_after": tokens,
                "dropped_tokens": previous - tokens,
                "cumulative_input_tokens_before": cumulative,
                "window_fraction_before": round(previous / CONTEXT_WINDOW, 4),
            })
        cumulative += tokens
    peak = max(trajectory, default=0)
    return {
        "turns_with_usage": len(trajectory),
        "peak_input_tokens": peak,
        "peak_window_fraction": round(peak / CONTEXT_WINDOW, 4),
        "final_input_tokens": trajectory[-1] if trajectory else 0,
        "cumulative_input_tokens": cumulative,
        "drops": drops,
    }

def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "full-suite-v3/runs")
    rows = []
    for events in sorted(root.glob("*/0*/bundle/output/events.jsonl")):
        run_dir = events.parents[2]
        rows.append({
            "task": run_dir.parent.name,
            "run": run_dir.name,
            **analyze_events(events),
        })
    json.dump({"context_window": CONTEXT_WINDOW, "runs": rows},
              sys.stdout, indent=2)
    print()
    drops = sum(len(row["drops"]) for row in rows)
    print(f"runs={len(rows)} input_token_drops={drops}", file=sys.stderr)
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
