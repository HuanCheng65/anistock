"""Level-dependent in-season drift: hotter shows fade more slowly in their first weeks. Fit by maximum likelihood
of every observation the close's filter takes (sim/fit_valuation.py close_days, plus gamma), with season hold-out.

    uv run python data/fast/an/fit_gamma.py INPUTS_DIR
"""
import json, math, sys
from datetime import date
from pathlib import Path

ROOT = Path('/home/starrydream/anisotck')
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT / 'sim'))
import fit_valuation as FV  # noqa
from engine import valuation as V  # noqa

MARKETS = V.MARKETS
REF = [7.1, 1.5, 3.25]
# parameter layout: (market, weeks) per entry
LAYOUT = [(0, (1,)), (0, (2, 3)), (0, (4, 5)), (1, (1,)), (1, (2,)), (2, (1,)), (2, (2,))]


HIST = {1: 0.0021, 2: 0.0026, 3: 0.0010}   # us, from the 2018-2025 history (data/fast/an/hist_gamma.py)


def gam(g, m, w):
    """per-day drift per log unit of the us level: us from the history; cn/jp = g[m-1] x the same weekly shape"""
    h = HIST.get(w, 0.0)
    return h if m == 0 else g[m - 1] * h


def loglik_show(x, cal, g):
    ad = FV.air_days(x)
    if not ad or x["long_runner"] or x["aired"][0] > 0:
        return 0.0, 0
    prem = ad[0]
    idx = {d: i for i, d in enumerate(x["days"])}
    pre_i = idx.get(prem)
    if pre_i is None or x["prior_us"][pre_i] is None or x["prior_us"][pre_i] <= 0:
        return 0.0, 0
    lp = math.log(x["prior_us"][pre_i])
    dr = [tuple(cal["drift_by_week"][m]) for m in MARKETS]
    hp = {"drift_heat": cal.get("drift_heat"), "drift_by_week": cal["drift_by_week"]}
    h4 = None
    q = [cal["q"][m] for m in MARKETS]; r = [cal["r"][m] for m in MARKETS]
    op = [cal["outlier_p"][m] for m in MARKETS]; orr = [cal["outlier_r"][m] for m in MARKETS]
    bump = [cal["finale_bump"][m] for m in MARKETS]; bvar = [cal["finale_bump_var"][m] for m in MARKETS]
    cu = cal["curve_us"]
    mean = [cal["prior_bias"][m] + cal["prior_beta"][m] * lp for m in MARKETS]
    cov = V.prior_cov([cal["p0"][m] for m in MARKETS], V.prior_corr(cal))
    fin = FV.finale_day(x)
    day = prem
    ys = []
    prev_raw = [None, None, None]
    ll, n = 0.0, 0
    for i, d in enumerate(x["days"]):
        if d < prem or (fin is not None and d >= fin + 7):
            continue
        for t in range(day, d):
            w = (t - prem) // 7
            gk = [gam(g, k, w) for k in range(3)]
            x0 = mean[0] - REF[0]
            mean = [mean[k] + FV.d_at(dr[k], prem, t) + gk[k] * x0 for k in range(3)]
            # x_k' = x_k + g_k (x_us - ref): F = I + g e_us^T
            Fm = [[(1.0 if a == b else 0.0) + (gk[a] if b == 0 else 0.0) for b in range(3)] for a in range(3)]
            FC = [[sum(Fm[a][k] * cov[k][b] for k in range(3)) for b in range(3)] for a in range(3)]
            cov = [[sum(FC[a][k] * Fm[b][k] for k in range(3)) + (q[a] if a == b else 0.0) for b in range(3)] for a in range(3)]
        day = d
        if h4 is None and d - prem >= 7 * V.DRIFT_WEEKS[0]:
            h4 = mean[0]
            if hp["drift_heat"]:
                dr[0] = tuple(V.for_class(hp, V.heat_class(hp, mean[0]))["drift_by_week"]["us"])
        a_ = V.last_air_before(ad, d)
        k = None if a_ is None else d - a_
        in_fin = fin is not None and fin <= d < fin + 7
        drift_us = FV.d_at(dr[0], prem, d)
        if ys and (k == 0 or len(ys) >= 7):
            covers = fin is not None and d - len(ys) <= fin + 6 and d - 1 >= fin
            xr = bvar[0] if covers else 0.0
            y = V.week_observation(sum(ys) / len(ys), len(ys), drift_us)
            mean, cov, l, _ = V.kf_update_joint_robust(mean, cov, 0, y, r[0] + xr, op[0], orr[0], FV.CLIP)
            ll += l; n += 1
            ys = []
        att = [x["att"][m][i] for m in MARKETS]
        obs = [x["obs"][m][i] for m in MARKETS]
        raw = att
        att = [att[m] / V.runup(cal, MARKETS[m], fin, d) for m in range(3)]
        if att[0] > 0 and obs[0]:
            v = att[0] / bump[0] if in_fin else att[0]
            ys.append(math.log(v / (cu[k] if (k is not None and k < 7) else 1.0)))
        for m in (1, 2):
            if not x["src"][MARKETS[m]] or not obs[m] or att[m] <= 0:
                continue
            if prev_raw[m] is not None and raw[m] == prev_raw[m]:
                continue
            prev_raw[m] = raw[m]
            v = att[m] / bump[m] if in_fin else att[m]
            mean, cov, l, _ = V.kf_update_joint_robust(mean, cov, m, math.log(v), r[m] + (bvar[m] if in_fin else 0.0),
                                                       op[m], orr[m], FV.CLIP)
            ll += l; n += 1
    return ll, n


def total(shows, cal, g, keep=lambda sid: True):
    ll, n = 0.0, 0
    for sid, x in shows.items():
        if keep(sid):
            a, b = loglik_show(x, cal, g)
            ll += a; n += b
    return ll, n


def main():
    inputs = Path(sys.argv[1])
    cal = json.loads((ROOT / 'sim/calibration/valuation_filter.json').read_text())
    shows = FV.load(inputs)
    if isinstance(shows, tuple):
        shows = shows[0]
    z = [0.0, 0.0]
    ll0, n = total(shows, cal, z)
    print('us from history only:', {k: round(total(shows, cal, z, f)[0], 2) for k, f in (('summer', lambda s_: True),)} if False else '', end='')
    print(f'{n} observations; log-likelihood gamma=0: {ll0:.2f}')
    split = date(2026, 9, 15).toordinal()
    prem = {sid: FV.air_days(x)[0] for sid, x in shows.items() if FV.air_days(x)}
    groups = {'all': lambda sid: True,
              'summer': lambda sid: sid in prem and prem[sid] < split,
              'autumn': lambda sid: sid in prem and prem[sid] >= split}

    def fit(keep):
        f = lambda g: -total(shows, cal, list(g), keep)[0]
        best, _ = FV.nelder_mead(f, z, [0.5, 0.5], iters=int(sys.argv[2]) if len(sys.argv) > 2 else 150)
        return [round(v, 5) for v in best]
    g_all = fit(groups['all'])
    print('fit on all (cn, jp multiples):', g_all, f'll {total(shows, cal, g_all)[0]:.2f}')
    for tr, te in (('summer', 'autumn'), ('autumn', 'summer')):
        g = fit(groups[tr])
        a = total(shows, cal, z, groups[te])[0]; b = total(shows, cal, g, groups[te])[0]; c = total(shows, cal, g_all, groups[te])[0]
        print(f'fit on {tr}: {g}; held-out {te}: gamma=0 {a:.2f} -> fitted {b:.2f} (all-fit {c:.2f})')


if __name__ == '__main__':
    main()
