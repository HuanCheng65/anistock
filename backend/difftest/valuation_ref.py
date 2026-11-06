"""Reference values for backend/src/vfilter.rs `matches_python`: the same cases through engine/valuation.py, printed
bit for bit (repr). Run from the repo root after a refit, and paste the numbers into the Rust test.

    uv run python backend/difftest/valuation_ref.py
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT))

from engine import valuation as V  # noqa: E402
from server.config import load  # noqa: E402

cfg = load()
p = cfg.params
p["valuation"]["sequel_option"] = 1   # (off by default; the reference covers it)
fp = V.filter_params(p)
M = V.MARKETS
w = p["markets"]
today = 740000
s1 = V.schedule([today - 20, today - 13, today - 6], 12, False)
s2 = V.schedule([], None, True)
m1, v1 = [7.5, 3.0, 2.0], [0.05, 0.2, 0.03]
own1 = {m: V.att_path(fp, p, m, m1[i], v1[i], today, s1, today - 20, False, 120) for i, m in enumerate(M)}
m2 = [8.0, 3.5, 2.5]
own2 = {m: V.att_path(fp, p, m, m2[i], 0.01, today, s2, today - 900, True, 120) for i, m in enumerate(M)}
tot = {m: [a + b + 1000.0 * (i + 1.0) for a, b in zip(own1[m], own2[m])] for i, m in enumerate(M)}
a = V.value_from_paths(own1, tot, w, 110925.85, p, 10000, 1234567, 0.0325, False)
b = V.value_from_paths(own2, tot, w, 110925.85, p, 10000, 555, 0.03, True)
print("a", repr(a))
print("b", repr(b))
print("own1[us][5]", repr(own1["us"][5]))
print("own1[cn][100]", repr(own1["cn"][100]))
s3 = V.schedule([today + 10], 12, False)
o3 = V.att_path(fp, p, "us", 7.0, 0.1, today, s3, today + 10, False, 120)
print("o3[15]", repr(o3[15]), "o3[110]", repr(o3[110]))
print("post_finale_factor", repr(V.post_finale_factor(fp, p, "us", today, today + 30)))
# the long tail (phase 1): a show that finished 200 days ago, with a score prior; the value includes the closed-form
# sum beyond the horizon
mult = V.tail_mult(fp, 78.0, True, 250000.0)
s4 = V.schedule([today - 290 + 7 * k for k in range(12)], 12, False)
fin4 = s4[-1]
own4 = {m: V.att_path(fp, p, m, m1[i], v1[i], today, s4, s4[0], False, 400, mult) for i, m in enumerate(M)}
tot4 = {m: [x + 500.0 * (i + 1.0) for x in own4[m]] for i, m in enumerate(M)}
tails = {m: (V.tail_of(fp, m), mult, today + 400 - fin4 - 7) for m in M}
c = V.value_from_paths(own4, tot4, w, 110925.85, p, 10000, 4321, 0.03, False, tails)
c0 = V.value_from_paths(own4, tot4, w, 110925.85, p, 10000, 4321, 0.03, False)
print("mult", repr(mult))
print("own4[us][0]", repr(own4["us"][0]), "own4[cn][399]", repr(own4["cn"][399]))
print("c", repr(c), "c0", repr(c0))
print("pff_cn", repr(V.post_finale_factor(fp, p, "cn", today - 100, today, mult)))
print("tail_var", repr(V.tail_var(V.tail_of(fp, "us"), 250)))
# absolute revenue (phase 1 step 2): platform sizes and the switch scale
plat = ({"us": 105000.0, "cn": 450.0, "jp": 1460.0}, 0.83)
print("d", repr(V.value_from_paths(own4, tot4, w, 110925.85, p, 10000, 4321, 0.03, False, tails, plat)))
# the sequel option (phase 1 step 3): before the finale, after it, an announced sequel
sp = fp["sequel"]
z = V.sequel_z(sp, 78.0, "LIGHT_NOVEL", False, True)
zs = z + sp["beta"]["sequel"]
disc = V.discount_day(0.03, p)
fin1 = s1[-1]
e1 = V.sequel_extra(fp, "us", 7.5, 0.05, today, fin1, today - 20, z, zs, disc)
e4 = V.sequel_extra(fp, "cn", 3.0, 0.2, today, fin4, s4[0], z, zs, disc)
e5 = V.sequel_extra(fp, "jp", 2.0, 0.03, today, fin4, s4[0], z, zs, disc, [today + 200, today + 300])
print("z", repr(z), "e1", repr(e1), "e4", repr(e4), "e5", repr(e5))
ex = {"us": e1, "cn": e4, "jp": e5}
print("g", repr(V.value_from_paths(own4, tot4, w, 110925.85, p, 10000, 4321, 0.03, False, tails, plat, ex)))
# a scheduled switch 30 days ahead: the relative rule until then, its per-unit revenue of that day held after
print("sw", repr(V.value_from_paths(own4, tot4, w, 110925.85, p, 10000, 4321, 0.03, False, tails, (plat[0], plat[1], 30))))
# relations not collected: the option is not updated as time passes (phase 1 fix)
print("e6", repr(V.sequel_extra(fp, "cn", 3.0, 0.2, today, fin4, s4[0], z, zs, disc, None, False)))
# the joint filter's outlier-prone update (v0.6.0): a one-episode spike in cn, and pi = 0 (the plain update)
c = V.prior_cov([0.35, 1.08, 0.86], 0.48)
mj, cj = V.kf_predict_joint([1.0, -4.0, -2.5], c, 3, [-0.02, -0.022, -0.027], [0.01, 0.015, 0.019])
mr, cr, llr, svr = V.kf_update_joint_robust(mj, cj, 1, -1.5, 0.0235, 0.013, 0.2537, 4.0)
print("robust_joint", repr(mr), repr(cr[1][1]), repr(cr[0][2]), repr(llr), repr(svr))
print("robust_joint_pi0", V.kf_update_joint_robust(mj, cj, 1, -1.5, 0.0235, 0.0, 0.2537, 4.0) == V.kf_update_joint(mj, cj, 1, -1.5, 0.0235, 4.0))
# the us episode week's days so far (v0.6.1): segments, and the observation after 3 days (premiere, a mid-tercile week)
# and 7 days
wk = [7.1, 8.4, 7.9, 7.6, 7.2, 7.05, 6.9]
print("pseg", V.partial_segment(fp, 6.0, False), V.partial_segment(fp, 7.0, False), V.partial_segment(fp, 9.0, False), V.partial_segment(fp, 9.0, True))
print("po0_3", repr(V.partial_observation(fp, 0, wk[:3], -0.019)), "po2_3", repr(V.partial_observation(fp, 2, wk[:3], -0.0065)))
print("po3_6", repr(V.partial_observation(fp, 3, wk[:6], 0.0)), "po1_7", repr(V.partial_observation(fp, 1, wk, -0.0065)), "po_1", V.partial_observation(fp, 1, wk[:1], 0.0))
# the pairwise prior correlations (v0.6.1): [us-cn, us-jp, cn-jp]; a common rho is the same three
print("prior_cov_pw", repr(V.prior_cov([0.385, 0.923, 0.322], [0.3185, 0.3983, 0.8146])), V.prior_cov([0.35, 1.08, 0.86], [0.48] * 3) == c)
# the us drift by heat class (v0.6.1): classes, and a class's path (its offset from week 4 on)
print("hc", V.heat_class(fp, 6.0), V.heat_class(fp, 7.0), V.heat_class(fp, 8.0), repr(V.for_class(fp, 2)["drift_by_week"]["us"]), V.for_class(fp, None) is fp)
print("hc_path", repr(V.att_path(V.for_class(fp, 2), p, "us", 8.1, 0.05, today, s1, today - 30, False, 60)[59]))
