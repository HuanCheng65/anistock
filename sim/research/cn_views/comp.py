import pickle, sys, statistics as S
from datetime import date, timedelta
from collections import defaultdict
P=pickle.load(open(sys.argv[1],'rb')); W=P['weeks']; D=[date.fromisoformat(w[:10]) for w in W]
def panel(s):
    ep=s['eps']; days=sorted(ep); 
    if not days: return None
    N=max(ep.values()); first=date.fromisoformat(days[0]); last=date.fromisoformat(max(d for d in days if ep[d]==N))
    if N<6 or N>30 or (last-first).days>220 or ep[days[0]]>2: return None
    rows=[]
    for i in range(len(W)-1):
        a,b=s['stock'][W[i]],s['stock'][W[i+1]]
        if not a or not b: rows.append(None); continue
        lo,hi=D[i],D[i+1]
        aired0=max([e for d,e in ep.items() if date.fromisoformat(d)<=lo] or [0])
        aired1=max([e for d,e in ep.items() if date.fromisoformat(d)<=hi] or [0])
        jp=sum(s['heat'].get((lo+timedelta(days=k)).isoformat(),(0,0,0))[2] for k in range(1,8))
        T=lambda x:x[1]+x[2]+x[3]+x[4]
        rows.append(dict(i=i,lo=lo,wish=b[0]-a[0],done=b[1]-a[1],doing=b[2]-a[2],hold=b[3]-a[3],drop=b[4]-a[4],
            T=T(b)-T(a), doing0=a[2], doing1=b[2], T1=T(b), wish1=b[0], aired0=aired0, aired1=aired1, jp=jp))
    pi=next(i for i in range(len(W)-1) if D[i]<first<=D[i+1])  if D[0]<first<=D[-1] else None
    fi=next(i for i in range(len(W)-1) if D[i]<last<=D[i+1]) if D[0]<last<=D[-1] else None
    return dict(N=N,first=first,last=last,rows=rows,pi=pi,fi=fi)
shows={w:p for w,s in P['shows'].items() if (p:=panel(s))}
pickle.dump(shows,open(sys.argv[2],'wb'))
print(len(shows))
