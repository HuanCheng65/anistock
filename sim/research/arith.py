import json, math, statistics as st, random, sys
from datetime import date
from collections import defaultdict
s = json.load(open(sys.argv[1]))
inp = {x['id']: x for x in json.load(open('data/fast/inputs_fix5/2026-11-05.json'))['shows']}
by = defaultdict(lambda: defaultdict(list))
for c, cls, path in s['paths']:
    x = inp.get(c)
    if not x or x['long_runner']: continue
    f = date.fromisoformat(x['first_air'])
    for a, b in zip(path, path[1:]):
        if a[2] <= 0 or b[2] <= 0: continue
        r = (b[2] + b[3]) / a[2] - 1
        age = (date.fromisoformat(b[0]) - f).days
        k = 'pre' if -14 <= age < 0 else (age // 7 if 0 <= age < 28 else ('4-11' if 28 <= age < 84 else None))
        if k is not None: by[k][c].append(r)
random.seed(1)
out = []
for k in ('pre', 0, 1, 2, 3, '4-11'):
    shows = list(by[k].values()); allr = [r for v in shows for r in v]
    bs = sorted(st.fmean([r for v in random.choices(shows, k=len(shows)) for r in v]) for _ in range(500))
    out.append(f'{k}: {100*st.fmean(allr):+.2f}% [{100*bs[25]:+.2f}, {100*bs[475]:+.2f}]')
print('  '.join(out))
