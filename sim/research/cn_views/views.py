import pickle, sys, statistics as S, math
sh=pickle.load(open(sys.argv[1],'rb')); H=float(sys.argv[2]) if len(sys.argv)>2 else 0.05
def med(x): x=[v for v in x if v==v]; return S.median(x) if len(x)>=10 else float('nan')
good=[p for p in sh.values() if p['fi'] is not None and p['pi'] is not None and p['pi']>=3 and p['fi']+12<len(p['rows'])
      and all(p['rows'][i] for i in range(p['pi']-3,p['fi']+13)) and p['rows'][p['fi']-1]['doing1']>=100]
good=[p for p in good if all(p['rows'][i]['jp']>0 for i in range(p['pi']+1,p['fi']))]
def series(p,variant,h=H,lag_eps=3,b=1.0,tail=1.0):
    R=p['rows'];A=0;out={}
    for i in range(p['pi']-3,p['fi']+13):
        r=R[i]; airing=i<=p['fi']; neweps=max(r['aired1']-r['aired0'],0)
        inflow=max(r['T'],0)
        if airing: A=max(A*(1-h)+inflow-max(r['hold'],0)-max(r['drop'],0),0)
        else: A=0
        v=A*neweps
        if isinstance(variant,int) and variant>=2: v+=b*inflow*(r['aired0'] if airing else p['N'])*(1 if airing else tail)
        if isinstance(variant,int) and variant>=3 and not airing: v+=max(-r['doing'],0)*lag_eps
        if variant=='doing': v=r['doing1']
        if variant=='T': v=max(r['T'],0)
        if variant=='jp': v=r['jp']
        out[i]=v
    return out
V=[('jp (Annict, 参照)','jp',{}),('在看存量','doing',{}),('新增标记','T',{}),
   ('M1 追番','1',{}),('M2 追番+补番','2',{}),('M2 补番折半','2',{'b':0.5,'tail':0.5}),('M3 +完结后补完','3',{}),('M3 补番折半','3',{'b':0.5,'tail':0.5})]
air=list(range(0,13,2)); post=[0,1,2,3,4,6,8,12]
print(f'h={H}  shows={len(good)}   (relative to airing weeks 1-4)')
print('variant'.ljust(18),'airing wk:',' '.join(f'{k:>5}' for k in air),' | after finale:',' '.join(f'{o:>5}' for o in post),' | r(log) vs jp')
for name,var,kw in V:
    a={k:[] for k in air}; f={o:[] for o in post}; cors=[]
    vv=int(var) if var.isdigit() else var
    for p in good:
        s=series(p,vv,**kw); pi,fi=p['pi'],p['fi']
        base=S.mean(s[pi+k] for k in range(1,5))
        if base<=0: continue
        for k in air:
            if pi+k<fi: a[k].append(s[pi+k]/base)
        for o in post: f[o].append(s[fi+o]/base)
        j=series(p,'jp'); xs=[(math.log(s[i]+1),math.log(j[i]+1)) for i in range(pi,fi+9)]
        if len(xs)>5 and S.pstdev([x for x,_ in xs])>0 and S.pstdev([y for _,y in xs])>0: cors.append(S.correlation(*zip(*xs)))
    print(name.ljust(16),'        ',' '.join(f'{med(a[k]):5.2f}' for k in air),' |              ',' '.join(f'{med(f[o]):5.2f}' for o in post),f' | {med(cors):.2f}')

good=[p for p in good if all(p['rows'][i]['jp']>0 for i in range(p['pi']+1,p['fi']))]
print('\n[only shows with JP data every airing week: %d]'%len(good))
print('\nsensitivity of M3 (post-finale shape)')
for kw in ({'h':0.03},{'h':0.08},{'lag_eps':1},{'lag_eps':6}):
    f={o:[] for o in post}; a={k:[] for k in air}
    for p in good:
        s=series(p,3,**kw); pi,fi=p['pi'],p['fi']; base=S.mean(s[pi+k] for k in range(1,5))
        if base<=0: continue
        for k in air:
            if pi+k<fi: a[k].append(s[pi+k]/base)
        for o in post: f[o].append(s[fi+o]/base)
    print(str(kw).ljust(16),' '.join(f'{med(a[k]):5.2f}' for k in air),' | ',' '.join(f'{med(f[o]):5.2f}' for o in post))
# cross-section and noise
print('\ncross-section (log corr of airing-weeks mean across shows) and week-to-week noise during airing')
import collections
def amean(s,p): return S.mean(s[i] for i in range(p['pi']+1,p['fi']))
rows=[]
for p in good:
    if p['fi']-p['pi']<5: continue
    m3=series(p,3); jp=series(p,'jp'); dg=series(p,'doing')
    if amean(jp,p)<=0 or any(jp[i]==0 for i in range(p['pi']+1,p['fi'])): continue
    rows.append((amean(m3,p),amean(jp,p),amean(dg,p),p['rows'][p['fi']+12]['T1'],
        [abs(math.log((m3[i]+1)/(m3[i-1]+1))) for i in range(p['pi']+2,p['fi'])],
        [abs(math.log((jp[i]+1)/(jp[i-1]+1))) for i in range(p['pi']+2,p['fi'])]))
L=lambda k:[math.log(r[k]+1) for r in rows]
print('M3 vs JP views r=%.2f   在看存量 vs JP r=%.2f   M3 vs total marks r=%.2f'%(S.correlation(L(0),L(1)),S.correlation(L(2),L(1)),S.correlation(L(0),L(3))))
print('median |weekly log change| airing: M3 %.3f   JP %.3f'%(S.median([x for r in rows for x in r[4]]),S.median([x for r in rows for x in r[5]])))
small=[r for r in rows if r[2]<300]
print('small shows (在看<300, n=%d): M3 %.3f  JP %.3f'%(len(small),S.median([x for r in small for x in r[4]]),S.median([x for r in small for x in r[5]])))
