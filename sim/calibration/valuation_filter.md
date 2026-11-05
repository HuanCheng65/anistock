# Valuation filter fit

Data: daily inputs 2026-07-01 to 2026-11-03 from engine/daily_inputs.py (v0.3.2 schema; market series rebuilt with the air-time fix in sim/reconstruct.py; 142 shows), i.e. the summer 2026 season and the first five weeks of autumn. Observations from a week after a show\'s finale are left out (the server reads them through the post-finale profile).

us within-episode curve (day 0..6 after airing, mean 1): [0.5952, 2.4934, 1.4687, 0.8811, 0.6225, 0.4996, 0.4395]

Joint filter (v0.3.2): one state per show, the three markets' log levels. Prior at the premiere: log(point-in-time us pre-air estimate) + bias_m, variances p0_m, common correlation rho = 0.534 (summer fit 0.646, autumn fit 0.539). 115 shows with a prior (53 premiering before 2026-09-15, 62 after). Two stages: each market's drift, q, r from its own filter; then bias, p0 and rho by maximum likelihood of the joint filter.

Plain Kalman filter (`--gaussian`, no outlier component): the outlier mixture was also fitted in v0.3.0 and gave no better held-out coverage, so the simpler model is used.

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

Noise variance of log(count_h / (curve_h × previous mean)) by hour: [1.0, 1.1935, 0.5586, 0.4651, 0.5375, 0.644, 0.6575, 0.6307, 0.6336, 0.6304, 0.5613, 0.5459, 0.5037, 0.5414, 0.4981, 0.4994, 0.4919, 0.6027, 0.5804, 0.5331, 0.5367, 0.5511, 0.5464, 0.5543]

## IPO discount

ipo_risk = 0: over the 61 shows listed in August-October of the v0.3.2 replay, the value 28 days after listing plus dividends relative to the model's value when the offer was priced is 0.935 value-weighted (1.158 equal-weighted, driven by three hits; 1.024 without them). The model's value already prices the uncertainty; a further discount was a riskless gain for subscribers (ipo_all +9.7 points over cap-weighted buy-and-hold with ipo_risk 0.25).
