# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Grid-search the macro knobs and pick a stable configuration by rule.

    uv run sim/sweep.py        # -> sim/out/sweep.csv and sim/params_recommended.toml

Every run uses the same seed and data. Each run is scored on the last 8 weeks:
  growth_mean  mean annualised M2 growth (28-day window, same yardstick for every run)
  growth_std   std of that growth, weekly samples
  rate_swing   max - min policy rate over the last 8 weeks (oscillation amplitude; small fine-tuning is fine)
Rule: among runs whose growth_std is at most the baseline's + 1 point (measurement noise) and whose
rate_swing is at most 1.5 points,
pick the one whose growth_mean is closest to the target.
"""

from __future__ import annotations

import copy
import csv
import itertools
import json
import statistics
import sys
import tomllib
from datetime import date
from multiprocessing import Pool
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import simulate as S  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
GRID = {
    "central_bank.max_step": [0.0025, 0.005, 0.01],
    "central_bank.growth_lookback_days": [28, 56],
    "company.corp_tax": [0.25, 0.35],
    "government.transfer_ratio": [0.25, 0.5],
    "revenue.demand_base_week": [120000, 80000, 60000, 40000],
}

_DATA = {}


def _load():
    base = tomllib.loads((ROOT / "sim" / "params.toml").read_text(encoding="utf-8"))
    start = date.fromisoformat(base["window"]["start"])
    shows, end = S.load_shows(ROOT / "data" / "anistock.db", base, start, None)
    markets = {int(k): v for k, v in json.loads((ROOT / "data" / "derived" / "markets.json")
                                                .read_text(encoding="utf-8"))["series"].items()}
    aud = S.load_audiences(ROOT / "data" / "anistock.db")
    _DATA.update(base=base, start=start, end=end, shows=shows, markets=markets, aud=aud)


def _set(p, path, value):
    *secs, key = path.split(".")
    node = p
    for s in secs:
        node = node[s]
    node[key] = value


def score(macro: list[dict], target: float) -> dict:
    m2 = [r["m2"] for r in macro]
    g = [(m2[i] / m2[i - 28]) ** (365 / 28) - 1 for i in range(28, len(m2))]
    weekly = g[::7]
    tail = weekly[-8:]
    rates = [r["rate"] for r in macro][::7]
    steps = [b - a for a, b in zip(rates, rates[1:]) if abs(b - a) > 1e-9][4:]
    reversals = sum(1 for a, b in zip(steps, steps[1:]) if (a > 0) != (b > 0))
    swing = max(rates[-8:]) - min(rates[-8:])
    return {
        "growth_mean": statistics.mean(tail),
        "growth_std": statistics.pstdev(tail),
        "reversals": reversals,
        "rate_swing": swing,
        "rate_final": macro[-1]["rate"],
        "rate_max": max(r["rate"] for r in macro),
        "bills_final": macro[-1]["bills"],
        "gap_to_target": abs(statistics.mean(tail) - target),
    }


def run(combo: dict) -> dict:
    p = copy.deepcopy(_DATA["base"])
    for k, v in combo.items():
        _set(p, k, v)
    try:
        res = S.simulate(_DATA["shows"], _DATA["start"], _DATA["end"], p, _DATA["markets"], _DATA["aud"])
    except AssertionError as e:
        return {**combo, "error": str(e)}
    return {**combo, **score(res["macro"], p["central_bank"]["m2_growth_target"])}


def main() -> None:
    _load()
    keys = list(GRID)
    combos = [dict(zip(keys, vals)) for vals in itertools.product(*GRID.values())]
    with Pool(initializer=_load) as pool:
        rows = pool.map(run, combos)
    out = ROOT / "sim" / "out" / "sweep.csv"
    out.parent.mkdir(parents=True, exist_ok=True)
    cols = keys + ["growth_mean", "growth_std", "reversals", "rate_swing", "rate_final", "rate_max", "bills_final",
                   "gap_to_target", "error"]
    with out.open("w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=cols)
        w.writeheader()
        for r in rows:
            w.writerow({c: r.get(c, "") for c in cols})

    base = _DATA["base"]
    baseline = next(r for r in rows if all(r[k] == _get(base, k) for k in keys))
    # the 28-day growth measure itself is noisy (~6 points, weekly dividend lumps), so "no worse than baseline"
    # allows one point of slack
    ok = [r for r in rows if "error" not in r and r["growth_std"] <= baseline["growth_std"] + 0.01
          and r["rate_swing"] <= 0.015]
    print(f"runs={len(rows)} errors={sum(1 for r in rows if 'error' in r)} passing={len(ok)}")
    print(f"baseline: growth {baseline['growth_mean']:.1%} ± {baseline['growth_std']:.1%}, "
          f"rate {baseline['rate_final']:.2%}, swing {baseline['rate_swing']:.2%}")
    for r in sorted(ok, key=lambda r: r["gap_to_target"])[:8]:
        print("  " + "  ".join(f"{k.split('.')[-1]}={r[k]}" for k in keys) +
              f"  -> growth {r['growth_mean']:.1%} ± {r['growth_std']:.1%} rate {r['rate_final']:.2%} swing {r['rate_swing']:.2%}")
    if not ok:
        print("no configuration passes the rule")
        return
    best = min(ok, key=lambda r: r["gap_to_target"])
    rec = copy.deepcopy(base)
    for k in keys:
        _set(rec, k, best[k])
    (ROOT / "sim" / "params_recommended.toml").write_text(
        "# Written by sim/sweep.py: the stable macro configuration picked by rule from sim/out/sweep.csv.\n"
        f"# growth (last 8 weeks) {best['growth_mean']:.1%} ± {best['growth_std']:.1%}, final rate "
        f"{best['rate_final']:.2%}, rate swing over the last 8 weeks {best['rate_swing']:.2%}\n\n" + to_toml(rec), encoding="utf-8")
    print("wrote sim/params_recommended.toml")


def _get(p, path):
    node = p
    for s in path.split("."):
        node = node[s]
    return node


def to_toml(d: dict, prefix: str = "") -> str:
    """Minimal TOML writer for this params file (scalars, strings, lists, one level of tables)."""
    def val(v):
        if isinstance(v, bool):
            return "true" if v else "false"
        if isinstance(v, str):
            return json.dumps(v, ensure_ascii=False)
        if isinstance(v, list):
            return "[" + ", ".join(val(x) for x in v) + "]"
        return repr(v)
    lines = [f"{k} = {val(v)}" for k, v in d.items() if not isinstance(v, dict)]
    for k, v in d.items():
        if isinstance(v, dict):
            lines.append(f"\n[{prefix}{k}]")
            lines += [f"{kk} = {val(vv)}" for kk, vv in v.items()]
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    main()
