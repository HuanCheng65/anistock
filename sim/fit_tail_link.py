"""Calibrate the live cn/jp long tail (docs/v2-series.md §4.2) on the server's collector history.

    ANISTOCK_DB=<copy of the server's anistock.db> python sim/fit_tail_link.py [--weeks 26]

Run it on a COPY of the server's collector database (it has daily Bangumi subject and Annict work snapshots since
June; the local one has a single day). For every finished show with daily snapshots across its season and after
it, `sim/reconstruct.py tail_ingest` scales the post-finale collection-count increments (cn) and record increments
(jp) by the show's in-season link (series ÷ increments). This prints, by week after the finale's week, the
geometric mean of that ingested tail relative to the finale week's value, next to the model's median factor
(valuation_filter.json "tail", what the filter assumes), and the share of shows with a link.

If the two agree, switch the ingestion on (ANISTOCK_TAIL_INGEST=1 for the inputs job) and refit the cn tail on
these data; until then the close books the filter's expectation for post-finale cn/jp days (the stopgap).
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from datetime import timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "sim"))

import reconstruct as R  # noqa: E402
from engine import valuation as V  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--weeks", type=int, default=26)
    a = ap.parse_args()
    fp = json.loads((ROOT / "sim" / "calibration" / "valuation_filter.json").read_text(encoding="utf-8"))
    series = R.episode_series(min_age=3.0)
    ing = R.tail_ingest(series)
    for mk in ("cn", "jp"):
        tl = V.tail_of(fp, mk)
        by_w: dict[int, list[float]] = {}
        n = 0
        for aid, vals in ing[mk].items():
            eps = series[mk].get(aid)
            if not eps or not vals:
                continue
            last = eps[-1][0].date()
            fin_week = eps[-1][1] / 7
            if fin_week <= 0:
                continue
            n += 1
            for d, v in vals.items():
                t = (d - (last + timedelta(days=7))).days
                if 0 <= t < 7 * a.weeks and v > 0:
                    by_w.setdefault(t // 7, []).append(math.log(v / fin_week))
        print(f"## {mk}: {n} finished shows with a link and post-finale snapshots (of {len(series[mk])} in the series)\n")
        print("| week after the finale week | shows | ingested ÷ finale week (geometric mean) | model median factor |")
        print("|---|---|---|---|")
        bump = fp.get("finale_bump", {}).get(mk, 1.0)
        for w in sorted(by_w):
            v = by_w[w]
            model = V.post_finale_factor(fp, {}, mk, 0, 7 * w + 3 + 7, 1.0) if tl else float("nan")
            print(f"| {w} | {len(v)} | {math.exp(sum(v) / len(v)):.3f} | {model:.3f} |")
        print()


if __name__ == "__main__":
    main()
