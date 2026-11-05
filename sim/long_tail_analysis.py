# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Long-tail and sequel analysis over the history backfill (sim/history_backfill.py). Prints markdown tables.

    HTTPS_PROXY=$https_proxy uv run --no-project --with numpy python sim/long_tail_analysis.py HISTORY_DB [today]

us attention as the game defines it (sim/params.toml): 2 * daily list adds + trending. cn: Bangumi episode comments
(the game's metric) and Bangumi subject comments (a proxy for new collections). Findings: sim/calibration/long_tail_sequel.md
"""

from __future__ import annotations

import json
import math
import sqlite3
import sys
from collections import defaultdict
from datetime import date, datetime, timedelta, timezone

import numpy as np

JST = timezone(timedelta(hours=9))
DB = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
TODAY = date.fromisoformat(sys.argv[2]) if len(sys.argv) > 2 else date(2026, 11, 4)
TAIL = [1, 2, 3, 4.5, 6, 9, 12, 15, 18, 21, 24, 30, 36, 42, 48, 60, 72, 84, 96]
SERIES = {"TV", "TV_SHORT", "ONA"}


def fdate(d):
    if not d or not d.get("year") or not d.get("month"):
        return None
    return date(d["year"], d["month"], d.get("day") or 15)


def q(xs, p):
    xs = sorted(x for x in xs if x is not None and not math.isnan(x))
    if not xs:
        return float("nan")
    k = (len(xs) - 1) * p
    lo = int(k)
    return xs[lo] + (xs[min(lo + 1, len(xs) - 1)] - xs[lo]) * (k - lo)


def fmt(x, d=3):
    if x is None or (isinstance(x, float) and math.isnan(x)):
        return "–"
    if abs(x) >= 100:
        return f"{x:.0f}"
    return f"{x:.{d}f}"


def table(head, rows):
    print("| " + " | ".join(head) + " |")
    print("|" + "---|" * len(head))
    for r in rows:
        print("| " + " | ".join(str(c) for c in r) + " |")
    print()


# ------------------------------------------------------------------ load
media = {mid: json.loads(d) for mid, d in DB.execute("SELECT id, data FROM media WHERE in_scope=1")}
trend = defaultdict(dict)
for mid, day, tr, pop, ip in DB.execute("SELECT media_id, day, trending, popularity, in_progress FROM trends"):
    trend[mid][date.fromisoformat(day)] = (tr or 0, pop or 0, ip or 0)
wins = defaultdict(dict)
for mid, tag, a, b, n in DB.execute("SELECT media_id, tag, start, end, n FROM windows_done"):
    wins[mid][tag] = (date.fromisoformat(a), date.fromisoformat(b), n)
first_trend = {mid: date.fromisoformat(d) for mid, d in DB.execute("SELECT media_id, day FROM first_trend WHERE day IS NOT NULL")}


def window_att(mid, a, b):
    """Mean daily (adds, trending, inProgress) over [a, b]. Missing days = no activity (trending 0)."""
    t = trend[mid]
    days = [a + timedelta(days=k) for k in range((b - a).days + 1)]
    rows = [(d, t[d]) for d in days if d in t]
    n = len(days)
    tr = sum(r[1][0] for r in rows) / n
    if len(rows) >= 2:
        adds = max(0.0, (rows[-1][1][1] - rows[0][1][1]) / max(1, (rows[-1][0] - rows[0][0]).days))
    else:
        adds = 0.0
    ip = rows[-1][1][2] if rows else 0
    return adds, tr, ip, len(rows)


def att(x):
    return 2 * x[0] + x[1]


def pop_at(mid, d):
    t = trend[mid]
    best = None
    for k in range(0, 30):
        if d - timedelta(days=k) in t:
            return t[d - timedelta(days=k)][1]
    return best


def sequels(m):
    out = []
    for ed in m["relations"]["edges"]:
        n = ed["node"]
        if ed["relationType"] == "SEQUEL" and n["type"] == "ANIME" and n["format"] != "MUSIC":
            out.append(n)
    return out


def source_node(m):
    for ed in m["relations"]["edges"]:
        n = ed["node"]
        if ed["relationType"] in ("SOURCE", "ADAPTATION") and n["type"] == "MANGA":
            return n
    return None


# ------------------------------------------------------------------ per-show us tail
shows = []
for mid, m in media.items():
    s, e = fdate(m["startDate"]), fdate(m["endDate"])
    if m["status"] != "FINISHED" or not s or not e or mid not in wins:
        continue
    if (e - s).days > 400 or (m.get("episodes") or 0) > 60:
        continue  # long-runners: separate population
    ins = window_att(mid, s, e)
    end = window_att(mid, max(s, e - timedelta(days=27)), e)
    if att(ins) <= 0:
        continue
    r = {}
    comp = {}
    for mo in TAIL:
        w = wins[mid].get(f"t{mo}")
        if not w:
            continue
        x = window_att(mid, w[0], w[1])
        r[mo] = att(x) / att(ins)
        comp[mo] = (x[0] / ins[0] if ins[0] > 0 else float("nan"), x[1] / ins[1] if ins[1] > 0 else float("nan"),
                    att(x) / att(end) if att(end) > 0 else float("nan"), x[2] / ins[2] if ins[2] > 0 else float("nan"))
    seq = sequels(m)
    seq_air = [fdate(n["startDate"]) for n in seq if fdate(n["startDate"]) and n["format"] in SERIES | {"MOVIE"}]
    shows.append(dict(id=mid, m=m, s=s, e=e, ins=ins, end=end, r=r, comp=comp, seq=seq,
                      first_seq_air=min(seq_air) if seq_air else None,
                      pop_fin=pop_at(mid, e + timedelta(days=14)) or m["popularity"]))
print(f"<!-- shows with us tail data: {len(shows)} -->\n")


# platform-activity index: median in-season attention/day of the in-scope shows airing in each quarter (all shows,
# including long ones), so a tail window is compared with what a typical show drew at that time (a share, as in the game)
def qkey(d):
    return d.year * 4 + (d.month - 1) // 3


per_q = defaultdict(list)
for mid, m in media.items():
    s_, e_ = fdate(m["startDate"]), fdate(m["endDate"])
    if not s_ or mid not in wins:
        continue
    e_ = e_ if e_ and m["status"] == "FINISHED" else TODAY
    e_ = min(e_, s_ + timedelta(days=120))
    x = att(window_att(mid, s_, e_))
    if x > 0:
        per_q[qkey(s_ + (e_ - s_) / 2)].append(math.log(x))
qs = sorted(k for k, v in per_q.items() if len(v) >= 15)
INDEX = {k: math.exp(q(per_q[k], .5)) for k in qs}


def index_at(d):
    k = qkey(d)
    near = [x for x in qs if abs(x - k) <= 1]   # 3-quarter smoothing
    if not near:
        near = [qs[-1]] if k > qs[-1] else [qs[0]]
    return math.exp(sum(math.log(INDEX[x]) for x in near) / len(near))


import os
_users = json.load(open(os.path.join(os.path.dirname(os.path.abspath(sys.argv[1])), "anilist_users.json")))
USERS = [(datetime.fromtimestamp(t, JST).date(), c) for t, c in _users]


def users_at(d):
    """AniList registered users (sampled monthly from mid-2019; earlier: extrapolated at the 2019H2 growth rate)."""
    if d <= USERS[0][0]:
        (d0, c0), (d1, c1) = USERS[0], USERS[6]
        g = math.log(c1 / c0) / (d1 - d0).days
        return c0 * math.exp(g * (d - d0).days)
    for (d0, c0), (d1, c1) in zip(USERS, USERS[1:]):
        if d0 <= d <= d1:
            return c0 + (c1 - c0) * (d - d0).days / max(1, (d1 - d0).days)
    return USERS[-1][1]


print("### Platform index: median in-season us attention/day of in-scope shows, by quarter\n")
table(["quarter", "shows", "median att/day", "AniList users (k)"],
      [[f"{k // 4}Q{k % 4 + 1}", len(per_q[k]), fmt(INDEX[k], 0), fmt(users_at(date(k // 4, (k % 4) * 3 + 2, 15)) / 1000, 0)]
       for k in qs])
by_y = defaultdict(list)
for sh in shows:
    by_y[sh["e"].year].append(sh["pop_fin"])
for sh in shows:
    v = by_y[sh["e"].year]
    sh["pct"] = sum(1 for x in v if x < sh["pop_fin"]) / len(v)
for sh in shows:
    i0 = index_at(sh["s"] + (sh["e"] - sh["s"]) / 2)
    sh["rd"] = {mo: v * i0 / index_at(wins[sh["id"]][f"t{mo}"][0]) for mo, v in sh["r"].items()}
    u0 = users_at(sh["s"] + (sh["e"] - sh["s"]) / 2)
    sh["ru"] = {mo: v * u0 / users_at(wins[sh["id"]][f"t{mo}"][0]) for mo, v in sh["r"].items()}
    sh["prequel"] = any(ed["relationType"] == "PREQUEL" for ed in sh["m"]["relations"]["edges"])

print("### us: deflated tail (attention share relative to the in-season share; divides by the platform index)\n")
rows = []
for mo in TAIL:
    xs = [sh["rd"][mo] for sh in shows if mo in sh["rd"]]
    nos = [sh["rd"][mo] for sh in shows if mo in sh["rd"] and not sh["seq"]]
    first = [sh["rd"][mo] for sh in shows if mo in sh["rd"] and not sh["seq"] and not sh["prequel"]]
    top = [sh["rd"][mo] for sh in shows if mo in sh["rd"] and not sh["seq"] and sh["pct"] >= 0.5]
    if len(xs) < 20:
        continue
    rows.append([mo, len(xs), fmt(q(xs, .25)), fmt(q(xs, .5)), fmt(q(xs, .75)), fmt(q(nos, .5)), len(nos),
                 fmt(q(first, .5)), fmt(q(top, .5)), len(top)])
table(["months after finale", "n", "p25", "median", "p75", "median, no sequel", "n", "median, standalone",
       "median, no sequel, top half of its year", "n"], rows)

print("### us: the same tail per registered user (second deflator; users grew ~13× 2019→2026, active users less)\n")
rows = []
for mo in TAIL:
    nos = [sh["ru"][mo] for sh in shows if mo in sh["ru"] and not sh["seq"]]
    if len(nos) >= 20:
        rows.append([mo, len(nos), fmt(q(nos, .25)), fmt(q(nos, .5)), fmt(q(nos, .75))])
table(["months", "n (no sequel)", "p25", "median", "p75"], rows)

print("### us: deflated tail (no sequel) by finale year — lifecycle shape or calendar effect?\n")
yrs = sorted({sh["e"].year for sh in shows})
sel = [1, 3, 6, 12, 18, 24, 36, 48, 60, 84]
rows = []
for y in yrs:
    g = [sh for sh in shows if sh["e"].year == y and not sh["seq"]]
    rows.append([y, len(g)] + [fmt(q([sh["rd"][mo] for sh in g if mo in sh["rd"]], .5)) if sum(1 for sh in g if mo in sh["rd"]) >= 10 else "–" for mo in sel])
table(["finale year", "n"] + [f"{mo}m" for mo in sel], rows)
rows = []
for y in yrs:
    g = [sh for sh in shows if sh["e"].year == y and not sh["seq"]]
    rows.append([y, len(g)] + [fmt(q([sh["ru"][mo] for sh in g if mo in sh["ru"]], .5)) if sum(1 for sh in g if mo in sh["ru"]) >= 10 else "–" for mo in sel])
print("Per registered user:\n")
table(["finale year", "n"] + [f"{mo}m" for mo in sel], rows)

print("### us: implied weekly multiplier of the deflated tail (medians, no sequel) vs the game's ×0.94/week\n")
rows = []
med = {mo: q([sh["rd"][mo] for sh in shows if mo in sh["rd"] and not sh["seq"]], .5) for mo in TAIL}
medu = {mo: q([sh["ru"][mo] for sh in shows if mo in sh["ru"] and not sh["seq"]], .5) for mo in TAIL}
for a, b in [(1, 3), (3, 6), (6, 12), (12, 24), (12, 36), (36, 60), (36, 96), (60, 96)]:
    w = (b - a) * 30.44 / 7
    f = lambda m: (m[b] / m[a]) ** (1 / w) if m.get(a) and m.get(b) and m[a] > 0 and m[b] > 0 else float("nan")
    rows.append([f"{a}→{b} months", fmt(f(med), 4), fmt(f(medu), 4)])
table(["segment", "× per week (platform index)", "× per week (per user)"], rows)


print("### us: attention after the finale relative to the in-season mean (2·adds + trending)\n")
rows = []
for mo in TAIL:
    xs = [sh["r"][mo] for sh in shows if mo in sh["r"]]
    nos = [sh["r"][mo] for sh in shows if mo in sh["r"] and not sh["seq"]]
    if len(xs) < 20:
        continue
    adds = [sh["comp"][mo][0] for sh in shows if mo in sh["comp"]]
    trs = [sh["comp"][mo][1] for sh in shows if mo in sh["comp"]]
    rows.append([mo, len(xs), fmt(q(xs, .25)), fmt(q(xs, .5)), fmt(q(xs, .75)), fmt(q(nos, .5)), len(nos),
                 fmt(q(adds, .5)), fmt(q(trs, .5))])
table(["months after finale", "n", "p25", "median", "p75", "median, no sequel ever", "n no-seq", "adds only",
       "trending only"], rows)

# the game's fitted post-finale curve, relative to the level at the end (last 4 weeks)
print("### us: measured vs the game's post-finale curve (relative to the last 4 in-season weeks)\n")
prof, tail = [0.446671, 0.299977, 0.225845, 0.179913, 0.162655], 0.939826


def model(mo):
    w = int(mo * 30.44 / 7) - 1
    return prof[w] if w < len(prof) else prof[-1] * tail ** (w - len(prof) + 1)


rows = []
for mo in TAIL:
    xs = [sh["comp"][mo][2] for sh in shows if mo in sh["comp"] and not sh["seq"]]
    if len(xs) < 20 or mo > 12:
        continue
    rows.append([mo, len(xs), fmt(q(xs, .5)), fmt(model(mo), 4), fmt(q(xs, .5) / model(mo), 1)])
table(["months", "n (no sequel)", "measured median", "game curve", "measured / curve"], rows)

# decay rate in the tail: per-show log-slope between 12 and 36+ months
print("### us: tail decay per year, no-sequel shows (log-slope of deflated attention, months 12 → last window ≥ 36)\n")
slopes = defaultdict(list)
for sh in shows:
    if sh["seq"]:
        continue
    pts = [(mo, sh["rd"][mo]) for mo in sh["rd"] if mo >= 12 and sh["rd"][mo] > 0]
    if len(pts) < 4 or max(p[0] for p in pts) < 36:
        continue
    x = np.array([p[0] / 12 for p in pts])
    y = np.log([p[1] for p in pts])
    b = np.polyfit(x, y, 1)[0]
    early = [p for p in pts if p[0] <= 24]
    late = [p for p in pts if p[0] >= 36]
    slopes["all"].append(b)
    if len(early) >= 2 and len(late) >= 2:
        slopes["12-24"].append(np.polyfit([p[0] / 12 for p in early], np.log([p[1] for p in early]), 1)[0])
        slopes["36+"].append(np.polyfit([p[0] / 12 for p in late], np.log([p[1] for p in late]), 1)[0])
rows = []
for k, v in slopes.items():
    med = q(v, .5)
    rows.append([k, len(v), fmt(med), fmt(math.exp(med), 3), fmt(math.log(2) / -med if med < 0 else float("inf"), 1)])
table(["segment", "n", "median log-slope /yr", "× per year", "half-life (years)"], rows)

# floor by popularity decile at the finale
print("### us: deflated tail level (mean of months 24–36) by popularity percentile within the finale year, no-sequel shows\n")
cand = [sh for sh in shows if not sh["seq"] and all(mo in sh["rd"] for mo in (24, 30, 36))]
for sh in cand:
    sh["floor"] = (sh["rd"][24] + sh["rd"][30] + sh["rd"][36]) / 3
cuts = [q([sh["pct"] for sh in cand], k / 5) for k in range(6)]
rows = []
for k in range(5):
    g = [sh for sh in cand if cuts[k] <= sh["pct"] <= cuts[k + 1]]
    rows.append([f"{cuts[k]:.2f}–{cuts[k + 1]:.2f}", len(g), fmt(q([x["floor"] for x in g], .25)),
                 fmt(q([x["floor"] for x in g], .5)), fmt(q([x["floor"] for x in g], .75)),
                 fmt(q([att(x["ins"]) for x in g], .5)), fmt(q([att(x["ins"]) * x["floor"] for x in g], .5), 1)])
table(["popularity at finale (quintile)", "n", "p25", "median", "p75", "in-season att/day (median)",
       "tail att/day (median)"], rows)

# regression of log floor
print("### us: what predicts the tail level (OLS on log deflated ratio, months 24–36; all shows incl. sequels)\n")
cand2 = [sh for sh in shows if all(mo in sh["rd"] for mo in (24, 30, 36))]
X, Y, names = [], [], ["const", "pop percentile in finale year", "score/10", "light novel", "original", "game/VN/other",
                       "ONA", "2+ cours", "sequel aired by month 36", "Comedy", "Romance", "Action",
                       "Slice of Life", "Fantasy", "year-2020", "is itself a sequel"]
for sh in cand2:
    m = sh["m"]
    fl = (sh["rd"][24] + sh["rd"][30] + sh["rd"][36]) / 3
    if fl <= 0:
        continue
    src = m.get("source") or "OTHER"
    g = set(m.get("genres") or [])
    sa = sh["first_seq_air"]
    X.append([1, sh["pct"], (m.get("averageScore") or 65) / 10, src == "LIGHT_NOVEL", src == "ORIGINAL",
              src in ("VIDEO_GAME", "VISUAL_NOVEL", "OTHER", "NOVEL", "WEB_NOVEL"), m["format"] == "ONA",
              (m.get("episodes") or 12) > 15, bool(sa and sa <= sh["e"] + timedelta(days=36 * 30)),
              "Comedy" in g, "Romance" in g, "Action" in g, "Slice of Life" in g, "Fantasy" in g, sh["s"].year - 2020, sh["prequel"]])
    Y.append(math.log(fl))
X, Y = np.array(X, dtype=float), np.array(Y)
beta, *_ = np.linalg.lstsq(X, Y, rcond=None)
res = Y - X @ beta
s2 = res @ res / (len(Y) - len(beta))
se = np.sqrt(np.diag(s2 * np.linalg.inv(X.T @ X)))
r2 = 1 - res @ res / ((Y - Y.mean()) @ (Y - Y.mean()))
table(["predictor", "coef", "se", "× effect"],
      [[n, fmt(b), fmt(s), fmt(math.exp(b), 2)] for n, b, s in zip(names, beta, se)])
print(f"n = {len(Y)}, R² = {r2:.2f}, residual sd (log) = {math.sqrt(s2):.2f}\n")

# ------------------------------------------------------------------ sequels
print("### Sequel rates after the finale (TV/ONA shows; any SEQUEL relation)\n")
seq_rows = []
for mid, m in media.items():
    e = fdate(m["endDate"])
    if m["status"] != "FINISHED" or not e:
        continue
    sq = sequels(m)
    ser = [n for n in sq if n["format"] in SERIES]
    film = [n for n in sq if n["format"] == "MOVIE"]
    anyc = [n for n in sq if n["format"] in SERIES | {"MOVIE"}]

    def t_air(ns):
        ds = [fdate(n["startDate"]) for n in ns if fdate(n["startDate"]) and fdate(n["startDate"]) <= TODAY]
        return (min(ds) - e).days if ds else None

    def t_ann(ns):
        ds = [first_trend[n["id"]] for n in ns if n["id"] in first_trend]
        ds += [fdate(n["startDate"]) for n in ns if fdate(n["startDate"])]
        ds = [d for d in ds if d and d <= TODAY]
        return (min(ds) - e).days if ds else None
    srcn = source_node(m)
    seq_rows.append(dict(id=mid, m=m, e=e, pop=pop_at(mid, e + timedelta(days=14)) or m["popularity"],
                         score=m.get("averageScore"), src=m.get("source") or "OTHER",
                         src_ongoing=((srcn["status"] in ("RELEASING", "HIATUS")) or
                                      (fdate(srcn["endDate"]) is not None and fdate(srcn["endDate"]) > e)) if srcn else None,
                         air_series=t_air(ser), air_any=t_air(anyc), ann_any=t_ann(anyc), ann_series=t_ann(ser),
                         film=t_air(film), studio=((m["studios"]["nodes"] or [{}])[0]).get("name"),
                         prequel=any(ed["relationType"] == "PREQUEL" for ed in m["relations"]["edges"])))
by_y = defaultdict(list)
for r in seq_rows:
    by_y[r["e"].year].append(r["pop"])
for r in seq_rows:
    r["pct"] = sum(1 for x in by_y[r["e"].year] if x < r["pop"]) / len(by_y[r["e"].year])


def rate(group, key, yrs):
    el = [r for r in group if r["e"] + timedelta(days=int(365.25 * yrs)) <= TODAY]
    if len(el) < 8:
        return "–", len(el)
    hit = sum(1 for r in el if r[key] is not None and r[key] <= 365.25 * yrs)
    return f"{hit / len(el):.0%}", len(el)


def rate_rows(groups, key):
    out = []
    for name, g in groups:
        cells = [name, len(g)]
        for y in (1, 2, 3):
            v, n = rate(g, key, y)
            cells.append(f"{v} ({n})")
        out.append(cells)
    return out


allr = seq_rows
print("Columns: share of shows with the event within 1/2/3 years of the finale (cohort size: shows whose finale is at "
      "least that long ago).\n")
table(["event", "n", "≤1y", "≤2y", "≤3y"],
      [["announced (any TV/ONA/film sequel)"] + rate_rows([("", allr)], "ann_any")[0][1:],
       ["aired: any TV/ONA/film sequel"] + rate_rows([("", allr)], "air_any")[0][1:],
       ["aired: TV/ONA sequel"] + rate_rows([("", allr)], "air_series")[0][1:],
       ["aired: film sequel"] + rate_rows([("", allr)], "film")[0][1:]])

print("#### First seasons vs shows that are themselves sequels (announcement)\n")
table(["", "n", "≤1y", "≤2y", "≤3y"], rate_rows([("first season (no PREQUEL)", [r for r in allr if not r["prequel"]]),
                                                ("itself a sequel", [r for r in allr if r["prequel"]])], "ann_any"))
print("#### By popularity decile within the finale year (announcement of any TV/ONA/film sequel; first seasons only)\n")
fs = [r for r in allr if not r["prequel"]]
groups = [(f"D{k + 1}", [r for r in fs if k / 10 <= r["pct"] < (k + 1) / 10 or (k == 9 and r["pct"] >= 0.9)]) for k in range(10)]
table(["popularity", "n", "≤1y", "≤2y", "≤3y"], rate_rows(groups, "ann_any"))

print("#### By score (first seasons)\n")
bins = [(0, 64), (65, 69), (70, 74), (75, 79), (80, 100)]
groups = [(f"{a}–{b}", [r for r in fs if r["score"] and a <= r["score"] <= b]) for a, b in bins]
table(["AniList score", "n", "≤1y", "≤2y", "≤3y"], rate_rows(groups, "ann_any"))

print("#### By source (first seasons)\n")
srcs = defaultdict(list)
for r in fs:
    srcs[r["src"] if r["src"] in ("MANGA", "LIGHT_NOVEL", "ORIGINAL", "VIDEO_GAME") else "OTHER (web novel, novel, VN, …)"].append(r)
table(["source", "n", "≤1y", "≤2y", "≤3y"], rate_rows(sorted(srcs.items()), "ann_any"))

print("#### Manga source still running at the anime finale vs already finished (first seasons)\n")
groups = [("manga source releasing/hiatus", [r for r in fs if r["src_ongoing"] is True]),
          ("manga source finished", [r for r in fs if r["src_ongoing"] is False]),
          ("no manga source relation", [r for r in fs if r["src_ongoing"] is None])]
table(["source state", "n", "≤1y", "≤2y", "≤3y"], rate_rows(groups, "ann_any"))

print("#### Studios with ≥ 15 shows (announcement within 3 years)\n")
st = defaultdict(list)
for r in allr:
    st[r["studio"]].append(r)
rows = []
for k, g in st.items():
    v, n = rate(g, "ann_any", 3)
    if n >= 15:
        rows.append([k, n, v])
rows.sort(key=lambda x: -float(x[2].rstrip("%")) if x[2] != "–" else 0)
table(["studio", "n (3y cohort)", "announced ≤3y"], rows)

# logistic regression: announced within 3 years
el = [r for r in allr if r["e"] + timedelta(days=int(365.25 * 3)) <= TODAY and r["score"]]
Xl = np.array([[1, r["pct"] - 0.5, (r["score"] - 70) / 10, r["src"] == "LIGHT_NOVEL", r["src"] == "ORIGINAL",
                r["src_ongoing"] is True, r["src_ongoing"] is False, r["prequel"]] for r in el], dtype=float)
yl = np.array([1.0 if r["ann_any"] is not None and r["ann_any"] <= 365.25 * 3 else 0.0 for r in el])
b = np.zeros(Xl.shape[1])
for _ in range(50):
    p = 1 / (1 + np.exp(-Xl @ b))
    H = Xl.T @ (Xl * (p * (1 - p))[:, None]) + 1e-6 * np.eye(len(b))
    b += np.linalg.solve(H, Xl.T @ (yl - p))
se = np.sqrt(np.diag(np.linalg.inv(H)))
print("#### Logistic model: sequel (TV/ONA/film) announced within 3 years of the finale\n")
table(["predictor", "coef", "se", "odds ×"],
      [[n, fmt(c), fmt(s), fmt(math.exp(c), 2)] for n, c, s in
       zip(["const (median popularity, score 70, no manga relation, first season)", "popularity percentile (0→1)", "score (per 10)",
            "light novel", "original", "manga source running at finale", "manga source finished by finale", "itself a sequel"], b, se)])
print(f"n = {len(yl)}, base rate = {yl.mean():.0%}\n")

print("### Announcement timing and lag (TV/ONA/film sequels; first AniList trend day as the announcement proxy)\n")
lag, ann_rel = [], []
for mid, m in media.items():
    e = fdate(m["endDate"])
    if not e:
        continue
    for n in sequels(m):
        if n["format"] not in SERIES | {"MOVIE"} or n["id"] not in first_trend:
            continue
        A = first_trend[n["id"]]
        if A < date(2018, 7, 1):
            continue
        ann_rel.append((A - e).days)
        sd = fdate(n["startDate"])
        if sd and sd <= TODAY and n["status"] in ("FINISHED", "RELEASING"):
            lag.append(((sd - A).days, n["format"]))
rows = []
for name, xs in [("announcement − original finale (days)", ann_rel),
                 ("air − announcement, TV/ONA (days)", [x for x, f in lag if f in SERIES]),
                 ("air − announcement, film (days)", [x for x, f in lag if f == "MOVIE"])]:
    rows.append([name, len(xs), fmt(q(xs, .1)), fmt(q(xs, .25)), fmt(q(xs, .5)), fmt(q(xs, .75)), fmt(q(xs, .9))])
table(["", "n", "p10", "p25", "median", "p75", "p90"], rows)
print(f"Share of announcements on or before the finale day + 7: "
      f"{sum(1 for x in ann_rel if x <= 7) / max(1, len(ann_rel)):.0%}\n")

print("### Announcement effect on the original show's us attention (daily, around the sequel's first trend day A)\n")
eff = defaultdict(list)
for mid, ws in wins.items():
    for tag in ws:
        if not tag.startswith("a") or not tag.endswith("_1"):
            continue
        A = ws[tag][0]
        base = window_att(mid, A - timedelta(days=28), A - timedelta(days=1))
        drift = window_att(mid, A - timedelta(days=45), A - timedelta(days=29))
        if att(base) <= 0:
            continue
        for name, (a, b) in {"pre (A−45..A−29)": (A - timedelta(days=45), A - timedelta(days=29)),
                             "A..A+6": (A, A + timedelta(days=6)), "A+7..A+20": (A + timedelta(days=7), A + timedelta(days=20)),
                             "A+21..A+45": (A + timedelta(days=21), A + timedelta(days=45))}.items():
            x = window_att(mid, a, b)
            eff[name].append((att(x) / att(base), x[0] / base[0] if base[0] > 0 else float("nan"),
                              x[1] / base[1] if base[1] > 0 else float("nan")))
rows = []
for name, xs in eff.items():
    rows.append([name, len(xs), fmt(q([x[0] for x in xs], .25)), fmt(q([x[0] for x in xs], .5)),
                 fmt(q([x[0] for x in xs], .75)), f"{sum(1 for x in xs if x[0] > 1.5) / len(xs):.0%}",
                 fmt(q([x[1] for x in xs], .5)), fmt(q([x[2] for x in xs], .5))])
table(["window vs A−28..A−1", "n", "p25", "median", "p75", "share > 1.5×", "adds median", "trending median"], rows)

# ------------------------------------------------------------------ Bangumi
print("### cn: Bangumi episode comments vs subject (collection) comments after the finale\n")
subj = {}
for bid, data, eps in DB.execute("SELECT bgm_id, data, episodes FROM bgm_subject"):
    eps = [e for e in json.loads(eps) if e.get("airdate")]
    if not eps:
        continue
    ad = sorted(date.fromisoformat(e["airdate"]) for e in eps if len(e["airdate"]) == 10)
    if not ad:
        continue
    subj[bid] = dict(s=ad[0], e=ad[-1], n=len(ad), data=json.loads(data))
epc = defaultdict(list)
for eid, bid, sort, airdate, n_eps, times in DB.execute("SELECT * FROM bgm_ep_comments"):
    if airdate and len(airdate) == 10:
        epc[bid].append((date.fromisoformat(airdate), [datetime.fromtimestamp(t, JST).date() for t in json.loads(times)]))
pages = defaultdict(list)
for bid, off, total, times in DB.execute("SELECT * FROM bgm_subject_comments"):
    pages[bid].append((off, total, json.loads(times)))


def n_since(bid, t):
    """Number of subject comments with updatedAt >= t (interpolated between sampled pages)."""
    pts = [(datetime.now(JST).timestamp() + 1, 0)]
    total = 0
    for off, tot, ts in pages[bid]:
        total = max(total, tot)
        for i, x in enumerate(ts):
            pts.append((x, off + i + 1))
    pts.sort(key=lambda p: -p[0])
    if not pts or t < pts[-1][0]:
        return total
    for (t0, r0), (t1, r1) in zip(pts, pts[1:]):
        if t1 <= t <= t0:
            return r0 + (r1 - r0) * (t0 - t) / (t0 - t1) if t0 > t1 else r1
    return total


def ts(d):
    return datetime(d.year, d.month, d.day, tzinfo=JST).timestamp()


ep_ratio, col_ratio = defaultdict(list), defaultdict(list)
ep_num, ep_den = defaultdict(float), defaultdict(float)
for bid, sj in subj.items():
    S, E = sj["s"], sj["e"]
    if (E - S).days > 400:
        continue
    ec = epc.get(bid)
    weeks_in = max(1, ((E - S).days + 7) / 7)
    if ec:
        lvl = sum(sum(1 for t in times if 0 <= (t - ad).days < 28) for ad, times in ec) / len(ec)
        for mo in TAIL:
            a = E + timedelta(days=round(mo * 30.44))
            b = a + timedelta(days=30)
            if b > TODAY or lvl <= 0:
                continue
            c = sum(sum(1 for t in times if a <= t < b) for ad, times in ec) * sj["n"] / len(ec)
            ep_ratio[mo].append(c / (30 / 7) / lvl)
            ep_num[mo] += c / (30 / 7)
            ep_den[mo] += lvl
    if pages.get(bid):
        tot = max(p[1] for p in pages[bid])
        if tot < 50:
            continue
        ins = (n_since(bid, ts(S)) - n_since(bid, ts(E + timedelta(days=14)))) / (weeks_in + 2)
        if ins <= 0:
            continue
        for mo in TAIL:
            a = E + timedelta(days=round(mo * 30.44))
            b = a + timedelta(days=30)
            if b > TODAY:
                continue
            col_ratio[mo].append((n_since(bid, ts(a)) - n_since(bid, ts(b))) / (30 / 7) / ins)
# us for comparison (no-sequel and all)
rows = []
for mo in TAIL:
    us = [sh["r"][mo] for sh in shows if mo in sh["r"]]
    usa = [sh["comp"][mo][0] for sh in shows if mo in sh["comp"]]
    if len(ep_ratio[mo]) < 10 and len(col_ratio[mo]) < 10:
        continue
    rows.append([mo, len(ep_ratio[mo]), fmt(q(ep_ratio[mo], .5), 4),
                 fmt(ep_num[mo] / ep_den[mo] if ep_den[mo] else float("nan"), 4),
                 f"{sum(1 for x in ep_ratio[mo] if x == 0) / max(1, len(ep_ratio[mo])):.0%}",
                 len(col_ratio[mo]), fmt(q(col_ratio[mo], .5)), fmt(q(col_ratio[mo], .75)),
                 fmt(q(us, .5)), fmt(q(usa, .5))])
table(["months", "n ep", "ep comments: median ratio", "pooled ratio", "share zero", "n col",
       "collection comments: median", "p75", "us att median", "us adds median"], rows)
print(f"<!-- Bangumi subjects: {len(subj)}, with episode comments {len(epc)}, with subject comments {len(pages)} -->")


# ------------------------------------------------------------------ design checks (docs/v2-series.md)
print("\n## Design checks\n")
print("### us tail level (deflated, months 24–36, no sequel) with windows overlapping 2020-04..2021-06 dropped\n")
LOCK_A, LOCK_B = date(2020, 4, 1), date(2021, 6, 30)
rows = []
for name, keep in [("all windows", lambda a: True), ("lockdown windows dropped", lambda a: not (LOCK_A <= a <= LOCK_B))]:
    for mo in (12, 18, 24, 36, 48, 60, 84):
        xs = [sh["rd"][mo] for sh in shows if not sh["seq"] and mo in sh["rd"] and keep(wins[sh["id"]][f"t{mo}"][0])]
        if len(xs) >= 10:
            rows.append([name, mo, len(xs), fmt(q(xs, .25)), fmt(q(xs, .5)), fmt(q(xs, .75))])
table(["", "months", "n", "p25", "median", "p75"], rows)

print("### Sequel season vs its prequel: in-season us attention ratio (platform-index deflated)\n")
ins_of = {}
for mid, m in media.items():
    s_, e_ = fdate(m["startDate"]), fdate(m["endDate"])
    if not s_ or not e_ or mid not in wins or m.get("format") not in SERIES:
        continue
    if (e_ - s_).days > 400:
        continue
    x = att(window_att(mid, s_, e_))
    if x > 0:
        ins_of[mid] = (x / index_at(s_ + (e_ - s_) / 2), s_, e_)
pairs = []
for mid, m in media.items():
    if mid not in ins_of:
        continue
    for ed in m["relations"]["edges"]:
        n = ed["node"]
        if ed["relationType"] == "PREQUEL" and n["id"] in ins_of:
            gap = (ins_of[mid][1] - ins_of[n["id"]][2]).days / 365.25
            pairs.append((ins_of[mid][0] / ins_of[n["id"]][0], gap))
rows = []
for name, lo, hi in [("all", -1, 99), ("gap < 1y", -1, 1), ("1–2y", 1, 2), ("2–4y", 2, 4), ("4y+", 4, 99)]:
    xs = [r for r, g in pairs if lo <= g < hi]
    if len(xs) >= 8:
        rows.append([name, len(xs), fmt(q(xs, .25)), fmt(q(xs, .5)), fmt(q(xs, .75)),
                     fmt(math.exp(sum(math.log(x) for x in xs) / len(xs)))])
table(["gap (prequel finale → sequel premiere)", "n", "p25", "median", "p75", "geo mean"], rows)

print("### Sequel announced within 5 years, by manga source state (first seasons; censoring check)\n")


def rate5(g, yrs=5):
    el = [r for r in g if r["e"] + timedelta(days=int(365.25 * yrs)) <= TODAY]
    if len(el) < 8:
        return "–", len(el)
    return f"{sum(1 for r in el if r['ann_any'] is not None and r['ann_any'] <= 365.25 * yrs) / len(el):.0%}", len(el)


rows = []
for name, g in [("manga running at finale", [r for r in fs if r["src_ongoing"] is True]),
                ("manga finished by finale", [r for r in fs if r["src_ongoing"] is False]),
                ("no manga relation", [r for r in fs if r["src_ongoing"] is None])]:
    cells = [name]
    for yrs in (3, 4, 5, 6):
        v, n = rate5(g, yrs)
        cells.append(f"{v} ({n})")
    rows.append(cells)
table(["source state", "≤3y", "≤4y", "≤5y", "≤6y"], rows)

print("### Hazard of a sequel announcement by year since the finale (first seasons, all)\n")
rows = []
for y in range(0, 7):
    at_risk = [r for r in fs if r["e"] + timedelta(days=int(365.25 * (y + 1))) <= TODAY
               and (r["ann_any"] is None or r["ann_any"] > 365.25 * y)]
    if len(at_risk) < 20:
        continue
    hit = sum(1 for r in at_risk if r["ann_any"] is not None and r["ann_any"] <= 365.25 * (y + 1))
    rows.append([f"{y}–{y + 1}", len(at_risk), f"{hit / len(at_risk):.1%}"])
table(["years after finale", "at risk", "announced in the year"], rows)
