"""Mean daily total return by days since the premiere (all stocks): where the early-season fall happens.
.venv/bin/python early.py VO_DIR INPUTS_DIR"""
import sys, statistics as S
sys.path.insert(0, '/home/starrydream/anisotck/sim'); sys.path.insert(0, '/home/starrydream/anisotck')
from pathlib import Path
from datetime import date
import quickcheck as Q
w = Q.World(Path(sys.argv[1]), Path(sys.argv[2]))
by = {}
for c in w.ids:
    fa = w.sched.get(c, {}).get('first_air')
    if not fa or w.sched[c].get('long_runner'):
        continue
    for i in range(1, len(w.days)):
        r = w.total_return(c, i - 1, i)
        if r is None:
            continue
        k = (date.fromisoformat(w.days[i]) - date.fromisoformat(fa)).days
        if -21 <= k <= 35:
            by.setdefault(k, []).append(r)
row = lambda ks: ' '.join(f'{k:+d}:{100*S.mean(by[k]):+.1f}' for k in ks if k in by and len(by[k]) > 10)
print('daily total return %, by day from the premiere')
print(' ', row(range(-21, 0, 3)))
print(' ', row(range(0, 8)))
print(' ', row(range(8, 36, 2)))
cum = lambda a, b: sum(S.mean(by[k]) for k in range(a, b) if k in by)
print(f'  sum days -21..-1 {100*cum(-21,0):+.1f}%   day 0..1 {100*cum(0,2):+.1f}%   days 2..13 {100*cum(2,14):+.1f}%   days 14..27 {100*cum(14,28):+.1f}%')
