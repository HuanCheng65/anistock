# The cn long tail measured directly on the new series (engine/cn_views.py over the Bangumi archive), against the
# tail the fit takes from the collection-comment proxy.
import pickle, sys, json, math, statistics as S
from datetime import date, timedelta
sys.path.insert(0,'/home/starrydream/anisotck')
from engine.cn_views import daily_views
from engine import valuation as V
from sim.fit_valuation import bucket
P=pickle.load(open(sys.argv[1],'rb')); W=P['weeks']; D=[date.fromisoformat(w[:10])+timedelta(days=1) for w in W]
tail=json.load(open(sys.argv[2]))['tail']['cn']; bump=float(sys.argv[3])
until=D[-1]; wins=[(0,7),(7,14),(14,21),(21,28),(28,56),(56,90),(90,150),(150,240),(240,400)]
by=[]
for w,s in P['shows'].items():
    eps=s['eps']; air=[]; prev=0
    for d in sorted(eps):
        e=eps[d]
        if e>prev: air+=[date.fromisoformat(d)]*(e-prev); prev=e
    if len(air)<6 or len(air)>30: continue
    fin=air[-1]
    if (fin-air[0]).days>220 or fin>until-timedelta(days=35) or fin<date(2025,7,15): continue
    stocks={D[i]:(v[1],v[2],v[3],v[4]) for i,wk in enumerate(W) if (v:=s['stock'][wk])}
    if not stocks or min(stocks)>air[0]-timedelta(days=14): continue
    v=daily_views(stocks,air,until)
    # relative to the mean of the 28 days before the finale x bump (the finale week's value as the basis instead, as
    # the fit's us transition, priced the tail ~20% above the replay's: 'buy before the finale, sell 8 days after'
    # lost 6-7 points; with this basis no post-finale strategy or drift is significant, 2026-11-09)
    lv=[v[fin-timedelta(days=k)] for k in range(1,29) if fin-timedelta(days=k) in v]
    if len(lv)<21: continue
    L=S.mean(lv)
    if L<=0 or L<5: continue
    base=fin+timedelta(days=7); pts=[]
    for a,b in wins:
        if base+timedelta(days=b)>until: break
        x=[v.get(base+timedelta(days=k)) for k in range(a,b)]
        x=[y for y in x if y is not None]
        if len(x)>=0.8*(b-a): pts.append(((a+b)/2, S.mean(x)/(L*bump)))
    by.append(pts)
st=bucket(by)
print(len(by),'finished shows')
print('t(days)  n   native mean factor   proxy-fit model   current-file model')
cur=json.load(open('/home/starrydream/anisotck/sim/calibration/valuation_filter.json'))['tail']['cn']
for t,mu,var,n in st:
    def model(p):
        if t<28: return p['trans'][min(int(t//7),len(p['trans'])-1)]
        return V.tail_formula(p,t,1.0)
    print(f'{t:6.1f} {n:4d}   {math.exp(mu+var/2):8.3f}            {model(tail):8.3f}          {model(cur):8.3f}')
pickle.dump(st,open(sys.argv[4],'wb'))
