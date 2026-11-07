"""Does the first week's update predict the later revision? (filter states from DBG_JSTATE)"""
import json, sys, math, statistics as st
from datetime import date, timedelta
from collections import defaultdict
S = defaultdict(dict)
for line in open(__import__('os').environ.get('JS','data/fast/jstate.tsv')):
    c, d, m0, m1, m2, v0, v1, v2, act, hc = line.rstrip('\n').split('\t')
    S[int(c)][d] = (float(m0), float(m1), float(m2), float(v0), act)
inp = {x['id']: x for x in json.load(open('data/fast/inputs_fix5/2026-11-05.json'))['shows']}
names = {int(k): v for k, v in json.load(open('data/fast/inputs_fix5/2026-11-05.json'))['cn_titles'].items()}
K1, K2 = int(sys.argv[1]) if len(sys.argv) > 1 else 7, int(sys.argv[2]) if len(sys.argv) > 2 else 28
m = int(sys.argv[3]) if len(sys.argv) > 3 else 0
rows = []
for c, days in S.items():
    x = inp.get(c)
    if not x or x['long_runner']: continue
    f = date.fromisoformat(x['first_air'])
    g = lambda k: days.get((f + timedelta(days=k)).isoformat())
    a, b, e = g(-1), g(K1), g(K2)
    if not (a and b and e): continue
    if b[4][m] != '1': continue
    pre, w1, w4 = a[m], b[m], e[m]
    rows.append((c, f.isoformat()[:7], pre, w1, w4, a[3], b[3]))
def ols(y, xs):
    # y on [1, xs...]
    import itertools
    n = len(y); k = len(xs) + 1
    X = [[1.0] + [col[i] for col in xs] for i in range(n)]
    XtX = [[sum(X[i][a] * X[i][b] for i in range(n)) for b in range(k)] for a in range(k)]
    Xty = [sum(X[i][a] * y[i] for i in range(n)) for a in range(k)]
    # solve
    M = [r[:] + [v] for r, v in zip(XtX, Xty)]
    for i in range(k):
        p = max(range(i, k), key=lambda r: abs(M[r][i])); M[i], M[p] = M[p], M[i]
        for r in range(k):
            if r != i:
                f = M[r][i] / M[i][i]; M[r] = [u - f * w for u, w in zip(M[r], M[i])]
    beta = [M[i][k] / M[i][i] for i in range(k)]
    res = [y[i] - sum(beta[j] * X[i][j] for j in range(k)) for i in range(n)]
    s2 = sum(r * r for r in res) / (n - k)
    # se via inverse of XtX
    A = [r[:] + [1.0 if i == j else 0.0 for j in range(k)] for i, r in enumerate(XtX)]
    for i in range(k):
        p = max(range(i, k), key=lambda r: abs(A[r][i])); A[i], A[p] = A[p], A[i]
        piv = A[i][i]; A[i] = [u / piv for u in A[i]]
        for r in range(k):
            if r != i:
                f = A[r][i]; A[r] = [u - f * w for u, w in zip(A[r], A[i])]
    se = [math.sqrt(s2 * A[i][k + i]) for i in range(k)]
    return beta, se
for season in ('2026-07', '2026-10', None):
    R = [r for r in rows if season is None or r[1] == season or (season == '2026-10' and r[1] == '2026-09')]
    if len(R) < 8: continue
    rev = [r[4] - r[3] for r in R]; up1 = [r[3] - r[2] for r in R]; pre = [r[2] for r in R]
    b, se = ols(rev, [up1, pre])
    b2, se2 = ols(rev, [[r[3] for r in R]])
    print(f'{season or "both"}: {len(R)} shows; revision day {K1}->{K2} on [first-week update, prior]:'
          f' {b[1]:+.3f} ({se[1]:.3f}), {b[2]:+.3f} ({se[2]:.3f}); on level at day K1 alone {b2[1]:+.4f} ({se2[1]:.4f}) = {b2[1]/(K2-K1):+.5f}/day;  mean prior var {st.fmean(r[5] for r in R):.3f} -> day {K1} var {st.fmean(r[6] for r in R):.3f}')
