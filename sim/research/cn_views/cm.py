# Do a show's per-episode comments say anything about its viewer retention beyond the typical decline?
import json, sqlite3, statistics as S, math, pickle, sys
from datetime import date
R='/home/starrydream/anisotck/'
db=sqlite3.connect(f'file:{R}data/anistock.db?mode=ro',uri=True)
m=json.load(open(R+'sim/matches.json'))['matches']
b2an={int(v['bgm']):v.get('annict') for v in m.values() if v.get('bgm')}
b2al={int(v['bgm']):int(k) for k,v in m.items() if v.get('bgm')}
eps={};ann={}
for i,d in db.execute("select external_id,data from snapshots where source='bgm' and kind='episodes' order by day"): eps[int(i)]=json.loads(d)['data']
for i,d in db.execute("select external_id,data from snapshots where source='annict' and kind='work' order by day"): ann[int(i)]=json.loads(d)
today=date(2026,11,5); rows=[]
for sid,E in eps.items():
    E=[e for e in E if e.get('airdate') and e['airdate'].count('-')==2 and e.get('sort') and float(e['sort']).is_integer()]
    E=[e for e in E if (today-date(*map(int,e['airdate'].split('-')))).days>=14]   # matured
    if len(E)<8 or len(E)>30: continue
    E.sort(key=lambda e:e['sort']); c=[e['comment'] for e in E]
    if S.mean(c[1:3])<10: continue
    rc=S.mean(c[5:8])/S.mean(c[1:3])
    a=ann.get(b2an.get(sid) or -1); rj=None
    if a:
        ae=sorted([e for e in a['episodes'] if e.get('recordsCount') is not None],key=lambda e:e['sortNumber'])
        r=[e['recordsCount'] for e in ae]
        if len(r)>=8 and S.mean(r[1:3])>20: rj=S.mean(r[5:8])/S.mean(r[1:3])
    rows.append((sid,rc,rj,len(E),S.mean(c[1:3])))
both=[r for r in rows if r[2]]
print('shows with ≥8 matured eps:',len(rows),' with Annict too:',len(both))
print('median retention ep6-8 / ep2-3: comments %.2f  Annict records %.2f'%(S.median(r[1] for r in rows),S.median(r[2] for r in both)))
L=lambda k,rs:[math.log(r[k]) for r in rs]
print('across shows, log corr(comment retention, Annict retention) = %.2f'%S.correlation(L(1,both),L(2,both)))
big=[r for r in both if r[4]>=50]
print('  shows with ≥50 comments/ep (n=%d): r=%.2f'%(len(big),S.correlation(L(1,big),L(2,big))))
pickle.dump(rows,open(sys.argv[1],'wb'))
