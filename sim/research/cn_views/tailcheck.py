"""Realised post-finale attention vs the filter's expected tail, per market, on the replay's finished shows:
realised weekly mean / (mean of weeks -3..-2 before the finale week × finale bump × the tail factor at t).
.venv/bin/python tailcheck.py INPUTS_DIR FILTER"""
import sys, json, glob, math, statistics as S
from datetime import date, timedelta
sys.path.insert(0, '/home/starrydream/anisotck')
from engine import valuation as V
fp = json.load(open(sys.argv[2]))
att = {}; meta = {}
for f in sorted(glob.glob(sys.argv[1] + '/*.json')):
    x = json.load(open(f))
    for s in x['shows']:
        att.setdefault(s['id'], {})[x['day']] = (s['att'], s.get('observed') or {})
        meta[s['id']] = s
def factor(m, t):
    tl = fp['tail'][m]
    if tl.get('trans') and t < 7 * len(tl['trans']):
        return tl['trans'][int(t // 7)]
    return V.tail_formula(tl, t, 1.0)
res = {m: {} for m in ('us', 'cn', 'jp')}
for sid, s in meta.items():
    if s['long_runner'] or not s.get('episodes') or s['aired'] < s['episodes'] or not s.get('air_dates'):
        continue
    fin = date.fromisoformat(max(s['air_dates']))
    a = att[sid]
    for m in res:
        ref = [a.get((fin - timedelta(days=k)).isoformat()) for k in range(8, 22)]
        ref = [r[0][m] for r in ref if r and r[0][m] > 0 and (m == 'us' or r[1].get(m))]
        if len(ref) < 7:
            continue
        L = S.mean(ref) * fp['finale_bump'][m]
        for wk in range(0, 10):
            days = [a.get((fin + timedelta(days=7 + 7 * wk + k)).isoformat()) for k in range(7)]
            v = [d[0][m] for d in days if d and d[0][m] > 0 and (m == 'us' or d[1].get(m))]
            if len(v) >= 5:
                res[m].setdefault(wk, []).append(math.log(S.mean(v) / (L * factor(m, 7 * wk + 3.5))))
for m, r in res.items():
    print(m, ' '.join(f'w{k}:{math.exp(S.mean(v)):.2f}({len(v)})' for k, v in sorted(r.items()) if len(v) >= 10))
