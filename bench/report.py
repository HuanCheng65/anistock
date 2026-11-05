"""Summarise bench/out/results.jsonl: median of repeated runs per (impl, multiplier)."""
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

path = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).parent / "out" / "results.jsonl")
runs = defaultdict(list)
for line in path.read_text().splitlines():
    if line.strip():
        r = json.loads(line)
        if r.get("collapsed"):
            continue
        impl, mult = r["label"].split()
        runs[(impl, int(mult.rstrip("x")))].append(r)

cols = [("orders_per_s", "下单/s"), ("npc_per_s", "NPC/s"), ("ws_msgs_per_s", "推送/s"), ("rtt_p50_ms", "p50"),
        ("rtt_p99_ms", "p99"), ("rtt_max_ms", "max"), ("ws_lag_p99_ms", "推送p99"), ("server_cpu", "CPU核"),
        ("server_rss_mb", "内存MB"), ("ws_dropped", "掉线")]
print(f"{'实现':<7}{'负载':>4} {'次数':>3} " + " ".join(f"{h:>9}" for _, h in cols))
for (impl, mult) in sorted(runs, key=lambda k: (["python", "go", "rust", "go-v2", "rust-v2"].index(k[0]), k[1])):
    rs = runs[(impl, mult)]
    vals = [statistics.median(r[k] for r in rs) for k, _ in cols]
    print(f"{impl:<7}{mult:>3}x {len(rs):>4} " + " ".join(f"{v:>9.1f}" for v in vals))
