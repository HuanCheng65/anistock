"""Around the finale: mean value change per day, by days from the finale (all listed finished shows), to see where
'buy the day before the finale, hold' earns.  .venv/bin/python finale.py VO_DIR INPUTS_DIR"""
import sys, statistics as S
sys.path.insert(0, '/home/starrydream/anisotck/sim'); sys.path.insert(0, '/home/starrydream/anisotck')
from pathlib import Path
from datetime import date
import quickcheck as Q
w = Q.World(Path(sys.argv[1]), Path(sys.argv[2]))
by = {}
for c in w.ids:
    fin = w.sched.get(c, {}).get('finale')
    if not fin:
        continue
    for i in range(1, len(w.days)):
        r = w.total_return(c, i - 1, i)
        if r is None:
            continue
        k = (date.fromisoformat(w.days[i]) - date.fromisoformat(fin)).days
        if -14 <= k <= 60:
            m = w.market(i - 1, i, [x for x in w.ids if x != c])
            by.setdefault(k // 7 if k >= 0 else -1 - (-k - 1) // 7, []).append(r - m)
for k in sorted(by):
    v = by[k]
    print(f'week {k:+3d} from the finale: excess {100*S.mean(v):+.2f}%/day  sum over the week {700*S.mean(v):+.1f}%  n={len(v)}')
