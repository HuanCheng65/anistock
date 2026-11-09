import sys, statistics as S
sys.path.insert(0,'/home/starrydream/anisotck/sim'); sys.path.insert(0,'/home/starrydream/anisotck')
from pathlib import Path
from datetime import date
import quickcheck as Q
w=Q.World(Path(sys.argv[1]), Path(sys.argv[2])); lo,hi=int(sys.argv[3]),int(sys.argv[4])
contrib={}
for c in w.ids:
    fa=w.sched.get(c,{}).get('first_air'); fin=w.sched.get(c,{}).get('finale')
    if not fa: continue
    for i in range(5,len(w.days)-7):
        d=w.days[i]
        if d<fa or (fin and d>fin): continue
        wk=(date.fromisoformat(d)-date.fromisoformat(fa)).days//7
        if not lo<=wk<=hi: continue
        p=w.total_return(c,i-5,i); f=w.total_return(c,i,i+7)
        if p is None or f is None: continue
        contrib.setdefault(c,[]).append((p,f))
allp=[x for v in contrib.values() for x in v]; mx=S.mean(p for p,_ in allp); my=S.mean(f for _,f in allp)
tot=sum((p-mx)*(f-my) for p,f in allp)
print('weeks',lo,hi,'pairs',len(allp))
for c,v in sorted(contrib.items(), key=lambda kv:-sum((p-mx)*(f-my) for p,f in kv[1]))[:10]:
    s=sum((p-mx)*(f-my) for p,f in v)
    print(f'  {w.title.get(c,c)[:22]:24} share of covariance {s/tot:+.2f}  n={len(v)}  {w.sched.get(c,{}).get("first_air")}..{w.sched.get(c,{}).get("finale")}  long={w.sched.get(c,{}).get("long_runner")}')
