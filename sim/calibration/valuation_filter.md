# Valuation filter fit

Data: daily inputs 2026-07-01 to 2026-11-03 from engine/daily_inputs.py (v0.3.2 schema; market series rebuilt with the air-time fix in sim/reconstruct.py; 142 shows), i.e. the summer 2026 season and the first five weeks of autumn. Observations from a week after a show\'s finale are left out (the server reads them through the post-finale profile).

us within-episode curve (day 0..6 after airing, mean 1): [0.5952, 2.4934, 1.4687, 0.8811, 0.6225, 0.4996, 0.4395]

Joint filter (v0.3.2): one state per show, the three markets' log levels. Prior at the premiere: log(point-in-time us pre-air estimate) + bias_m, variances p0_m, common correlation rho = 0.534 (summer fit 0.646, autumn fit 0.539). 115 shows with a prior (53 premiering before 2026-09-15, 62 after). Two stages: each market's drift, q, r from its own filter; then bias, p0 and rho by maximum likelihood of the joint filter.

Plain Kalman filter (`--gaussian`, no outlier component): the outlier mixture was also fitted in v0.3.0 and gave no better held-out coverage, so the simpler model is used.

Outlier component (v0.6.0, `--outlier-only`, a profile fit with drift, q and r above fixed; standard inputs 2026-07-01 to 2026-11-03, new format): with probability `outlier_p` an observation carries extra noise `outlier_r`, and the update uses r + P(outlier | y) × outlier_r (`kf_update_joint_robust`). It is fitted per market because a one-episode spike (178789 無職転生 III, cn weekly values 73 → 256 → 78) moved the Gaussian filter's level almost all the way and the following week's value only partly back; the finale week's extra noise then carried the spike into the long tail. Log-likelihood of each market's own filter, Gaussian → mixture: us 32.8 → 41.3 (outlier_p 0.029, outlier_r 0.061; 958 observations), cn −124.4 → −111.9 (0.013, 0.254; 1,662), jp −524.5 → +8.7 (0.0017, 12.07; 1,576: a few gross Annict glitches). Coverage was the v0.3.0 criterion; the likelihood (and the single spikes it reads as level changes) is the reason to use it now.

Finale week: attention relative to the level two to three weeks before it: {'us': 1.507413, 'cn': 1.118205, 'jp': 0.966828}.

## us

drift by weeks since the premiere (0-3, 4-7, 8+) [-0.01912, -0.00653, 2e-05]/day, q 0.00380/day, r 0.0174 (own filter); prior: bias +0.254 + 1.000 × log(us pre-air estimate) (level-unbiased; maximum likelihood +0.230), variance 0.426 (joint filter).

80% interval coverage of the next observation: 87.0% (817) in sample; 90.2% (420) on held-out shows (fit on even ids, tested on odd); 66.1% (230) on autumn shows with the summer fit; 34.9% (587) on summer shows with the autumn fit. Prior bias fitted on summer alone +0.213, on autumn alone +0.344.

## cn

drift by weeks since the premiere (0-3, 4-7, 8+) [-0.01977, -0.0019, -0.00481]/day, q 0.00883/day, r 0.0235 (own filter); prior: bias -2.641 + 0.622 × log(us pre-air estimate) (level-unbiased; maximum likelihood -2.670), variance 0.910 (joint filter).

80% interval coverage of the next observation: 83.7% (1482) in sample; 82.3% (779) on held-out shows (fit on even ids, tested on odd); 82.0% (545) on autumn shows with the summer fit; 86.6% (937) on summer shows with the autumn fit. Prior bias fitted on summer alone -4.115, on autumn alone -0.657.

## jp

drift by weeks since the premiere (0-3, 4-7, 8+) [-0.01723, -0.00472, -0.00022]/day, q 0.00902/day, r 0.0588 (own filter); prior: bias +0.695 + 0.381 × log(us pre-air estimate) (level-unbiased; maximum likelihood +0.690), variance 0.295 (joint filter).

80% interval coverage of the next observation: 97.8% (1435) in sample; 98.6% (737) on held-out shows (fit on even ids, tested on odd); 67.1% (529) on autumn shows with the summer fit; 78.6% (906) on summer shows with the autumn fit. Prior bias fitted on summer alone +0.106, on autumn alone +2.077.

## Pre-air estimate until the premiere

Mean log growth of the point-in-time estimate from k days before the day before the premiere (k = 7, 14, 21, 28): [0.103771, 0.24072, 0.342765, 0.385468], variance [0.006617, 0.017171, 0.03502, 0.046655] ([79, 65, 64, 6] shows).

