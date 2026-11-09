# Long-running shows: does the follower model with h=5% keep a sensible level relative to JP, vs seasonal shows?
import pickle, sys, statistics as S
from datetime import date, timedelta
P=pickle.load(open(sys.argv[1],'rb')); W=P['weeks']; D=[date.fromisoformat(w[:10]) for w in W]
H=0.05; out={'seasonal':[], 'long':[]}
for w,s in P['shows'].items():
    ep=s['eps']; 
    if not ep: continue
    days=sorted(ep); span=(date.fromisoformat(days[-1])-date.fromisoformat(days[0])).days
    kind='long' if span>250 else 'seasonal' if 60<span<120 else None
    if not kind: continue
    A=0; rat=[]
    for i in range(len(W)-1):
        a,b=s['stock'][W[i]],s['stock'][W[i+1]]
        if not a or not b: A=0; continue
        T=lambda x:x[1]+x[2]+x[3]+x[4]
        A=max(A*(1-H)+max(T(b)-T(a),0)-max(b[3]-a[3],0)-max(b[4]-a[4],0),0)
        lo=D[i]; jp=sum(s['heat'].get((lo+timedelta(days=k)).isoformat(),(0,0,0))[2] for k in range(1,8))
        neweps=sum(1 for d in ep if lo<date.fromisoformat(d)<=D[i+1])
        if jp>20 and neweps and lo>=date(2026,3,1) and lo>=date.fromisoformat(days[0])+timedelta(days=35) and lo<=date.fromisoformat(days[-1]):
            rat.append((A*neweps/jp, b[2]/jp))
    if len(rat)>=4: out[kind].append((S.median(r[0] for r in rat), S.median(r[1] for r in rat), w))
for k,v in out.items():
    print(k, len(v), 'followers/JP median %.1f   在看存量/JP median %.1f'%(S.median(x[0] for x in v),S.median(x[1] for x in v)))
print(sorted(out['long'])[:12])
