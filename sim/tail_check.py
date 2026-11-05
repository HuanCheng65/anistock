"""Out-of-sample check of the us long tail on the history backfill (docs/v2-series.md §11, phase 1 step 1).

    python sim/tail_check.py INPUTS_DIR HISTORY_DB

The tail (curve, spread D(t), score/format/popularity prior) is fitted on the shows with even AniList ids (the
first weeks from the inputs, as in sim/fit_valuation.py) and checked on the odd ones: the forecast is made at the
finale, from public information only (score, format, popularity at the finale), and compared with the deflated
attention factor 3, 6, 12, 24, 36 months later. Per group (popularity fifth within the finale year; itself a
sequel or not): the mean log error against the median forecast (≈ 0 if unbiased), the ratio of mean realised to
mean forecast factor (≈ 1 if unbiased in level, what the value needs), and the share inside the 80% interval.
"""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

import sim.fit_valuation as FV  # noqa: E402
from engine import valuation as V  # noqa: E402
from sim.tail_history import History, fdate  # noqa: E402

MONTHS = (3, 6, 12, 24, 36)


def main():
    inputs, hist_path = Path(sys.argv[1]), sys.argv[2]
    fp = json.loads((ROOT / "sim" / "calibration" / "valuation_filter.json").read_text(encoding="utf-8"))
    bump = fp["finale_bump"]["us"]
    shows = FV.load(inputs)
    H = History(hist_path)
    pct, ref = H.popularity_percentiles()
    us = H.us_tail(bump)
    even = [h for h in us if h[0] % 2 == 0]
    odd = [h for h in us if h[0] % 2 == 1]
    tr = [s for s in FV.transition_stats(shows, "us", fp["drift_by_week"]["us"][2], weeks=3) if s[3] >= 10]
    st_even = tr + FV.bucket(pts for _, _, pts in even)
    curve, _ = FV.tail_curve_fit(st_even, 21)
    tail = {"trans": [round(math.exp(mu + 0.5 * v), 6) for t, mu, v, n in tr], **curve, "var": FV.tail_var_fit(st_even)}
    prior, *_ = FV.tail_prior_fit(even, pct, ref)
    fpe = {"tail": {"us": tail, "prior": prior}}
    rows = []
    for mid, m, pts in odd:
        prequel = any(ed["relationType"] == "PREQUEL" and ed["node"]["type"] == "ANIME" for ed in m["relations"]["edges"])
        e = None
        mult = V.tail_mult(fpe, m.get("averageScore"), m["format"] == "ONA",
                           H.pop_at_finale(mid, m, fdate(m["endDate"])))
        for mo in MONTHS:
            t0 = mo * 30.44 - 7
            near = [(abs(t - t0), t, f) for t, f in pts if abs(t - t0) <= 10 and f > 0]
            if not near:
                continue
            _, t, f = min(near)
            mean = V.tail_mean(tail, round(t), mult)
            d = V.tail_var(tail, round(t))
            rows.append((mo, pct.get(mid), prequel, f, mean, d))
    groups = [("all", lambda r: True)] + [(f"popularity {lo:.1f}–{hi:.1f}", (lambda lo, hi: lambda r: r[1] is not None and lo <= r[1] < hi)(lo, hi))
                                          for lo, hi in ((0, .2), (.2, .4), (.4, .6), (.6, .8), (.8, 1.01))] + \
             [("first season", lambda r: not r[2]), ("itself a sequel", lambda r: r[2])]
    print("Held-out us tail (fit on even AniList ids, checked on odd; forecast at the finale):\n")
    print("| group | " + " | ".join(f"{mo} m: n, log err, level ratio, 80% cov" for mo in MONTHS) + " |")
    print("|---|" + "---|" * len(MONTHS))
    for name, g in groups:
        cells = []
        for mo in MONTHS:
            rs = [r for r in rows if r[0] == mo and g(r)]
            if len(rs) < 10:
                cells.append("–")
                continue
            err = sum(math.log(r[3]) - (math.log(r[4]) - 0.5 * r[5]) for r in rs) / len(rs)
            lvl = (sum(r[3] for r in rs) / len(rs)) / (sum(r[4] for r in rs) / len(rs))
            cov = sum(1 for r in rs if abs(math.log(r[3]) - (math.log(r[4]) - 0.5 * r[5])) <= FV.Z80 * math.sqrt(r[5])) / len(rs)
            cells.append(f"{len(rs)}, {err:+.2f}, {lvl:.2f}, {cov:.0%}")
        print(f"| {name} | " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
