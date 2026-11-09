import os
# The 热度快讯 link: an episode's comments relative to the previous episodes' (x) vs its week's views relative to
# the previous weeks' (z), on the new cn series.
import json, sqlite3, statistics as S, math
from datetime import date
R='/home/starrydream/anisotck/'
mk=json.load(open(R+'data/fast/cnv/markets.json'))['series']
m=json.load(open(R+'sim/matches.json'))['matches']; b2a={int(v['bgm']):k for k,v in m.items() if v.get('bgm')}
eps={}
h=sqlite3.connect(f'file:{R}data/history.db?mode=ro',uri=True)
for bid,e in h.execute("select bgm_id, episodes from bgm_subject"):
    if e: eps[int(bid)]=json.loads(e)
db=sqlite3.connect(f'file:{R}data/anistock.db?mode=ro',uri=True)
for i,d in db.execute("select external_id,data from snapshots where source='bgm' and kind='episodes' order by day"):
    eps[int(i)]=[e for e in json.loads(d)['data'] if e.get('type',0)==0]
pts=[]
for bid,E in eps.items():
    aid=b2a.get(bid)
    if not aid or aid not in mk: continue
    ser=mk[aid]
    E=[e for e in E if e.get('airdate') and e['airdate'].count('-')==2 and e.get('comment') is not None and e['airdate']<='2026-10-25']
    E.sort(key=lambda e:e['airdate'])
    seen=set(); E=[e for e in E if not (e['airdate'] in seen or seen.add(e['airdate']))]   # one episode per air day
    J=int(os.environ.get('LAG','0'))
    for k in range(4,len(E)-J):    # skip ep1 burst: k>=4 uses eps 2..k
        prev=E[k-3:k]; c=E[k]['comment']; cp=S.mean(p['comment'] for p in prev)
        v=ser.get(E[k+J]['airdate'],{}).get('jp'); vp=[ser.get(p['airdate'],{}).get('jp') for p in prev]
        if not v or None in vp or min(vp)<=0 or cp<10 or c<=0: continue
        pts.append((math.log(c/cp), math.log(v/S.mean(vp)), cp, aid))
print('episodes',len(pts),'shows',len({p[3] for p in pts}))
x=[p[0] for p in pts]; z=[p[1] for p in pts]
mx,mz=S.mean(x),S.mean(z); b=sum((a-mx)*(c-mz) for a,c in zip(x,z))/sum((a-mx)**2 for a in x)
res=[c-mz-b*(a-mx) for a,c in zip(x,z)]
print('z = %.3f + %.3f x   resid var %.4f   var z %.4f   corr %.2f'%(mz-b*mx,b,S.pvariance(res),S.pvariance(z),S.correlation(x,z)))
for lo,hi in ((10,30),(30,100),(100,1e9)):
    sub=[p for p in pts if lo<=p[2]<hi]
    if len(sub)>20:
        xs=[p[0] for p in sub]; zs=[p[1] for p in sub]; mx,mz=S.mean(xs),S.mean(zs)
        bb=sum((a-mx)*(c-mz) for a,c in zip(xs,zs))/sum((a-mx)**2 for a in xs)
        print(f'  comments/ep {lo}-{hi}: n={len(sub)} slope {bb:.3f} corr {S.correlation(xs,zs):.2f}')
