import sys, statistics as S
sys.path.insert(0,'/home/starrydream/anisotck/sim'); sys.path.insert(0,'/home/starrydream/anisotck')
from pathlib import Path
from datetime import date
import quickcheck as Q
w=Q.World(Path(sys.argv[1]), Path(sys.argv[2]))
b={}
for c in w.ids:
    fa=w.sched.get(c,{}).get('first_air'); fin=w.sched.get(c,{}).get('finale')
    if not fa: continue
    for i in range(5,len(w.days)-7):
        p=w.total_return(c,i-5,i); f=w.total_return(c,i,i+7)
        if p is None or f is None: continue
        d=w.days[i]
        if d<fa: k='pre'
        elif fin and d>fin: k='post'
        else: k='wk%d'%min((date.fromisoformat(d)-date.fromisoformat(fa)).days//7//2*2,12)
        b.setdefault(k,[]).append((p,f))
for k in sorted(b):
    x=[p for p,_ in b[k]]; y=[f for _,f in b[k]]
    mx,my=S.mean(x),S.mean(y); sl=sum((u-mx)*(v-my) for u,v in zip(x,y))/sum((u-mx)**2 for u in x)
    print(f'{k:6} n={len(x):5}  slope(next 7d on past 5d) {sl:+.3f}  mean next7 {100*my:+.2f}%')
