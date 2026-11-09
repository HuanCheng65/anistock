"""The futures forecast's link (backend/src/futures/rules.rs `index_link`): per market and four-week block since the
premiere, the median per show-day of the index's figure (heat_history in data/derived/heat_backfill.db) over the
inputs' att.  .venv/bin/python index_link.py INPUTS_DIR [FROM_DAY]"""
import sys, json, glob, os, sqlite3, statistics as S
from datetime import date
heat = {}
for w, d, us, cn, jp in sqlite3.connect('file:/home/starrydream/anisotck/data/derived/heat_backfill.db?mode=ro', uri=True).execute(
        "SELECT work_id, day, us, cn, jp FROM heat_history"):
    heat[(w, d)] = {'us': us, 'cn': cn, 'jp': jp}
lo = sys.argv[2] if len(sys.argv) > 2 else '2026-07-01'
r = {m: {b: [] for b in range(4)} for m in ('us', 'cn', 'jp')}
for f in sorted(glob.glob(sys.argv[1] + '/*.json')):
    x = json.load(open(f))
    if x['day'] < lo:
        continue
    for s in x['shows']:
        if s['long_runner'] or not s['aired']:
            continue
        h = heat.get((s['id'], x['day']))
        if not h:
            continue
        b = min(3, max(0, (date.fromisoformat(x['day']) - date.fromisoformat(s['first_air'])).days // 7 // 4))
        for m in r:
            a = s['att'][m]
            if h[m] and a and a > 0 and (m == 'us' or (s.get('observed') or {}).get(m)):
                r[m][b].append(h[m] / a)
print({m: [round(S.median(v), 3) if len(v) > 50 else None for b, v in sorted(r[m].items())] for m in r})
print({m: [len(v) for b, v in sorted(r[m].items())] for m in r})