## After the finale

Attention by week after the finale week, relative to the level at its end (us observed, cn/jp the rebuilt series the revenue uses): {'us': [0.446671, 0.299977, 0.225845, 0.179913, 0.162655], 'cn': [0.49496, 0.325196, 0.239016, 0.174467, 0.148673], 'jp': [0.524435, 0.373252, 0.280391, 0.222116, 0.176345]}; then ×{'us': 0.939826, 'cn': 0.821925, 'jp': 0.95} a week. The game's `[valuation] post_finale_profile` (measured on AniList in v1) was [0.40, 0.30, 0.22, 0.16, 0.14], ×0.78.

## Arrival of a season's shows

Share of a season's pre-air attention not yet listed, by days from the season start (−30, −14, 0, +14, +30): [0.924121, 0.102761, 0.0, 0.0, 0.0] (seasons: ['2026-10-01']).

## 热度快讯 hourly curve

131 episodes. Share of 28-day comments by hour 1..24: [0.0, 0.0741, 0.1304, 0.1719, 0.2105, 0.2273, 0.2333, 0.2394, 0.25, 0.2667, 0.2805, 0.2982, 0.3235, 0.3519, 0.3824, 0.4167, 0.4316, 0.4507, 0.4737, 0.4868, 0.5116, 0.5238, 0.5439, 0.5714]

Noise variance of the hour-h count read as the cn level, r_cn + var log(count_h / (curve_h × own final)), by hour: [1.0, 0.9204, 0.4758, 0.3205, 0.3257, 0.3366, 0.3279, 0.2825, 0.3005, 0.2906, 0.1882, 0.1341, 0.1169, 0.1106, 0.1041, 0.1013, 0.0903, 0.0858, 0.0837, 0.0804, 0.0772, 0.0743, 0.0683, 0.064]

(Refitted 2026-11-07 from the same 131-episode sample with sim/fit_valuation.py fit_flash; the earlier figures used the previous episodes' mean in place of the episode's own final and so counted the signal as noise.)

## IPO discount

ipo_risk = 0: over the 61 shows listed in August-October of the v0.3.2 replay, the value 28 days after listing plus dividends relative to the model's value when the offer was priced is 0.935 value-weighted (1.158 equal-weighted, driven by three hits; 1.024 without them). The model's value already prices the uncertainty; a further discount was a riskless gain for subscribers (ipo_all +9.7 points over cap-weighted buy-and-hold with ipo_risk 0.25).


## Long tail (docs/v2-series.md §4)

Expected attention t days after the finale's week = level at the finale × finale bump × factor(t): weekly transition, then floor × yearly^(t/365) + slow excess × exp(−t/τ2) + fast excess × exp(−t/τ), the floor and the slow excess scaled by the score prior. The filter reads post-finale observations through the median factor (expected × exp(−D(t)/2)), the level a random walk whose variance grows by D(t), the spread of show-specific tails.

