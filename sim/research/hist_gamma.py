"""History (2018-2025 AniList): does the weekly us change in week w depend on the level? Slope of
log(att[w+1]/att[w]) on log att[w-1] (lagged: no shared noise), demeaned within each season."""
import json, math, sqlite3, statistics as st
from collections import defaultdict
from datetime import date, timedelta
db = sqlite3.connect('file:data/history.db?mode=ro', uri=True)
trend = defaultdict(dict)
for mid, day, tr, pop in db.execute("SELECT media_id, day, trending, popularity FROM trends"):
    trend[mid][date.fromisoformat(day)] = (tr or 0, pop or 0)
def fd(d):
    return date(d["year"], d["month"], d["day"]) if d and d.get("year") and d.get("month") and d.get("day") else None
def week_att(mid, a):
    t = trend[mid]
    rows = [(a + timedelta(days=k), t.get(a + timedelta(days=k))) for k in range(8)]
    rows = [(d, r) for d, r in rows if r is not None]
    if len(rows) < 6: return None
    wk = [r for d, r in rows if d < a + timedelta(days=7)]
    tr = sum(r[0] for r in wk) / max(1, len(wk))
    adds = max(0.0, (rows[-1][1][1] - rows[0][1][1]) / max(1, (rows[-1][0] - rows[0][0]).days))
    x = 2 * adds + tr
    return x if x > 0 else None
shows = []
for mid, raw in db.execute("SELECT id, data FROM media WHERE in_scope=1"):
    m = json.loads(raw)
    s_, e_ = fd(m.get("startDate")), fd(m.get("endDate"))
    if not s_ or not e_ or m.get("status") != "FINISHED" or m.get("format") not in ("TV", "ONA"): continue
    if m.get("countryOfOrigin") != "JP" or not m.get("episodes") or not (10 <= m["episodes"] <= 28) or not (2018 <= s_.year <= 2025): continue
    nw = (e_ - s_).days // 7
    if nw < 9: continue
    att = [week_att(mid, s_ + timedelta(days=7 * w)) for w in range(nw + 1)]
    shows.append(((s_.year, (s_.month - 1) // 3), nw, att))
print(len(shows), 'shows')
for w in range(1, 9):
    by = defaultdict(list)
    for k, nw, att in shows:
        if w + 1 > nw - 4: continue      # not the run-in to the finale
        a0, a1, a2 = att[w - 1], att[w], att[w + 1]
        if a0 and a1 and a2:
            by[k].append((math.log(a0), math.log(a2 / a1)))
    xs, ys, per = [], [], []
    for k, v in by.items():
        if len(v) < 8: continue
        mx, my = st.fmean(a for a, _ in v), st.fmean(b for _, b in v)
        xs += [a - mx for a, _ in v]; ys += [b - my for _, b in v]
        sx = sum((a - mx) ** 2 for a, _ in v)
        if sx > 0: per.append(sum((a - mx) * (b - my) for a, b in v) / sx)
    b = sum(x * y for x, y in zip(xs, ys)) / sum(x * x for x in xs)
    se = st.pstdev(per) / math.sqrt(len(per))
    print(f'week {w}->{w+1}: {len(xs)} shows, {len(per)} seasons; slope {b:+.4f}/week (per-season mean {st.fmean(per):+.4f}, se {se:.4f}) = {b/7:+.5f}/day')
