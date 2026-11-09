import pickle, sys, statistics as S
sh=pickle.load(open(sys.argv[1],'rb'))
def med(x): return S.median(x) if len(x)>=10 else float('nan')
keys=['T','wish','done','doing','hold','drop','jp']
print('aligned at the FINALE week; flows as % of the 在看 stock just before the finale; jp relative to its airing mean')
prof={k:{o:[] for o in range(-4,13)} for k in keys}; n=0
for w,p in sh.items():
    fi,pi=p['fi'],p['pi']
    if fi is None or pi is None or fi+12>=len(p['rows']) or any(p['rows'][fi+o] is None for o in range(-4,13)): continue
    base=p['rows'][fi-1]['doing1']
    if base<100: continue
    air=[p['rows'][i]['jp'] for i in range(pi+1,fi) if p['rows'][i]]
    ja=S.mean(air) if air else 0
    n+=1
    for o in range(-4,13):
        r=p['rows'][fi+o]
        for k in keys[:-1]: prof[k][o].append(100*r[k]/base)
        if ja>0: prof['jp'][o].append(r['jp']/ja)
print('shows',n)
print('wk'.ljust(6),' '.join(f'{o:>6}' for o in range(-4,13)))
for k in keys: print(k.ljust(6),' '.join(f'{med(prof[k][o]):6.2f}' for o in range(-4,13)))
