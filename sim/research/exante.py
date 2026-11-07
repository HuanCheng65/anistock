import json, sys, math, statistics as st
s = json.load(open(sys.argv[1])); inp = sys.argv[2]; day0 = sys.argv[3] if len(sys.argv) > 3 else '2026-07-08'
d0 = {x['id']: x for x in json.load(open(f'{inp}/{day0}.json'))['shows']}
names = {int(k): v for k, v in json.load(open(f'{inp}/2026-11-05.json'))['cn_titles'].items()}
rows = []
for c, cls, path in s['paths']:
    pts = [r for r in path if r[0] >= day0 and r[2] > 0]
    if not pts or pts[0][0] != day0 or c not in d0: continue
    x = d0[c]
    from datetime import date as D
    age = (D.fromisoformat(day0) - D.fromisoformat(x['first_air'])).days
    if x['long_runner'] or x['aired'] < 1 or not (int(__import__('os').environ.get('AMIN','0')) <= age <= int(__import__('os').environ.get('AMAX','14'))): continue
    v0, v1 = pts[0][2], pts[-1][2]; div = sum(r[3] for r in pts[1:])
    cap0 = v0  # per share; weights need shares: skip, equal weight
    rows.append((x['att']['us'], c, names.get(c, '')[:12], v0, v1, div, (v1 + div) / v0 - 1, (v1 + div) / v0))
rows.sort(reverse=True)
n = len(rows); t = n // 3
print(f'{n} airing shows on {day0} (not long-runners); total return to 11-05 (value + dividends)')
for name, part in (('top third', rows[:t]), ('middle', rows[t:2*t]), ('bottom third', rows[2*t:])):
    print(f'  {name:12s} median {100*st.median(r[6] for r in part):+.0f}%  mean log {st.fmean(math.log(r[7]) for r in part):+.2f}')
print('  top 3:', [(r[2], f'{100*r[6]:+.0f}%') for r in rows[:3]])
def rank(v): 
    o = sorted(range(len(v)), key=lambda i: v[i]); r = [0]*len(v)
    for k, i in enumerate(o): r[i] = k
    return r
a = rank([r[0] for r in rows]); b = rank([r[6] for r in rows])
ma, mb = st.fmean(a), st.fmean(b)
print('  spearman(heat on day 0, total return) %+.2f' % (sum((x-ma)*(y-mb) for x, y in zip(a, b)) / math.sqrt(sum((x-ma)**2 for x in a) * sum((y-mb)**2 for y in b))))
