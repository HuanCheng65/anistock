"""Pre-air AniList list growth: does a show's growth surprise over the last week predict its growth until the freeze
(the day before the premiere)?  .venv/bin/python pregrowth.py INPUTS_DIR FILTER"""
import sys, json, glob, math, statistics as S
from datetime import date, timedelta
G = json.load(open(sys.argv[2]))['preair_growth']
g = lambda k: G[min(k, len(G) - 1)][0] if k > 0 else 0.0
pr = {}
for f in sorted(glob.glob(sys.argv[1] + '/*.json')):
    x = json.load(open(f))
    for s in x['shows']:
        p = (s.get('prior') or {}).get('us')
        if p and p > 0:
            pr.setdefault(s['id'], {})[x['day']] = (p, s['first_air'])
for k in (7, 14, 21):
    xs = []
    for sid, v in pr.items():
        fa = date.fromisoformat(next(iter(v.values()))[1]); frz = (fa - timedelta(days=1))
        d0, d1 = (frz - timedelta(days=k + 7)).isoformat(), (frz - timedelta(days=k)).isoformat()
        if d0 in v and d1 in v and frz.isoformat() in v:
            past = math.log(v[d1][0] / v[d0][0]) - (g(k + 7) - g(k))
            fut = math.log(v[frz.isoformat()][0] / v[d1][0]) - g(k)
            xs.append((past, fut))
    if len(xs) > 10:
        a = [p for p, _ in xs]; b = [q for _, q in xs]; ma, mb = S.mean(a), S.mean(b)
        sl = sum((p - ma) * (q - mb) for p, q in xs) / sum((p - ma) ** 2 for p in a)
        print(f'k={k:2}: n={len(xs)}  slope of the surprise until the freeze on last week\'s surprise {sl:+.2f}  corr {S.correlation(a, b):+.2f}  mean future surprise {100*mb:+.1f}%')
