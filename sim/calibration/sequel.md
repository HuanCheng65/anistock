# Sequel option fit (docs/design/series.md §5)

Data: /home/starrydream/.cache/claude-tmp/claude-1007/-home-starrydream-anisotck/ae90f005-b0a1-4ebf-b50e-662abee6e48f/scratchpad/hist_final.db; 1688 finished TV/ONA shows, 6426 show-periods.

Discrete-time logistic hazard: period 0 = the finale ± 7 days, periods 1–3 = years 1–3 after it, period 4 = each later year.

| period | α | P for a first season, score 70, other source, not top decile |
|---|---|---|
| 0 | -1.921 | 12.8% |
| 1 | -2.104 | 10.9% |
| 2 | -3.117 | 4.2% |
| 3 | -3.649 | 2.5% |
| 4 | -4.157 | 1.5% |

| predictor | β | se | odds × |
|---|---|---|---|
| score/10 | +0.652 | 0.066 | 1.92 |
| light novel | +0.213 | 0.120 | 1.24 |
| original | -0.272 | 0.147 | 0.76 |
| itself a sequel | +0.450 | 0.103 | 1.57 |
| top decile popularity | +0.724 | 0.139 | 2.06 |

Calibration, announced within 3 years (predicted vs actual by fifth of the predicted probability; in sample | fit on even ids, tested on odd):

| fifth | predicted | actual | n | predicted (held out) | actual (held out) | n |
|---|---|---|---|---|---|---|
| 1 | 11.9% | 10.4% | 231 | 13.4% | 9.4% | 117 |
| 2 | 20.4% | 22.0% | 232 | 22.5% | 16.9% | 118 |
| 3 | 28.1% | 31.5% | 232 | 30.6% | 33.3% | 117 |
| 4 | 38.8% | 36.6% | 232 | 41.3% | 31.4% | 118 |
| 5 | 63.1% | 67.7% | 232 | 64.0% | 61.9% | 118 |

Announcement → premiere (TV/ONA, 453 sequels), decile midpoints: [19, 102, 174, 204, 283, 344, 398, 522, 650, 1013] days.

Sequel in-season us attention / prequel's (deflated), by gap from the prequel's finale:

| gap up to (days) | n | geometric mean | log sd | expected ratio |
|---|---|---|---|---|
| 365 | 148 | 0.5984 | 0.6684 | 0.748143 |
| 730 | 100 | 0.658 | 0.7 | 0.840764 |
| 1460 | 68 | 0.5142 | 0.9635 | 0.817899 |
| 100000 | 15 | 0.2834 | 0.456 | 0.314446 |

Finale week relative to the in-season mean (where a sequel season's tail starts): {'us': 1.239328, 'cn': 0.929368, 'jp': 1.239328}.


Announcement transient on the prequel's us attention (sim/calibration/long_tail_sequel.md, 306 announcements): ×1.286 in the announcement week, ×1.058 in the two weeks after, then none. The filter reads those weeks' observations through it.
