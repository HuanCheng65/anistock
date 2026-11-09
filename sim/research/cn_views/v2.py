import pickle, sys, statistics as S, math
sh=pickle.load(open(sys.argv[1],'rb'))
def med(x): x=[v for v in x if v==v]; return S.median(x) if len(x)>=10 else float('nan')
good=[p for p in sh.values() if p['fi'] is not None and p['pi'] is not None and p['pi']>=3 and p['fi']+12<len(p['rows'])
      and all(p['rows'][i] for i in range(p['pi']-3,p['fi']+13)) and p['rows'][p['fi']-1]['doing1']>=100]
# CN-native: of everyone who marked the show, the share that ends up 看过 12 weeks after the finale
fin=[]
for p in good:
    r=p['rows'][p['fi']+12]; st=r  # need stock levels: T1 total; done level not stored -> rebuild from deltas
done_share=[]
for p in good:
    R=p['rows']; T=R[p['fi']+12]['T1']; d=sum(R[i]['done'] for i in range(p['pi']-3,p['fi']+13))
    done_share.append(d/T if T else 0)
print('看过 gained from 3 weeks before premiere to 12 after finale / all marks: median %.2f'%med(done_share))
jpc=[p for p in good if all(p['rows'][i]['jp']>0 for i in range(p['pi']+1,p['fi']))]
def series(p,h=0.05,per_ep=False,c=None,lag=3,spread=1,tail_only=False):
    R=p['rows'];A=0;out={};N=p['N'];carry=[0]*8
    for i in range(p['pi']-3,p['fi']+13):
        r=R[i]; airing=i<=p['fi']; ne=max(r['aired1']-r['aired0'],0); inflow=max(r['T'],0)
        if airing:
            dec=(1-h)**ne if per_ep else (1-h)
            A=max(A*dec+inflow-max(r['hold'],0)-max(r['drop'],0),0)
        else: A=0
        v=A*ne
        backlog=(r['aired0'] if airing else N)
        eps=backlog if c is None else backlog*(c+(1-c)/3)   # finishers watch all, the rest about a third
        if not (tail_only and airing):
            for k in range(spread): carry[k]+=inflow*eps/spread
        else: v+=inflow*backlog
        if not airing: v+=max(-r['doing'],0)*lag
        v+=carry.pop(0); carry.append(0)
        out[i]=v
    return out
def jp(p): return {i:p['rows'][i]['jp'] for i in range(p['pi']-3,p['fi']+13)}
air=[2,6,10]; post=[0,1,2,3,4,6,8,12]
def show(name,f,sel=jpc):
    a={k:[] for k in air}; q={o:[] for o in post}; cors=[]
    for p in sel:
        s=f(p); pi,fi=p['pi'],p['fi']; base=S.mean(s[pi+k] for k in range(1,5))
        if base<=0: continue
        for k in air:
            if pi+k<fi: a[k].append(s[pi+k]/base)
        for o in post: q[o].append(s[fi+o]/base)
        j=jp(p); xs=[(math.log(s[i]+1),math.log(j[i]+1)) for i in range(pi,fi+9)]
        cors.append(S.correlation(*zip(*xs)))
    print(name.ljust(26),' '.join(f'{med(a[k]):5.2f}' for k in air),' | ',' '.join(f'{med(q[o]):5.2f}' for o in post),f' | {med(cors):.2f}')
c=med(done_share)
print('airing wk 2 6 10 | finale +0 +1 +2 +3 +4 +6 +8 +12 | r vs jp      (%d shows)'%len(jpc))
show('jp 参照',jp)
show('M3 (上一版)',lambda p:series(p))
show('按集弃番 h=5%/集',lambda p:series(p,per_ep=True))
show('+补番完成率',lambda p:series(p,per_ep=True,c=c))
show('+补番分两周看完',lambda p:series(p,per_ep=True,c=c,spread=2))
show('+分三周',lambda p:series(p,per_ep=True,c=c,spread=3))
