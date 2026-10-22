#!/usr/bin/env python3
"""Report token and time efficiency from the local telemetry mirror.

Reads `<agentDir>/telemetry.jsonl` — the transparency mirror that every emitted
event is written to, so this needs no network and no credentials. Nothing here
is sent anywhere.

Usage:
    scripts/efficiency_report.py                 # last 7 days
    scripts/efficiency_report.py --days 30
    scripts/efficiency_report.py --tool graph    # per-tool breakdown
"""

import argparse
import json
import statistics
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path

TOKEN_KEYS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")
TIME_KEYS = ("duration_ms", "model_latency_ms", "max_model_latency_ms", "visible_ttft_ms")
COUNT_KEYS = ("tool_call_count", "tool_error_count", "turn_count", "model_call_count")


def read_events(path):
    """Yield one decoded event per line, skipping malformed lines."""
    with path.open(encoding="utf-8", errors="replace") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def percentile(values, fraction):
    """Nearest-rank percentile of a non-empty list."""
    ordered = sorted(values)
    index = min(len(ordered) - 1, int(len(ordered) * fraction))
    return ordered[index]


def summary(values):
    """Median/p90/max for a list of numbers, or None when empty."""
    if not values:
        return None
    return {
        "n": len(values),
        "median": statistics.median(values),
        "p90": percentile(values, 0.9),
        "max": max(values),
    }


def cut_off(days):
    """Earliest timestamp that belongs to the window."""
    return datetime.now(timezone.utc) - timedelta(days=days)


def parse_time(event):
    """Best-effort timestamp parse; None when absent or unreadable."""
    raw = event.get("timestamp") or event.get("time")
    if not isinstance(raw, str):
        return None
    try:
        return datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError:
        return None


def collect(path, days):
    """Split the mirror into run summaries and per-tool aggregates."""
    since = cut_off(days)
    runs = []
    tools = defaultdict(lambda: {"calls": 0, "failures": 0, "duration_ms": 0, "max_ms": 0})
    for event in read_events(path):
        name = event.get("name", "")
        props = event.get("properties") or {}
        if name == "agent run completed":
            when = parse_time(event)
            if when is None or when >= since:
                runs.append(props)
        for key, value in props.items():
            if not key.startswith("tool_") or not key.endswith("_call_count"):
                continue
            if not isinstance(value, (int, float)) or value <= 0:
                continue
            tool = key[len("tool_"):-len("_call_count")]
            entry = tools[tool]
            entry["calls"] += int(value)
            entry["failures"] += int(props.get(f"tool_{tool}_error_count") or 0)
            entry["duration_ms"] += int(props.get(f"tool_{tool}_duration_ms") or 0)
            entry["max_ms"] = max(entry["max_ms"], int(props.get(f"tool_{tool}_max_duration_ms") or 0))
    return runs, tools


def print_runs(runs):
    """Token and time distribution across runs."""
    print(f"runs: {len(runs)}")
    for key in TOKEN_KEYS + TIME_KEYS + COUNT_KEYS:
        values = [r[key] for r in runs if isinstance(r.get(key), (int, float))]
        stats = summary(values)
        if stats is None:
            continue
        unit = " ms" if key.endswith("_ms") else ""
        print(
            f"  {key:22} n={stats['n']:4}  median={stats['median']:>12,.0f}{unit}"
            f"  p90={stats['p90']:>12,.0f}{unit}  max={stats['max']:>12,.0f}{unit}"
        )
    ratio = [r for r in runs if r.get("input_tokens")]
    if ratio:
        cache = sum(r.get("cache_read_tokens") or 0 for r in ratio)
        fresh = sum(r.get("input_tokens") or 0 for r in ratio)
        if fresh + cache:
            print(f"  cache-read share      {100 * cache / (fresh + cache):.1f}% of all input tokens")
    per_call = [
        r["tool_call_count"] / r["model_call_count"]
        for r in runs
        if isinstance(r.get("model_call_count"), (int, float)) and r["model_call_count"] > 0
    ]
    stats = summary(per_call)
    if stats:
        print(f"  tools per model call  median={stats['median']:.2f}  p90={stats['p90']:.2f}")


def print_tools(tools):
    """Per-tool call, failure and duration table."""
    if not tools:
        print("no per-tool counters recorded")
        return
    print()
    print(f"  {'tool':16} {'calls':>7} {'fail':>6} {'share':>7} {'mean ms':>9} {'max ms':>9}")
    total = sum(t["calls"] for t in tools.values()) or 1
    for name, entry in sorted(tools.items(), key=lambda kv: -kv[1]["calls"]):
        mean = entry["duration_ms"] / entry["calls"] if entry["calls"] else 0
        print(
            f"  {name:16} {entry['calls']:>7} {entry['failures']:>6}"
            f" {100 * entry['calls'] / total:>6.1f}% {mean:>9,.1f} {entry['max_ms']:>9,}"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent-dir", default=str(Path.home() / ".prime" / "agent"))
    parser.add_argument("--days", type=int, default=7)
    parser.add_argument("--tool", help="show only this tool's row")
    options = parser.parse_args()

    path = Path(options.agent_dir) / "telemetry.jsonl"
    if not path.exists():
        raise SystemExit(f"no telemetry mirror at {path}")
    runs, tools = collect(path, options.days)
    print(f"# efficiency report — {path} (last {options.days} days)")
    print_runs(runs)
    if options.tool:
        tools = {k: v for k, v in tools.items() if k == options.tool}
    print_tools(tools)


if __name__ == "__main__":
    main()
