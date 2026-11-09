import pickle, sys, json, math
sys.path.insert(0,'/home/starrydream/anisotck')
from engine import valuation as V
from sim.fit_valuation import tail_var_fit
st=pickle.load(open(sys.argv[1],'rb')); proxy=json.load(open(sys.argv[2]))['tail']['cn']
trans=[round(math.exp(mu+0.5*v),6) for t,mu,v,n in st if t<28]
late=[(t,mu+0.5*v,n) for t,mu,v,n in st if t>=28]
# one scale on the proxy's floor and excesses (its shape from 2018-2025), n-weighted on the log
k=math.exp(sum(n*(lm-math.log(V.tail_formula(proxy,t,1.0))) for t,lm,n in late)/sum(n for _,_,n in late))
tail={**proxy,'trans':trans,'floor':round(proxy['floor']*k,6),'excess':round(proxy['excess']*k,6),'excess2':round(proxy['excess2']*k,6),
      'var':tail_var_fit(st)}
print('scale',round(k,3)); print(json.dumps(tail))
for t,lm,n in late: print(t, round(math.exp(lm),3), round(V.tail_formula(tail,t,1.0),3))
json.dump(tail,open(sys.argv[3],'w'))
