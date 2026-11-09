# Which stocks and phases carry "top-5 by 5-day value gain, hold 7 days"?
import sys, json, statistics as S
sys.path.insert(0,'/home/starrydream/anisotck/sim'); sys.path.insert(0,'/home/starrydream/anisotck')
from pathlib import Path
import quickcheck as Q
out, inputs = Path(sys.argv[1]), Path(sys.argv[2])
w=Q.World(out, inputs)
rows=[]
for i in range(19, len(w.days)-7, 1):
    univ=[c for c in w.ids if w.alive(c,i) and w.alive(c,i-5)]
    g=lambda c:w.total_return(c,i-5,i)
    top=sorted([c for c in univ if g(c) is not None], key=g, reverse=True)[:5]
    m=w.market(i,i+7,univ)
    for c in top:
        r=w.total_return(c,i,i+7)
        if r is None: continue
        sc=w.sched.get(c,{}); fa=sc.get('first_air'); fin=sc.get('finale')
        d=w.days[i]
        ph='pre' if fa and d<fa else ('post' if fin and d>fin else 'air')
        rows.append((r-m, g(c), c, d, ph))
print('trades',len(rows),'mean excess %.2f%%'%(100*S.mean(r[0] for r in rows)))
for ph in ('pre','air','post'):
    x=[r[0] for r in rows if r[4]==ph]
    if x: print(ph, len(x), 'mean excess %.2f%%'%(100*S.mean(x)))
by={}
for r in rows: by.setdefault(r[2],[]).append(r[0])
print('top contributors (sum of excess, trades):')
for c,v in sorted(by.items(), key=lambda kv:-sum(kv[1]))[:8]:
    print(f'  {w.title.get(c,c)[:20]:22} {100*sum(v):7.1f}  n={len(v)}  sched={w.sched.get(c,{}).get("first_air")}..{w.sched.get(c,{}).get("finale")}')
