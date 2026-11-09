# engine/cn_views.py on the research panel: weekly profile vs Annict (should match v2.py's "+补番完成率" row)
import pickle, sys, statistics as S, math
from datetime import date, timedelta
sys.path.insert(0,'/home/starrydream/anisotck')
from engine.cn_views import daily_views
P=pickle.load(open(sys.argv[1],'rb')); sh=pickle.load(open(sys.argv[2],'rb'))
W=P['weeks']; D=[date.fromisoformat(w[:10]) for w in W]
def med(x): return S.median(x) if len(x)>=10 else float('nan')
air_=[2,6,10]; post=[0,1,2,3,4,6,8,12]; a={k:[] for k in air_}; q={o:[] for o in post}; j_={o:[] for o in post}; cors=[]
for w,p in sh.items():
    if p['fi'] is None or p['pi'] is None or p['pi']<3 or p['fi']+12>=len(p['rows']) or any(p['rows'][i] is None for i in range(p['pi']-3,p['fi']+13)): continue
    if p['rows'][p['fi']-1]['doing1']<100 or not all(p['rows'][i]['jp']>0 for i in range(p['pi']+1,p['fi'])): continue
    s=P['shows'][w]
    stocks={D[i]:(v[1],v[2],v[3],v[4]) for i,wk in enumerate(W) if (v:=s['stock'][wk])}
    eps=s['eps']; air=[]; prev=0
    for d in sorted(eps):
        e=eps[d]
        if e>prev: air+= [date.fromisoformat(d)]*(e-prev); prev=e
    v=daily_views(stocks,air,D[-1])
    wk=lambda i: sum(v.get(D[i]+timedelta(days=k),0) for k in range(1,8))
    pi,fi=p['pi'],p['fi']; base=S.mean(wk(pi+k) for k in range(1,5))
    if base<=0: continue
    for k in air_:
        if pi+k<fi: a[k].append(wk(pi+k)/base)
    for o in post: q[o].append(wk(fi+o)/base)
    xs=[(math.log(wk(i)+1),math.log(p['rows'][i]['jp']+1)) for i in range(pi,fi+9)]
    cors.append(S.correlation(*zip(*xs)))
print(len(cors),'shows  airing',' '.join(f'{med(a[k]):.2f}' for k in air_),'| post',' '.join(f'{med(q[o]):.2f}' for o in post),'| r %.2f'%med(cors))
