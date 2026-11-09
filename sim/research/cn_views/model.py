import pickle, sys, statistics as S, math
sh=pickle.load(open(sys.argv[1],'rb'))
def med(x): return S.median(x) if len(x)>=10 else float('nan')
good=[]
for w,p in sh.items():
    fi,pi,R=p['fi'],p['pi'],p['rows']
    if fi is None or pi is None or pi<3 or fi+12>=len(R) or any(R[i] is None for i in range(pi-3,fi+13)): continue
    if R[fi-1]['doing1']<100: continue
    good.append(p)
print('shows',len(good))
# 1. completers: 在看 that turn into something else after the finale (12 weeks)
cr=[sum(max(-p['rows'][p['fi']+o]['doing'],0) for o in range(0,13))/p['rows'][p['fi']-1]['doing1'] for p in good]
print('share of pre-finale 在看 that leave 在看 within 12 weeks: median %.2f  IQR %.2f-%.2f'%(med(cr),*S.quantiles(cr,n=4)[::2]))
def active(p,h):
    R=p['rows'];A=0;out={}
    for i in range(p['pi']-3,p['fi']+13):
        r=R[i]; inflow=max(r['T'],0) if i<=p['fi'] else 0
        A=max(A*(1-h)+inflow-max(r['hold'],0)-max(r['drop'],0),0); out[i]=A
    return out
for h in (0,0.03,0.05,0.08,0.1,0.15):
    rat=[active(p,h)[p['fi']-1]/(sum(max(-p['rows'][p['fi']+o]['doing'],0) for o in range(0,13))+1e-9) for p in good]
    # airing shape: A over airing weeks rel. to weeks 1-4
    sh_=[]
    for p in good:
        a=active(p,h); base=S.mean(a[p['pi']+k] for k in range(1,5)) or 1
        sh_.append([a[p['pi']+k]/base for k in range(0,13)])
    print(f'h={h:.2f}  active@finale / completers = {med(rat):.2f}   active over airing wk0..12:',' '.join(f'{med([s[k] for s in sh_ if len(s)>k]):.2f}' for k in range(0,13,2)))