us: 1496 finished shows of the history backfill (TV/ONA 2018–2025; windows before any sign of a sequel; deflated attention / (mean of the 4 weeks before the finale's week × finale bump 1.507)); weeks 0–2 from the inputs ([50, 49, 49] shows).

| t (days after finale+7) | n | median factor | log var | expected (data) | expected (model) |
|---|---|---|---|---|---|
| 3 | 50 | 0.3853 | 0.144 | 0.4140 | (transition) |
| 10 | 49 | 0.2789 | 0.103 | 0.2937 | (transition) |
| 17 | 49 | 0.2111 | 0.101 | 0.2220 | (transition) |
| 29 | 1200 | 0.1538 | 0.299 | 0.1786 | 0.1789 |
| 60 | 1186 | 0.1171 | 0.457 | 0.1472 | 0.1490 |
| 90 | 1168 | 0.1162 | 0.340 | 0.1377 | 0.1318 |
| 136 | 1149 | 0.0879 | 0.471 | 0.1113 | 0.1168 |
| 182 | 1127 | 0.0877 | 0.444 | 0.1095 | 0.1080 |
| 273 | 1090 | 0.0761 | 0.456 | 0.0956 | 0.0970 |
| 364 | 1023 | 0.0663 | 0.515 | 0.0857 | 0.0888 |
| 456 | 964 | 0.0606 | 0.602 | 0.0819 | 0.0817 |
| 547 | 915 | 0.0556 | 0.690 | 0.0786 | 0.0754 |
| 638 | 857 | 0.0524 | 0.626 | 0.0716 | 0.0698 |
| 730 | 811 | 0.0490 | 0.612 | 0.0666 | 0.0648 |
| 912 | 734 | 0.0405 | 0.685 | 0.0570 | 0.0565 |
| 1095 | 659 | 0.0337 | 0.821 | 0.0508 | 0.0498 |
| 1277 | 585 | 0.0297 | 0.729 | 0.0428 | 0.0445 |
| 1460 | 518 | 0.0264 | 0.733 | 0.0382 | 0.0402 |
| 1825 | 391 | 0.0208 | 0.882 | 0.0324 | 0.0342 |
| 2191 | 276 | 0.0173 | 1.098 | 0.0300 | 0.0304 |
| 2556 | 179 | 0.0151 | 1.646 | 0.0344 | 0.0279 |
| 2921 | 82 | 0.0150 | 0.906 | 0.0236 | 0.0264 |

us: transition [0.414046, 0.293731, 0.222047]; then 0.0238 × 1.0000^(t/365) + 0.1025 × exp(−t/797) + 0.1009 × exp(−t/50); spread D(t) = 0.409(1−e^(−t/22)) + 0.1128·t/365.

Cross-check (data report: median ≈ 0.025 + 0.14·e^(−t/7 months) of the in-season mean; here the median factor × level·bump / in-season mean = × 1.239): 1 m: 0.146 vs 0.188, 3 m: 0.116 vs 0.131, 6 m: 0.084 vs 0.106, 12 m: 0.050 vs 0.085, 24 m: 0.030 vs 0.058, 36 m: 0.026 vs 0.042, 60 m: 0.025 vs 0.026, 96 m: 0.025 vs 0.017.


cn: 351 shows with Bangumi subject (collection) comments; the collection rate relative to the season's mean (incl. the two weeks after the finale) × 1.076 (the game's cn attention: in-season mean / finale week, 48 shows of the inputs) ÷ finale bump 1.118.

| t (days after finale+7) | n | median factor | log var | expected (data) | expected (model) |
|---|---|---|---|---|---|
| 4 | 271 | 1.4647 | 0.435 | 1.8210 | (transition) |
| 10 | 271 | 0.8192 | 0.558 | 1.0830 | (transition) |
| 18 | 271 | 0.5353 | 0.672 | 0.7489 | (transition) |
| 24 | 271 | 0.3902 | 0.739 | 0.5647 | (transition) |
| 42 | 269 | 0.2982 | 0.641 | 0.4108 | 0.4114 |
| 73 | 262 | 0.2260 | 0.642 | 0.3115 | 0.3114 |
| 120 | 257 | 0.1653 | 0.672 | 0.2314 | 0.2292 |
| 195 | 251 | 0.1120 | 0.741 | 0.1623 | 0.1666 |
| 320 | 224 | 0.0832 | 0.815 | 0.1250 | 0.1200 |
| 500 | 198 | 0.0568 | 0.904 | 0.0893 | 0.0922 |
| 750 | 170 | 0.0484 | 0.928 | 0.0770 | 0.0775 |
| 1100 | 133 | 0.0436 | 0.923 | 0.0692 | 0.0682 |
| 1550 | 91 | 0.0416 | 0.798 | 0.0620 | 0.0600 |
| 2200 | 31 | 0.0337 | 0.658 | 0.0468 | 0.0501 |

cn: transition [1.820989, 1.083025, 0.748894, 0.564722]; then 0.0922 × 0.9037^(t/365) + 0.2458 × exp(−t/165) + 0.3446 × exp(−t/43); spread D(t) = 0.705(1−e^(−t/5)) + 0.0496·t/365.

jp: no post-finale data anywhere (the rebuilt Annict series extends finished shows with a model; the Annict watch-record history is not collected yet): **the us shape, flagged**.

Tail level prior (us, mean log residual 3–36 months after the finale against the curve, 1128 shows): 0.120·(score−70)/10 + 0.482·ONA + popularity fifth within the finale year [0.279911, 0.03796, 0.0, 0.061764, 0.122181] (the middle fifth the reference) − 0.173 (R² 0.168, held out 0.196; residual sd 0.56). The relation with popularity is U-shaped, so one effect per fifth. Popularity is compared with the 2025 finales (`pop_ref`). Scales the whole post-transition curve, all markets. The data report's −1.035 for a linear percentile is a different normalisation (relative to the in-season mean, sequels included, a 'sequel aired' covariate).

Finale week (phase 1 review): the bump differs by show; its log variance across shows (us 0.084, cn 0.080, jp 0.003; `finale_bump_var`) is added to the noise of an observation that includes the finale's week. The us prior's elasticity refitted with the dynamics fixed: 0.95 (held out 0.98), log-likelihood +0.3: not significant, kept at 1.

Hold-out (§11): share of us factors inside the 80% interval (fit on even AniList ids, tested on odd; in sample in brackets): 12 months 82.6% of 517 (83.1%), 24 months 83.1% of 414 (85.6%), 36 months 81.4% of 349 (81.9%).


### Long tail: what the data can and cannot say (phase 1, step 1)

Fitted by `sim/fit_valuation.py INPUTS --history HISTORY_DB` (`sim/tail_history.py` reads the backfill of
`sim/history_backfill.py`; this fit: the final backfill snapshot of 2026-11-06 and the standard inputs 2026-07-01..11-03).

- **Form.** The doc's “floor + decaying excess” needs two decay speeds: one excess alone either misses the first months
  or makes the floor itself decay (a one-excess fit put the floor at ×0.81 a year). With a fast (τ ≈ 50 days) and a slow
  (τ ≈ 800 days) excess the us floor comes out flat (yearly 1.0000, at the bound): the 5–8-year medians still drift
  down (0.0208 → 0.0150, ~×0.9/yr) but those windows hold few, survivor-biased shows. Not tuned.
- **Mean and median.** The value uses the expected (lognormal mean) factor; the filter reads post-finale observations
  through the median (expected × e^(−D(t)/2)), and after the finale's week the level is a random walk whose variance
  grows by D(t). So the value of a finished show is a martingale under the model: the spread of show-specific tails
  is revealed by observations, not by a predictable drift.
- **Cross-check with the data report.** This fit's median tracks the report's table (no-sequel median at 12 months
  0.074 of the in-season mean; here 0.085), not its closed form 0.025 + 0.14·e^(−t/7 months) (0.050 at 12 months,
  which does not match the report's own table there). Left as it is.
- **cn.** The doc's §4.2 link (episode comments ÷ collection increments over the 4 weeks before the finale) cannot be
  calibrated: the collector has one day of Bangumi snapshots, and in the history backfill collection comments (the
  proxy) cluster when people mark a show finished, so their pre-finale rate says little about collection-count
  increments. The cn tail is therefore measured against the season's mean (incl. the two weeks after, the report's
  normalisation) and converted to the level at the finale with the game's own in-season/finale-week ratio. Deflated
  by a Bangumi platform index (median in-season collection-comment rate of the sample's shows by start year). The
  cn spread's fast part (0.705 within ~5 days) is mostly the page-interpolation noise of the proxy, not show-specific
  spread: live, the first post-finale cn observation will move the cn level with a gain near 1. Known limitation.
- **jp.** No post-finale data anywhere (the rebuilt Annict series assigns each episode's records to its air week;
  Annict's record history is not collected yet): the us shape. The doc's “jp's own first post-finale weeks” are not
  available (in the inputs they are a modelled extension), so they are not used.
- **Replay inputs.** cn/jp after the finale are not observed in the standard inputs: `sim/reconstruct.py` extends a
  finished show's last week with the model's median factor (it used the v1 profile before), so the filter reads them as
  “no news”, and realised cn/jp tail revenue in the replay runs at the median, below the mean the value prices. Live,
  cn comes from Bangumi collection increments (the collector keeps every finished work's subject snapshot daily) and
  jp from Annict record increments — not implemented in this step (no snapshot history to build it on).
- **Tail prior.** OLS on score and ONA only (the report's 16-covariate regression includes “a sequel aired”, future
  information): ×1.19 per 10 points, ×1.88 for ONA, R² 0.13. Applied to the floor and the slow excess.

#### Sensitivity: how much of a finished show's value is the far tail

Present value of the expected tail at the finale, in level-days (a 12-week season at a constant level ≈ 81), per
market, mult 1:

| equity premium | us: ≤ 400 days | us: beyond | cn: ≤ 400 days | cn: beyond | us beyond, yearly 0.975 | cn beyond, yearly 0.975 |
|---|---|---|---|---|---|---|
| 0.30 | 42.4 | 38.4 | 90.4 | 50.8 | 36.6 | 66.3 |
| 0.15 | 45.2 | 68.7 | 94.7 | 90.5 | 62.9 | 133.4 |
| 0.08 | 46.6 | 105.2 | 96.8 | 129.3 | 90.6 | 217.0 |

At the current 30% premium the tail is worth about as much as the season itself (it was a few level-weeks under the
×0.94/week curve), and half of it lies beyond the 400-day horizon. A lower premium (step 5) makes the flat floor
dominate: the floor's level and its decay beyond 8 years — which the data cannot pin down — then drive the value.


### Out-of-sample check of the us tail (phase 1 acceptance, `sim/tail_check.py`)

Held-out us tail (fit on even AniList ids, checked on odd; forecast at the finale):

| group | 3 m: n, log err, level ratio, 80% cov | 6 m: n, log err, level ratio, 80% cov | 12 m: n, log err, level ratio, 80% cov | 24 m: n, log err, level ratio, 80% cov | 36 m: n, log err, level ratio, 80% cov |
|---|---|---|---|---|---|
| all | 586, +0.04, 1.04, 90% | 562, -0.06, 0.97, 87% | 517, -0.10, 0.96, 81% | 414, -0.02, 1.06, 84% | 349, -0.09, 0.99, 80% |
| popularity 0.0–0.2 | 122, +0.12, 1.31, 80% | 118, -0.01, 1.25, 81% | 106, -0.01, 1.26, 76% | 76, +0.14, 1.70, 71% | 60, +0.22, 1.70, 75% |
| popularity 0.2–0.4 | 135, -0.02, 0.94, 88% | 133, -0.15, 0.80, 89% | 130, -0.19, 0.79, 80% | 109, -0.20, 0.73, 85% | 92, -0.34, 0.69, 79% |
| popularity 0.4–0.6 | 124, +0.01, 0.90, 95% | 122, -0.09, 0.83, 91% | 112, -0.18, 0.77, 84% | 94, -0.14, 0.81, 84% | 79, -0.18, 0.78, 72% |
| popularity 0.6–0.8 | 117, +0.05, 1.00, 94% | 113, -0.03, 0.93, 91% | 101, +0.03, 1.04, 83% | 76, +0.06, 0.94, 93% | 68, -0.11, 0.80, 91% |
| popularity 0.8–1.0 | 88, +0.05, 0.99, 92% | 76, +0.01, 1.01, 83% | 68, -0.13, 0.89, 82% | 59, +0.15, 1.15, 83% | 50, +0.15, 1.03, 84% |
| first season | 456, +0.06, 1.09, 89% | 442, -0.06, 0.98, 88% | 412, -0.12, 0.94, 82% | 333, -0.02, 1.09, 83% | 282, -0.10, 1.02, 80% |
| itself a sequel | 130, -0.02, 0.91, 91% | 120, -0.07, 0.94, 84% | 105, -0.05, 1.02, 77% | 81, -0.04, 0.93, 84% | 67, -0.04, 0.89, 81% |

Columns per horizon: shows, mean log error against the median forecast (realised − forecast; 0 if unbiased),
mean realised ÷ mean forecast factor (1 if unbiased in level, what the value needs), share inside the 80% interval.

Reading: over all held-out shows the tail is unbiased (level ratio 0.96–1.06, coverage 80–90%), for first seasons
and sequels alike, and for the top two popularity fifths (0.80–1.15), where most of the market's value is. It is
**not** unbiased within the lower fifths: the bottom fifth's tail is under-forecast (realised 1.25–1.70× the
forecast even with its prior flag) and the second and third fifths are over-forecast (0.69–0.94×, worst at 24–36
months). The prior's two flags capture the U-shape only in part; a prior by popularity fifth would be the next
step. Acceptance for step 1: met overall and for the top two fifths, not met for the lower three.


#### With the prior by popularity fifth (second round)

The prior now has one effect per popularity fifth (log scale, on each show's mean residual 3–36 months after the
finale; the middle fifth the reference; [+0.28, +0.04, 0, +0.06, +0.12]) and scales the whole post-transition curve
(the effect is there from the first months on). Held out (fit on even ids, checked on odd); each cell: n, mean log
error against the median forecast, level ratio ± its sampling error, 80% coverage:

| group | 3 m: n, log err, level ratio, 80% cov | 6 m: n, log err, level ratio, 80% cov | 12 m: n, log err, level ratio, 80% cov | 24 m: n, log err, level ratio, 80% cov | 36 m: n, log err, level ratio, 80% cov |
|---|---|---|---|---|---|
| all | 586, +0.10, 1.11±0.05, 89% | 562, +0.00, 1.04±0.06, 89% | 517, -0.03, 1.04±0.06, 81% | 414, +0.05, 1.15±0.10, 86% | 349, -0.02, 1.08±0.10, 81% |
| popularity 0.0–0.2 | 122, +0.19, 1.40±0.16, 80% | 118, +0.09, 1.37±0.19, 82% | 106, +0.08, 1.39±0.18, 76% | 76, +0.23, 1.87±0.40, 76% | 60, +0.31, 1.86±0.41, 73% |
| popularity 0.2–0.4 | 135, +0.05, 1.01±0.07, 86% | 133, -0.07, 0.88±0.06, 89% | 130, -0.11, 0.86±0.06, 80% | 109, -0.10, 0.81±0.06, 86% | 92, -0.23, 0.77±0.09, 82% |
| popularity 0.4–0.6 | 124, +0.07, 0.96±0.05, 94% | 122, -0.03, 0.88±0.06, 91% | 112, -0.12, 0.83±0.06, 85% | 94, -0.07, 0.88±0.11, 89% | 79, -0.12, 0.85±0.09, 76% |
| popularity 0.6–0.8 | 117, +0.09, 1.04±0.10, 94% | 113, +0.00, 0.96±0.08, 92% | 101, +0.06, 1.08±0.12, 82% | 76, +0.10, 0.98±0.08, 92% | 68, -0.07, 0.84±0.06, 93% |
| popularity 0.8–1.0 | 88, +0.11, 1.06±0.10, 92% | 76, +0.07, 1.09±0.12, 89% | 68, -0.07, 0.96±0.14, 85% | 59, +0.20, 1.22±0.17, 85% | 50, +0.20, 1.10±0.12, 82% |
| first season | 456, +0.11, 1.15±0.07, 88% | 442, -0.00, 1.04±0.07, 89% | 412, -0.06, 1.00±0.06, 82% | 333, +0.04, 1.17±0.12, 85% | 282, -0.04, 1.09±0.12, 80% |
| itself a sequel | 130, +0.07, 0.99±0.06, 92% | 120, +0.03, 1.06±0.09, 87% | 105, +0.06, 1.15±0.14, 79% | 81, +0.08, 1.06±0.13, 89% | 67, +0.08, 1.01±0.11, 84% |

Against the acceptance (level ratio within ±0.15 of 1 at 6/12/24 months):

- **all, first seasons, sequels, the 0.6–0.8 and 0.8–1.0 fifths:** met (1.04/1.04/1.15 overall; the top fifth's
  1.22 at 24 months is 1.3 sampling errors from 1.15).
- **0.2–0.4 and 0.4–0.6:** 0.81–0.88, i.e. just outside at 12–24 months (each ±0.06, so the miss is about
  1–3 sampling errors); the log error there is −0.07 to −0.12.
- **0.0–0.2: not met (1.37–1.87) and not fixable by a prior here — survivorship.** The history covers shows with
  AniList popularity ≥ 3,000 *today*. 83% of the bottom fifth had fewer than 3,000 list entries at their finale
  and are in the sample only because they grew afterwards (median ×4.8 since the finale, against ×2.7–3.4 for the
  other fifths): they are selected on a strong tail. A level-unbiased fit per fifth (ratio of sums instead of log
  means) chased these few large tails and made every fifth worse out of sample (overall 0.83–0.90), so it was not
  kept. The game lists only shows with ≥ 3,000 entries at listing, so this fifth is mostly outside the market.

## us episode week, day by day (v0.6.1, `--partial-only`)

The days so far of a us episode week are observed every day from the second day as the GLS estimate of the
complete week's average (engine/valuation.py `partial_observation`, docs/v2-valuation.md §3.8). Fitted on the
complete aligned episode weeks of the standard inputs (not the finale's), through the close's own day-by-day filter
(`close_days`): d_j = ybar_j - ybar_7, j = 1..6, mean and covariance per segment, the variance scaled per n by
maximum likelihood out of fold (fit on even AniList ids, score the complete week's observation on odd, and back):
1.62, 1.38, 1.47, 1.34, 1.34 for n = 2..6.

| segment | weeks | mean of d_1..d_6 | variance of the GLS estimate, n = 1..6 (scaled) |
|---|---|---|---|
| premiere week | 95 | −0.114 −0.310 −0.194 −0.070 −0.027 −0.015 | 0.764 0.507 0.231 0.051 0.019 0.008 |
| later, us level < 6.61 | 235 | +0.463 +0.180 +0.087 +0.040 +0.016 +0.005 | 0.668 0.033 0.014 0.008 0.0025 0.0008 |
| later, 6.61–7.74 | 236 | +0.188 +0.082 +0.040 +0.023 +0.011 +0.005 | 0.556 0.078 0.017 0.010 0.0017 0.0004 |
| later, ≥ 7.74 | 236 | +0.297 +0.093 +0.022 +0.005 +0.001 +0.001 | 0.495 0.027 0.010 0.004 0.0013 0.0004 |

(n = 1, the air day alone, is not observed: `min_days` 2.) The premiere segment's mean differs between the summer
and autumn premieres on day 1 and days 3–4 (summer −0.33 / −0.15, autumn −0.10 / −0.01 for d_3 / d_4; day 2 −0.31
in both); its variance is correspondingly large.

Held out (fit on even ids, tested on odd), 80% coverage of the complete week's observation given the base and the
first n days: n = 0 (the base alone) 89.9%, n = 2..6 85.5%, 79.0%, 79.0%, 77.4%, 84.3% (unscaled: 78.3%, 72.8%,
70.8%, 71.8%, 79.0%); mean log predictive density 0.03 → 0.42, 0.77, 1.10, 1.65, 2.31. In sample 88.3% → 90.0%,
86.2%, 88.9%, 85.9%, 88.7%.

Forecast check (held out): the slope of the next 7 days' revision of the us level (less the drift) on the days-so-far
surprise (partial observation − base mean) is 0.626 (se 0.008) without the partial observation and 0.035 (0.010) with
it; after a surprise above +25% the revision is +54% without, +8.2% with (196 days), below −12.5% −20% / −2.9% (494).
For reference, the base filter's own revision over the 7 days after a complete-week observation has slope 0.044 on
that observation's innovation.

## Joint prior: pairwise correlations (v0.6.1, `--prior-only`)

The prior covariance of the three markets' log levels at the premiere had one common correlation (rho 0.534). With
the dynamics (drift, q, r) fixed, bias, p0, the cn/jp elasticities and three pairwise correlations [us-cn, us-jp,
cn-jp] were refitted by maximum likelihood of the joint filter (115 shows premiering inside the standard inputs with a
point-in-time prior; docs/v2-valuation.md §3.9).

| | common rho | pairwise |
|---|---|---|
| correlations us-cn / us-jp / cn-jp | 0.534 | 0.319 / 0.398 / 0.815 |
| p0 us / cn / jp | 0.426 / 0.910 / 0.295 | 0.385 / 0.923 / 0.322 |
| log-likelihood, in sample | −916.4 | −898.6 |
| held out: fit even → odd / odd → even | −211.1 / −743.7 | −201.2 / −735.7 |
| out of season: autumn → summer / summer → autumn | −123.5 / −904.9 | −110.0 / −913.3 |
| 80% coverage in sample us / cn / jp | 87.0% / 84.1% / 98.2% | 88.2% / 84.0% / 97.5% |
| 80% coverage held out, us (even → odd / odd → even) | 90.0% / 82.6% | 90.0% / 84.1% |

Moments of the premiere surprises against the prior (first cn/jp value, first complete us week): variances us 0.41,
cn 0.98, jp 0.34 (the common-rho model: 0.47, 0.93, 0.35); covariances us-cn 0.18, us-jp 0.10, cn-jp 0.39 (model
0.33, 0.19, 0.28), i.e. correlations 0.28 / 0.26 / 0.67. Half-sample fits are noisy: even ids 0.17 / 0.24 / 0.76, odd
0.53 / 0.69 / 0.85; the summer premieres alone give 0.46 / 0.59 / 0.91.

Transmission check (`close_days` with the partial us week): on the day of a show's first cn/jp data, T = the move of the
us level estimate caused by them; R_k = the us estimate's revision over the next k days less the drift. Slope of R_k on
T (se), 114 shows:

| | R_1 | R_3 | R_7 | R_14 |
|---|---|---|---|---|
| common rho, in sample | −0.18 (0.08) | −0.27 (0.11) | −0.33 (0.12) | −0.35 (0.14) |
| pairwise, in sample | −0.11 (0.15) | +0.03 (0.20) | +0.06 (0.22) | +0.09 (0.25) |
| common rho, fitted on the other half | −0.19 (0.07) | −0.31 (0.10) | −0.40 (0.11) | −0.43 (0.13) |
| pairwise, fitted on the other half | −0.15 (0.11) | −0.23 (0.16) | −0.28 (0.17) | −0.33 (0.20) |

The standard deviation of T falls from 0.42 to 0.26 (out of fold). The first value of a cn/jp episode is an incomplete
count revised over the next 1–2 days (settled − first: mean +0.035 / +0.052, variance 0.060 / 0.044 for cn / jp), small
against the prior variance; not modelled.

The partial us week refitted on this filter: cuts 6.607 / 7.736, variance scale 1.64, 1.40, 1.48, 1.36, 1.34 (n = 2..6);
held out, 80% coverage of the complete week's observation 90.8% (base) → 85.8%, 79.3%, 78.6%, 77.1%, 84.3%; forecast
slope 0.627 (0.008) without, 0.027 (0.010) with the days so far.

## us drift by heat class (v0.6.1, `--heat-only --history`)

The heat class: the tercile of the close's us log level estimate 4 weeks after the premiere over the calibration
shows (95 with that point inside the data): cuts 6.423 / 7.558. Offsets on the us daily drift from week 4 on:

| | bottom | middle | top |
|---|---|---|---|
| history (32 seasons 2018-2025, 1,297 series): weekly drift relative to the season mean, weeks 4..finale−4 (season-clustered se) | −0.0040 (0.0025) | −0.0032 (0.0043) | +0.0072 (0.0026) |
| even years / odd years | −0.0089 / +0.0009 | +0.0011 / −0.0075 | +0.0078 / +0.0065 |
| offset used, per day | −0.00057 | −0.00045 | +0.00102 |
| calibration ML (in sample) | −0.00083 | +0.00118 | +0.00070 |

Calibration log-likelihood of the 381 complete us weeks after the class is set: none 98.81, history offsets 98.82,
calibration ML 99.06 (held out, fit even → odd 69.57 → 68.46, odd → even 29.24 → 27.78; worse): the two seasons cannot
estimate the offsets, the history can (top − bottom +0.011/week, t 4.2), and the history offsets do not contradict the
calibration (even 29.24 → 28.89, odd 69.57 → 69.93). partial_us refitted on top: cuts 6.605 / 7.736, held-out forecast
slope 0.027 (0.010).

## Season start: incumbents and the new cohort (v0.6.1 check)

Standard replay (seed 1, hidden value, dividends included, split-adjusted): the cap-weighted 14-day total return of the
stocks listed before 8/15 from 9/20, 9/27, 10/1, 10/4, 10/8, 10/15: +6.0, −1.0, +0.9, −0.4, +0.5, +1.3% (every 14-day
window of the replay: mean +1.5%, sd 4.5): no dilution of the incumbents at the autumn season start. The new shows'
point-in-time priors against their realised us attention, level-weighted (sum of realised weekly mean / sum of the
prior's expected level): premiere week 0.91 (summer) / 1.04 (autumn), weeks 1–3 0.74–0.84; per-show means 1.16–1.19 and
0.85–0.95. The priors are not low in aggregate; if anything big new shows fall short of them after the premiere week.

## Run-up before the finale (v0.6.1, `--runup-only --history`)

Weekly attention aligned on the last episode's air day (week 0 = the finale's week), log relative to the level of weeks
−6..−4 less that level's drift (weeks −7 → −4):

| | week −3 | week −2 | week −1 | week 0 |
|---|---|---|---|---|
| us history, 32 seasons, mean (season-clustered se) | +0.004 (0.007) | +0.011 (0.031) | +0.076 (0.052) | +0.404 (0.068) |
| us history, season median | +0.012 | +0.056 | +0.119 | +0.492 |
| us history, odd years / even years (mean) | +0.002 / +0.006 | +0.023 / −0.001 | +0.092 / +0.061 | +0.474 / +0.333 |
| us calibration (47 shows) | +0.070 (0.033) | +0.096 (0.052) | +0.225 (0.078) | +0.598 (0.096) |
| cn calibration (41) | −0.132 (0.063) | −0.154 (0.078) | −0.174 (0.107) | −0.122 (0.105) |
| jp calibration (34) | +0.009 (0.006) | +0.024 (0.008) | +0.045 (0.016) | +0.054 (0.018) |

The history's mean is pulled down by 2018Q2 (week −1: −1.36); the median is used. By heat tercile (history), week −1:
+0.061 / +0.097 / +0.074. `runup`: us [1.127], jp [1.046, 1.024], cn none (flat over weeks −3..0: the reference sits
high). The finale bump is rebased on the run-up of weeks −2/−3 it was fitted against: jp 0.967 → 0.978. Calibration
us complete-week log-likelihood −73.39 → −73.11; mean innovation in week −1 +0.062 (0.029) → −0.055 (0.029). Heat
offsets and partial_us refitted on top (unchanged to the printed precision; held-out forecast slope 0.027).
