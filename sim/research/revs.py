import json, math, statistics as st, sys
from datetime import date
from collections import defaultdict
s = json.load(open(sys.argv[1]))
inp = {x['id']: x for x in json.load(open('data/fast/inputs_fix5/2026-11-05.json'))['shows']}
byage = defaultdict(list); bycal = defaultdict(list); bycal_old = defaultdict(list)
for c, cls, path in s['paths']:
    x = inp.get(c)
    if not x: continue
    f = date.fromisoformat(x['first_air'])
    for a, b in zip(path, path[1:]):
        if a[2] <= 0 or b[2] <= 0: continue
        r = math.log((b[2] + b[3]) / a[2])
        age = (date.fromisoformat(b[0]) - f).days
        if x['long_runner']: continue
        if -14 <= age < 0: byage['pre'].append(r)
        elif 0 <= age < 28: byage[age // 7].append(r); bycal[b[0]].append(r)
        elif age >= 28 and age < 84: bycal_old[b[0]].append(r)
print('mean daily revision (value + dividend, log) by weeks since premiere:')
for k in ('pre', 0, 1, 2, 3):
    v = byage[k]; print(f'  {k}: {100*st.fmean(v):+.2f}%  median {100*st.median(v):+.2f}%  n {len(v)}')
print('by calendar week, shows in weeks 0-3 vs shows in weeks 4-11 (same days):')
wk = defaultdict(lambda: [[], []])
for d, v in bycal.items(): wk[date.fromisoformat(d).isocalendar()[:2]][0] += v
for d, v in bycal_old.items(): wk[date.fromisoformat(d).isocalendar()[:2]][1] += v
for k in sorted(wk):
    a, b = wk[k]
    if len(a) >= 20:
        print(f'  {k}: new {100*st.fmean(a):+.2f}% (n {len(a)})   older {100*st.fmean(b):+.2f}% (n {len(b)})' if b else f'  {k}: new {100*st.fmean(a):+.2f}% (n {len(a)})')
