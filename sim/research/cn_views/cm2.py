import json, sqlite3, statistics as S, math, pickle, sys
from datetime import date
R='/home/starrydream/anisotck/'
db=sqlite3.connect(f'file:{R}data/anistock.db?mode=ro',uri=True)
b=sqlite3.connect(f'file:{R}data/derived/heat_backfill.db?mode=ro',uri=True)
m=json.load(open(R+'sim/matches.json'))['matches']; b2an={int(v['bgm']):v.get('annict') for v in m.values() if v.get('bgm')}
eps={};ann={}
for i,d in db.execute("select external_id,data from snapshots where source='bgm' and kind='episodes' order by day"): eps[int(i)]=json.loads(d)['data']
for i,d in db.execute("select external_id,data from snapshots where source='annict' and kind='work' order by day"): ann[int(i)]=json.loads(d)
P=lambda s:date(*map(int,s.split('-')))
today=date(2026,11,5); out=[]
for sid,E in eps.items():
    E=sorted([e for e in E if e.get('airdate') and e['airdate'].count('-')==2 and float(e.get('sort') or 0).is_integer() and e['sort']>0 and (today-P(e['airdate'])).days>=14],key=lambda e:e['sort'])
    if not 8<=len(E)<=30: continue
    a=ann.get(b2an.get(sid) or -1)
    if not a: continue
    ae={int(e['sortNumber']//100) if e['sortNumber']>=100 else int(e['sortNumber']):e['recordsCount'] for e in a['episodes'] if e.get('recordsCount') is not None}
    c=[e['comment'] for e in E]; j=[ae.get(int(e['sort'])) for e in E]
    if None in j[:8] or S.mean(j[1:3])<20 or S.mean(c[1:3])<10: continue
    # follower model on weekly stocks, read at each episode's air date
    st=sorted(b.execute("select at,done,doing,on_hold,dropped from bgm_anime_weekly where subject_id=? order by at",(sid,)).fetchall())
    if len(st)<10: continue
    air=[P(e['airdate']) for e in E]; A=0; prev=None; Aat={}
    k=0
    for at,dn,dg,h,dr in st:
        d=date.fromisoformat(at[:10]); T=(dn or 0)+(dg or 0)+(h or 0)+(dr or 0)
        if prev:
            ne=sum(1 for x in air if prev[0]<x<=d)
            A=max(A*(0.95**ne)+max(T-prev[1],0)-max((h or 0)-prev[2],0)-max((dr or 0)-prev[3],0),0)
            for i,x in enumerate(air):
                if prev[0]<x<=d: Aat[i]=A
        prev=(d,T,h or 0,dr or 0)
    if not all(i in Aat for i in range(8)): continue
    N=len(E); ent=[]; pv=None
    for at,dn,dg,h,dr in st:
        d=date.fromisoformat(at[:10]); T=(dn or 0)+(dg or 0)+(h or 0)+(dr or 0)
        if pv: ent.append((d,max(T-pv,0)))
        pv=T
    def pe(H):
        A=0;prev=None;Ak={}
        for (d,inf),(at,dn,dg,h,dr),(at0,dn0,dg0,h0,dr0) in zip(ent,st[1:],st):
            ne=[i for i,x in enumerate(air) if date.fromisoformat(at0[:10])<x<=d]
            A=max(A*((1-H)**len(ne))+inf-max((h or 0)-(h0 or 0),0)-max((dr or 0)-(dr0 or 0),0),0)
            for i in ne: Ak[i]=A
        return [Ak.get(i,0)+sum(inf*(0.52+(0.48 if i<N/3 else 0)) for d,inf in ent if d>air[i]) for i in range(8)]
    PE={H:pe(H) for H in (0.0,0.03,0.05,0.08,0.12)}
    def fol(H):
        A=0;Ak={}
        for (d,inf),(at,dn,dg,h,dr),(at0,dn0,dg0,h0,dr0) in zip(ent,st[1:],st):
            ne=[i for i,x in enumerate(air) if date.fromisoformat(at0[:10])<x<=d]
            A=max(A*((1-H)**len(ne))+inf-max((h or 0)-(h0 or 0),0)-max((dr or 0)-(dr0 or 0),0),0)
            for i in ne: Ak[i]=A
        return [Ak[i] for i in range(8)]
    cret=S.mean(c[5:8])/S.mean(c[1:3]); hc=min(max(1-cret**(1/4.5),-0.05),0.3)
    vol=S.mean(c[1:8]); w=vol/(vol+40)
    hs=w*hc+(1-w)*0.05
    FG=fol(0.05); FS=fol(hs)
    mod=[Aat[i] for i in range(8)]
    r=lambda v:S.mean(v[5:8])/S.mean(v[1:3])
    out.append(dict(fg=S.mean(FG[5:8])/S.mean(FG[1:3]),fs=S.mean(FS[5:8])/S.mean(FS[1:3]),hc=hc,hs=hs,**{f'pe{H}':S.mean(PE[H][5:8])/S.mean(PE[H][1:3]) for H in PE},sid=sid,c=r(c),j=r(j),mod=r(mod),c1=c[0]/S.mean(c[1:3]),j1=j[0]/S.mean(j[1:3]),lvl_c=S.mean(c[1:8]),lvl_j=S.mean(j[1:8]),lvl_m=S.mean(mod[1:8])))
print('shows',len(out))
L=lambda k:[math.log(x[k]) for x in out]
for k in ('c','mod'): print(f'retention ep6-8/ep2-3  {k}: median {S.median(x[k] for x in out):.2f}   corr with Annict across shows {S.correlation(L(k),L("j")):.2f}')
cm=[(a+b)/2 for a,b in zip(L('c'),L('mod'))]; print('  average of both (log): corr %.2f'%S.correlation(cm,L('j')))
print('ep1 / ep2-3: comments %.2f  Annict %.2f'%(S.median(x['c1'] for x in out),S.median(x['j1'] for x in out)))
for k in ('lvl_c','lvl_m'): print(f'level across shows {k} vs Annict: r={S.correlation(L(k),L("lvl_j")):.2f}')
cm=[(a+b)/2 for a,b in zip(L('lvl_c'),L('lvl_m'))]; print('  average of both (log): r=%.2f'%S.correlation(cm,L('lvl_j')))
for H in (0.0,0.03,0.05,0.08,0.12):
    print(f'model per-episode viewers, h={H}: retention ep6-8/ep2-3 median {S.median(x[f"pe{H}"] for x in out):.2f}  corr with Annict {S.correlation(L(f"pe{H}"),L("j")):.2f}')
print('implied per-episode drop from comments: median %.3f'%S.median(x['hc'] for x in out))
for k,n in (('fg','followers, h=5% for all'),('fs','followers, h per show from comments (shrunk)')):
    print(f'{n}: retention median {S.median(x[k] for x in out):.2f}  corr with Annict {S.correlation(L(k),L("j")):.2f}')
