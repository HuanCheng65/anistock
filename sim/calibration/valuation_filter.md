# Valuation filter fit

Data: the summer 2026 season, daily inputs 2026-07-01 to 2026-10-06 from engine/daily_inputs.py (140 shows).

us within-episode curve (day 0..6 after airing, mean 1): [0.6004, 2.5475, 1.4329, 0.8589, 0.617, 0.5001, 0.4432]

Plain Kalman filter (`--gaussian`, no outlier component): the outlier mixture was also fitted and gave no better held-out coverage, so the simpler model is used.

## us

shows 65, observations 818; drift -0.00957/day (×0.935/week), q 0.00377/day, r 0.0442, p0 0.283, prior bias +0.020, outliers 0.0% with extra variance 1.042; log-likelihood -234.8.

80% interval coverage: 81.9% in sample, 82.6% on held-out shows (453 observations; fit on even ids, tested on odd).

Prior bias used by the server: +0.086 (maximum likelihood +0.020 is unbiased in log; this one makes the value's first revision after the premiere zero on average).

## cn

shows 80, observations 1287; drift -0.01634/day (×0.892/week), q 0.01666/day, r 0.0159, p0 0.255, prior bias +0.126, outliers 0.0% with extra variance 4.010; log-likelihood -342.9.

80% interval coverage: 84.8% in sample, 83.0% on held-out shows (712 observations; fit on even ids, tested on odd).

Prior bias used by the server: +0.179 (maximum likelihood +0.126 is unbiased in log; this one makes the value's first revision after the premiere zero on average).

## jp

shows 72, observations 790; drift -0.01684/day (×0.889/week), q 0.01240/day, r 0.0044, p0 0.024, prior bias +0.130, outliers 0.0% with extra variance 2.482; log-likelihood -22.7.

80% interval coverage: 89.7% in sample, 89.5% on held-out shows (399 observations; fit on even ids, tested on odd).

Prior bias used by the server: +0.222 (maximum likelihood +0.130 is unbiased in log; this one makes the value's first revision after the premiere zero on average).

## 热度快讯 hourly curve

131 episodes. Share of 28-day comments by hour 1..24: [0.0, 0.0741, 0.1304, 0.1719, 0.2105, 0.2273, 0.2333, 0.2394, 0.25, 0.2667, 0.2805, 0.2982, 0.3235, 0.3519, 0.3824, 0.4167, 0.4316, 0.4507, 0.4737, 0.4868, 0.5116, 0.5238, 0.5439, 0.5714]

Noise variance of log(count_h / (curve_h × previous mean)) by hour: [1.0, 1.1935, 0.5586, 0.4651, 0.5375, 0.644, 0.6575, 0.6307, 0.6336, 0.6304, 0.5613, 0.5459, 0.5037, 0.5414, 0.4981, 0.4994, 0.4919, 0.6027, 0.5804, 0.5331, 0.5367, 0.5511, 0.5464, 0.5543]
