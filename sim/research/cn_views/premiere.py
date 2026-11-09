"""How regular is the premiere week's shortfall? Per show: the premiere week's views / the mean of weeks 1-3
(engine/cn_views.py on the archive's weekly counts), and what it relates to.  .venv/bin/python premiere.py PANEL"""
import sys, pickle, math, statistics as S
from datetime import date, timedelta
sys.path.insert(0, '/home/starrydream/anisotck')
from engine import cn_views as C
P = pickle.load(open(sys.argv[1], 'rb')); W = P['weeks']; D = [date.fromisoformat(w[:10]) for w in W]
rows = []
for sid, s in P['shows'].items():
    eps = s['eps']; air = []; prev = 0
    for d in sorted(eps):
        if eps[d] > prev:
            air += [date.fromisoformat(d)] * (eps[d] - prev); prev = eps[d]
    if len(air) < 6 or air[0] < date(2025, 7, 1) or air[0] > date(2026, 9, 15):
        continue
    stocks = {D[i]: (v[1], v[2], v[3], v[4]) for i, wk in enumerate(W) if (v := s['stock'][wk])}
    pre = [d for d in stocks if d <= air[0]]
    if not pre or min(stocks) > air[0] - timedelta(days=14):
        continue
    raw = C.raw_views(stocks, air, D[-1])
    wk = lambda k: sum(raw.get(air[0] + timedelta(days=7 * k + j), 0) for j in range(7)) / 7
    base = S.mean(wk(k) for k in (1, 2, 3))
    if base < 20:
        continue
    st0 = stocks[max(pre)]
    wish = s['stock'][W[[i for i, d in enumerate(D) if d == max(pre)][0]]][0]
    rows.append((wk(0) / base, base, wish / max(st0[0] + st0[1] + st0[2] + st0[3] + wish, 1), sid))
r = [x[0] for x in rows]
q = S.quantiles(r, n=10)
print(f'{len(rows)} shows: premiere week / weeks 1-3, median {S.median(r):.2f}, middle half {q[2]:.2f}-{q[7]:.2f}, 10-90% {q[0]:.2f}-{q[8]:.2f}')
L = lambda k: [math.log(x[k]) for x in rows]
print(f'  log ratio vs log size (weeks 1-3): corr {S.correlation(L(0), L(1)):+.2f}')
print(f'  log ratio vs pre-air share of 想看 among all marks: corr {S.correlation(L(0), [x[2] for x in rows]):+.2f}')
for lo, hi in ((0, 100), (100, 400), (400, 1e9)):
    sub = [x[0] for x in rows if lo <= x[1] < hi]
    if len(sub) > 5:
        print(f'  size {lo}-{hi}: n={len(sub)} median {S.median(sub):.2f}')
x = [r[2] for r in rows]; y = L(0)
mx, my = S.mean(x), S.mean(y); b = sum((u - mx) * (v - my) for u, v in zip(x, y)) / sum((u - mx) ** 2 for u in x); a = my - b * mx
res = [v - a - b * u for u, v in zip(x, y)]
print(f'  log ratio = {a:+.2f} {b:+.2f} x 想看 share; spread (sd of log) {S.pstdev(y):.2f} -> {S.pstdev(res):.2f}')
for lo, hi in ((0, .3), (.3, .5), (.5, .7), (.7, 1)):
    sub = [r[0] for r in rows if lo <= r[2] < hi]
    if len(sub) > 5:
        print(f'  想看 share {lo:.1f}-{hi:.1f}: n={len(sub)} median ratio {S.median(sub):.2f}')
# held out: fit on even AniList ids, test on odd, and back
for fit_par, test_par in ((0, 1), (1, 0)):
    tr = [r for r in rows if r[3] % 2 == fit_par]; te = [r for r in rows if r[3] % 2 == test_par]
    xs = [r[2] for r in tr]; ys = [math.log(r[0]) for r in tr]
    mx, my = S.mean(xs), S.mean(ys); bb = sum((u - mx) * (v - my) for u, v in zip(xs, ys)) / sum((u - mx) ** 2 for u in xs); aa = my - bb * mx
    rt = [math.log(r[0]) - aa - bb * r[2] for r in te]
    print(f'  fit on {"even" if fit_par == 0 else "odd"} ids: {aa:+.2f} {bb:+.2f} x share; held-out sd {S.pstdev([math.log(r[0]) for r in te]):.2f} -> {S.pstdev(rt):.2f}, mean residual {S.mean(rt):+.2f}')
