"""Pre-air value changes vs the prior's own change (log p_us + expected growth until the freeze), by days to the
premiere: is the value's pre-air rise explained by the prior?  .venv/bin/python pre.py VO_DIR INPUTS_DIR FILTER"""
import sys, json, glob, math, statistics as S
sys.path.insert(0, '/home/starrydream/anisotck/sim'); sys.path.insert(0, '/home/starrydream/anisotck')
from pathlib import Path
from datetime import date
import quickcheck as Q
w = Q.World(Path(sys.argv[1]), Path(sys.argv[2]))
G = json.load(open(sys.argv[3]))['preair_growth']
gk = lambda k: G[min(k, len(G) - 1)][0] if k > 0 else 0.0
pr = {}
for f in sorted(glob.glob(sys.argv[2] + '/*.json')):
    x = json.load(open(f))
    for s in x['shows']:
        pr.setdefault(s['id'], {})[x['day']] = ((s.get('prior') or {}).get('us'), s['att']['us'])
rows = []
for c in w.ids:
    fa = w.sched.get(c, {}).get('first_air')
    if not fa:
        continue
    for i in range(1, len(w.days)):
        d = w.days[i]
        if d >= fa:
            break
        v0, v1 = w.value[c][i - 1], w.value[c][i]
        a, b = pr.get(c, {}).get(w.days[i - 1]), pr.get(c, {}).get(d)
        if not (v0 and v1 and a and b and a[0] and b[0]):
            continue
        k1 = (date.fromisoformat(fa) - date.fromisoformat(d)).days - 1
        rows.append((math.log((v1 + w.div[c][i]) / v0), math.log(b[0]) + gk(k1) - math.log(a[0]) - gk(k1 + 1), k1))
print('pre-air show-days', len(rows))
for lo, hi in ((0, 6), (7, 13), (14, 20), (21, 60)):
    r = [q for q in rows if lo <= q[2] <= hi]
    if r:
        x = [q[1] for q in r]; y = [q[0] for q in r]
        print(f'  {lo:2}-{hi:2} days to the freeze: value {100*S.mean(y):+.3f}%/day   prior mean {100*S.mean(x):+.3f}%/day   n={len(r)}')
