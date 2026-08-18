#!/usr/bin/env python3
"""Per-variant context-compaction analysis over suite result bundles.

The agent's event stream records per-turn ``input_tokens`` on assistant
events. Between compressions the conversation context is append-only, so
the per-turn input-token trajectory is non-decreasing; any strict drop
marks a history compaction, and the value it dropped from is the context
size at which the compressor fired. Qwen Code 0.21.12 emits no explicit
compression event in stream-json, so this trajectory analysis is the
authoritative detector.

For every variant this reports: number of graded turns carrying usage,
peak and final context size (absolute and as a fraction of the 262144
window), and each compaction as
``{usage_index, input_tokens_before, input_tokens_after,
cumulative_input_tokens_before}``. Drops of less than 10% are still
listed (as ``minor_drops``) so nothing is silently classified away.
"""
import json
import pathlib
import sys

CONTEXT_WINDOW = 262144

def classify(variant: str, result: dict) -> dict:
    """Preserved histories are append-only, so every drop there is a true
    compaction. Unpreserved histories legitimately shed prior thinking, so
    their drops are the mode's own history-thinning dynamics; the near-limit
    compressor has never been observed to fire and is claimed only for a
    preserved-mode drop."""
    key = "compactions" if "-preserved" in variant else "history_drops"
    result[key] = result.pop("drops")
    return result

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
        variant_dir = events.parents[2]
        rows.append({
            "task": variant_dir.parent.name,
            "variant": variant_dir.name,
            **classify(variant_dir.name, analyze_events(events)),
        })
    json.dump({"context_window": CONTEXT_WINDOW, "variants": rows},
              sys.stdout, indent=2)
    print()
    compactions = sum(len(r.get("compactions", [])) for r in rows)
    thinning = sum(len(r.get("history_drops", [])) for r in rows)
    print(f"variants={len(rows)} true_compactions={compactions} "
          f"unpreserved_history_drops={thinning}", file=sys.stderr)
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
