# Does an episode's comment surprise predict the next 4 weeks of the heat index (what futures settle on)?
import json, sqlite3, statistics as S, math
from datetime import date, timedelta
R='/home/starrydream/anisotck/'
W={'cn':0.5,'us':0.3,'jp':0.2}; PL={'us':116352.0,'cn':1772.0,'jp':1649.0}
hh={}
for w,d,us,cn,jp in sqlite3.connect(f'file:{R}data/derived/heat_backfill.db?mode=ro',uri=True).execute("select work_id,day,us,cn,jp from heat_history"):
    hh.setdefault(str(w),{})[d]={'us':us,'cn':cn,'jp':jp}
new=json.load(open(R+'data/fast/cnv/markets.json'))['series']
m=json.load(open(R+'sim/matches.json'))['matches']; b2a={int(v['bgm']):k for k,v in m.items() if v.get('bgm')}
eps={}
for bid,e in sqlite3.connect(f'file:{R}data/history.db?mode=ro',uri=True).execute("select bgm_id, episodes from bgm_subject"):
    if e: eps[int(bid)]=json.loads(e)
for i,d in sqlite3.connect(f'file:{R}data/anistock.db?mode=ro',uri=True).execute("select external_id,data from snapshots where source='bgm' and kind='episodes' order by day"):
    eps[int(i)]=[e for e in json.loads(d)['data'] if e.get('type',0)==0]
def comp(aid,d,k):
    r=hh.get(aid,{}).get(d)
    if k=='newcn': v=new.get(aid,{}).get(d,{}).get('cn'); return v
    if r is None: return None
    if k=='idx': return sum(W[mk]*(r[mk] or 0)/PL[mk]*14437 for mk in W)
    return r[k]
def avg(aid,a,b,k):
    v=[comp(aid,(a+timedelta(days=i)).isoformat(),k) for i in range((b-a).days)]
    v=[x for x in v if x is not None]
    return S.mean(v) if len(v)>=0.7*(b-a).days else None
pts={k:[] for k in ('idx','us','cn','jp','newcn')}
for bid,E in eps.items():
    aid=b2a.get(bid)
    if not aid or aid not in hh: continue
    E=[e for e in E if e.get('airdate') and e['airdate'].count('-')==2 and e.get('comment') is not None and '2025-11-15'<=e['airdate']<='2026-10-05']
    E.sort(key=lambda e:e['airdate']); seen=set(); E=[e for e in E if not (e['airdate'] in seen or seen.add(e['airdate']))]
    for j in range(4,len(E)):
        cp=S.mean(p['comment'] for p in E[j-3:j]); c=E[j]['comment']
        if cp<10 or c<=0: continue
        x=math.log(c/cp); a=date(*map(int,E[j]['airdate'].split('-')))
        for k in pts:
            pre=avg(aid,a-timedelta(days=7),a,k); post=avg(aid,a+timedelta(days=1),a+timedelta(days=29),k)
            if pre and post and pre>0 and post>0: pts[k].append((x,math.log(post/pre),aid))
for k,p in pts.items():
    if len(p)<30: print(k,'n',len(p)); continue
    x=[a for a,_,_ in p]; z=[b for _,b,_ in p]; mx,mz=S.mean(x),S.mean(z)
    b=sum((u-mx)*(v-mz) for u,v in zip(x,z))/sum((u-mx)**2 for u in x)
    print(f"{k:6} n={len(p):4} shows={len({q[2] for q in p}):3}  slope {b:+.3f}  corr {S.correlation(x,z):+.3f}  sd(z) {S.pstdev(z):.3f}  "
          f"=> a 35% surprise moves the 4-week average by {math.exp(b*math.log(1.35))-1:+.1%}")
