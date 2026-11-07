import json, sys, math, statistics as st, os
from datetime import date, timedelta
from collections import defaultdict
S = defaultdict(dict)
for line in open(os.environ.get('JS', 'data/fast/jstate_g0.tsv')):
    c, d, m0, m1, m2, v0, v1, v2, act, hc = line.rstrip('\n').split('\t')
    S[int(c)][d] = ([float(m0), float(m1), float(m2)], act)
inp = {x['id']: x for x in json.load(open('data/fast/inputs_fix5/2026-11-05.json'))['shows']}
K1, K2 = int(sys.argv[1]), int(sys.argv[2])
rows = []
for c, days in S.items():
    x = inp.get(c)
    if not x or x['long_runner']: continue
    f = date.fromisoformat(x['first_air'])
    a, b = days.get((f + timedelta(days=K1)).isoformat()), days.get((f + timedelta(days=K2)).isoformat())
    if a and b and a[1] == '111' and b[1] == '111':
        rows.append((a[0][0], [b[0][m] - a[0][m] for m in range(3)], [a[0][m] for m in range(3)]))
xs = [r[0] for r in rows]; mx = st.fmean(xs)
for m in range(3):
    ys = [r[1][m] for r in rows]; my = st.fmean(ys)
    sxx = sum((x - mx) ** 2 for x in xs); b = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx
    res = [y - my - b * (x - mx) for x, y in zip(xs, ys)]; se = math.sqrt(sum(e * e for e in res) / (len(xs) - 2) / sxx)
    print(f'market {m}: revision day {K1}->{K2} on us level at day {K1}: {b:+.3f} ({se:.3f})   mean revision {my:+.3f}')
print(len(rows), 'shows with all three markets')
