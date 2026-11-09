# Long tail and sequel option: what the history says

Data: `sim/history_backfill.py` (AniList TV/ONA 2018–2025 with popularity ≥ 3,000 and their relations; daily trends in
season and in 14-day windows 1–96 months after the finale; the first trend day of every sequel entry as the
announcement proxy; a Bangumi sample of ~380 subjects with episode and subject comments). Analysis:
`sim/long_tail_analysis.py HISTORY_DB` prints everything below. Design: `docs/design/series.md`.

## Summary

- **us tail (platform-index deflated, no-sequel median, relative to the in-season mean):** 1 month 0.166, 3 months
  0.122, 6 months 0.094, 12 months 0.074, 24 months 0.056, 36 months 0.041, 60 months 0.026, then flat (×0.975/yr).
  Dropping the 2020-04..2021-06 windows changes little. The game's current post-finale curve (×0.94/week) is 12× too
  low at 12 months. The per-registered-user deflator double-counts platform growth (registered users grew far faster
  than activity) and is not used; the platform index (median in-season attention of the quarter's shows) is the
  deflator that matches a share-of-platform revenue rule.
- **Tail level predictors** (OLS on log level at 24–36 months, R² 0.45, residual sd 0.76): score ×1.57 per 10 points,
  ONA ×2.0, Romance ×1.38, sequel aired ×1.37; popularity percentile ×0.36 (relative tails of less popular shows sit
  higher). `year-2020` is a linear year trend (year − 2020), not a lockdown dummy.
- **cn:** episode comments stop (median 0 from month 2). Subject (collection) comments: 0.36 at 1 month, 0.15 at 6,
  ~0.10 from 12 months on, flat for years. The cn tail uses a collection metric.
- **Sequel probability:** within 3 years of the finale, first seasons 26%, sequels 51%. Logistic (3-year
  announcement): score ×2.38 per 10 points, itself a sequel ×1.87, light novel ×1.56, original ×0.67; popularity adds
  nothing given score. "Manga source still running" looks negative at 3 years but is flat at 5–6 years (censoring):
  not used.
- **Announcement timing:** 48% of announcements fall on or before finale + 7 days. After the finale, yearly hazard
  13.7%, 3.6%, 2.0%, then ~1%. Announcement → air: TV median 292 days (IQR 140–501), film 382.
- **Sequel attention vs prequel** (in-season, deflated): median 0.59; gap < 1 y 0.64, 1–2 y 0.68, 2–4 y 0.43, 4 y+ 0.30
  (conditional on a sequel being made, which is what the option needs).
- **Announcement effect on the prequel's us attention** (306 announcements ≥ 30 days after the finale): first week
  ×1.29 (median; 36% above ×1.5), back to ×1.06 in weeks 2–3 and ×0.98 after: a one-week spike, no lasting lift.

## Sources

| source | history | long tail | sequels |
|---|---|---|---|
| AniList `Page.mediaTrends` | daily trending/popularity/inProgress back to 2014–18, finished shows included (days without activity have no row) | yes, in the game's us metric (2·adds + trending) | SEQUEL relations with dates; the sequel entry's first trend day as the announcement date |
| Bangumi episode comments | every comment with its time, one request per episode | measurable but ~0 after the finale | no |
| Bangumi subject (collection) comments | `updatedAt`, deep paging | yes, proxy for new collections | no |
| Bangumi `/collects` | offset capped at 500 | current rate only | no |
| Annict | not fetched yet (token is server-only) | `records(orderBy CREATED_AT)` on first/last episode, 2–4 requests per work | no |

Bangumi sample: 411 of 550 planned subjects (stratified by start year; resumable with `--sample N`).

## Caveats

- **Platform growth:** raw ratios are inflated and rise again after 60 months; the platform index is noisy and is
  smoothed over 3 quarters.
- **Survivorship:** the scope is popularity ≥ 3,000 today, so older cohorts hold more tail survivors; the top-half
  check suggests a small effect.
- **Cumulative vs activity:** popularity is cumulative, so its daily difference is net list adds; inProgress is a
  stale stock and unused. In the tail most attention is adds (~0.2) rather than trending (~0.05).
- **Announcement date:** the first trend day can precede (placeholder entry) or follow the official announcement; the
  pre-window is flat, so no systematic lead.
- **Censoring:** every k-year rate uses only shows whose finale is at least k years old; the headline tail excludes
  shows with any sequel.
- **Bangumi proxy:** collection comments carry the last-edit time and come from a subset of collectors; Bangumi's own
  growth is not deflated.
- **Popularity and sequels is U-shaped** (top decile 59% within 3 years, middle 14–19%, bottom 43%), so a linear term
  comes out ~0; use a top-decile flag instead.

## Recommendation

1. us tail ≈ 0.025 + 0.14·e^(−t / 7 months) of the in-season mean (within ~10% of the medians from 1 to 96 months),
   scaled by score and ONA; replaces ×0.94/week.
2. cn tail from Bangumi collection-count differences (~0.10 of in-season after 12 months). With episode comments as the
   only cn metric the honest cn tail is ~0.
3. jp: us shape until a server-side Annict records backfill. (Done 2026-11-09: fitted on 1,378 shows' Annict records, `sim/fit_valuation.py fit_tail_jp`, docs/design/series.md §12.)
4. Sequel option from score, is-sequel, a top-decile popularity flag and light-novel source; mostly resolved at the
   finale, then a yearly hazard; sequel attention ≈ 0.6× the prequel; the announcement is a one-week event.
5. Not supported by the data: studio effects, source material remaining, a jp tail, anything before 2018.

## Full output

<!-- shows with us tail data: 1636 -->

### Platform index: median in-season us attention/day of in-scope shows, by quarter

| quarter | shows | median att/day | AniList users (k) |
|---|---|---|---|
| 2018Q1 | 45 | 51 | 149 |
| 2018Q2 | 58 | 77 | 166 |
| 2018Q3 | 47 | 178 | 186 |
| 2018Q4 | 50 | 189 | 209 |
| 2019Q1 | 43 | 98 | 234 |
| 2019Q2 | 34 | 242 | 261 |
| 2019Q3 | 42 | 381 | 299 |
| 2019Q4 | 53 | 242 | 337 |
| 2020Q1 | 43 | 244 | 372 |
| 2020Q2 | 44 | 165 | 456 |
| 2020Q3 | 30 | 377 | 595 |
| 2020Q4 | 48 | 477 | 756 |
| 2021Q1 | 59 | 382 | 951 |
| 2021Q2 | 58 | 1092 | 1175 |
| 2021Q3 | 44 | 1051 | 1338 |
| 2021Q4 | 53 | 404 | 1461 |
| 2022Q1 | 40 | 708 | 1569 |
| 2022Q2 | 54 | 503 | 1673 |
| 2022Q3 | 54 | 394 | 1784 |
| 2022Q4 | 54 | 647 | 1882 |
| 2023Q1 | 58 | 917 | 1982 |
| 2023Q2 | 54 | 1013 | 2088 |
| 2023Q3 | 56 | 930 | 2204 |
| 2023Q4 | 69 | 655 | 2300 |
| 2024Q1 | 52 | 1132 | 2422 |
| 2024Q2 | 58 | 935 | 2552 |
| 2024Q3 | 56 | 809 | 2677 |
| 2024Q4 | 60 | 679 | 2826 |
| 2025Q1 | 47 | 1159 | 2974 |
| 2025Q2 | 57 | 930 | 3118 |
| 2025Q3 | 65 | 822 | 3243 |
| 2025Q4 | 52 | 730 | 3362 |

### us: deflated tail (attention share relative to the in-season share; divides by the platform index)

| months after finale | n | p25 | median | p75 | median, no sequel | n | median, standalone | median, no sequel, top half of its year | n |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 1635 | 0.139 | 0.181 | 0.270 | 0.166 | 1024 | 0.161 | 0.166 | 452 |
| 2 | 1635 | 0.097 | 0.131 | 0.210 | 0.118 | 1024 | 0.114 | 0.113 | 452 |
| 3 | 1635 | 0.099 | 0.137 | 0.216 | 0.122 | 1024 | 0.119 | 0.119 | 452 |
| 4.5 | 1635 | 0.071 | 0.101 | 0.186 | 0.090 | 1024 | 0.087 | 0.086 | 452 |
| 6 | 1635 | 0.072 | 0.108 | 0.184 | 0.094 | 1024 | 0.092 | 0.092 | 452 |
| 9 | 1624 | 0.063 | 0.097 | 0.167 | 0.085 | 1014 | 0.082 | 0.085 | 448 |
| 12 | 1575 | 0.055 | 0.086 | 0.154 | 0.074 | 978 | 0.072 | 0.073 | 431 |
| 15 | 1507 | 0.050 | 0.079 | 0.141 | 0.068 | 928 | 0.066 | 0.067 | 412 |
| 18 | 1458 | 0.045 | 0.076 | 0.139 | 0.063 | 890 | 0.061 | 0.063 | 396 |
| 21 | 1395 | 0.044 | 0.074 | 0.134 | 0.062 | 848 | 0.059 | 0.059 | 376 |
| 24 | 1345 | 0.040 | 0.068 | 0.132 | 0.056 | 813 | 0.053 | 0.050 | 363 |
| 30 | 1234 | 0.034 | 0.060 | 0.118 | 0.048 | 744 | 0.044 | 0.045 | 337 |
| 36 | 1114 | 0.027 | 0.051 | 0.107 | 0.041 | 670 | 0.039 | 0.041 | 311 |
| 42 | 1002 | 0.024 | 0.044 | 0.089 | 0.034 | 597 | 0.032 | 0.035 | 274 |
| 48 | 893 | 0.024 | 0.043 | 0.079 | 0.032 | 532 | 0.030 | 0.031 | 242 |
| 60 | 690 | 0.020 | 0.036 | 0.074 | 0.026 | 395 | 0.025 | 0.026 | 183 |
| 72 | 479 | 0.018 | 0.033 | 0.067 | 0.026 | 279 | 0.025 | 0.025 | 130 |
| 84 | 309 | 0.014 | 0.034 | 0.079 | 0.021 | 180 | 0.020 | 0.021 | 83 |
| 96 | 138 | 0.016 | 0.036 | 0.073 | 0.025 | 82 | 0.024 | 0.032 | 33 |

### us: the same tail per registered user (second deflator; users grew ~13× 2019→2026, active users less)

| months | n (no sequel) | p25 | median | p75 |
|---|---|---|---|---|
| 1 | 1024 | 0.127 | 0.161 | 0.234 |
| 2 | 1024 | 0.084 | 0.111 | 0.174 |
| 3 | 1024 | 0.084 | 0.112 | 0.181 |
| 4.5 | 1024 | 0.061 | 0.083 | 0.150 |
| 6 | 1024 | 0.059 | 0.082 | 0.133 |
| 9 | 1014 | 0.050 | 0.069 | 0.109 |
| 12 | 978 | 0.040 | 0.058 | 0.096 |
| 15 | 928 | 0.035 | 0.051 | 0.086 |
| 18 | 890 | 0.030 | 0.045 | 0.079 |
| 21 | 848 | 0.027 | 0.042 | 0.075 |
| 24 | 813 | 0.024 | 0.037 | 0.068 |
| 30 | 744 | 0.018 | 0.032 | 0.057 |
| 36 | 670 | 0.014 | 0.025 | 0.047 |
| 42 | 597 | 0.011 | 0.020 | 0.040 |
| 48 | 532 | 0.009 | 0.017 | 0.033 |
| 60 | 395 | 0.007 | 0.012 | 0.023 |
| 72 | 279 | 0.005 | 0.010 | 0.018 |
| 84 | 180 | 0.003 | 0.007 | 0.019 |
| 96 | 82 | 0.005 | 0.009 | 0.021 |

### us: deflated tail (no sequel) by finale year — lifecycle shape or calendar effect?

| finale year | n | 1m | 3m | 6m | 12m | 18m | 24m | 36m | 48m | 60m | 84m |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 2018 | 106 | 0.197 | 0.192 | 0.151 | 0.075 | 0.094 | 0.126 | 0.039 | 0.045 | 0.020 | 0.020 |
| 2019 | 96 | 0.183 | 0.155 | 0.141 | 0.158 | 0.092 | 0.056 | 0.049 | 0.025 | 0.026 | 0.029 |
| 2020 | 100 | 0.215 | 0.175 | 0.117 | 0.059 | 0.045 | 0.049 | 0.021 | 0.022 | 0.025 | – |
| 2021 | 121 | 0.170 | 0.120 | 0.105 | 0.090 | 0.054 | 0.036 | 0.032 | 0.035 | 0.044 | – |
| 2022 | 139 | 0.150 | 0.108 | 0.069 | 0.041 | 0.032 | 0.033 | 0.036 | 0.040 | – | – |
| 2023 | 149 | 0.155 | 0.106 | 0.072 | 0.067 | 0.064 | 0.064 | 0.081 | – | – | – |
| 2024 | 136 | 0.149 | 0.111 | 0.091 | 0.060 | 0.071 | 0.077 | – | – | – | – |
| 2025 | 167 | 0.168 | 0.108 | 0.093 | 0.083 | 0.078 | – | – | – | – | – |
| 2026 | 11 | 0.243 | 0.214 | 0.104 | – | – | – | – | – | – | – |

Per registered user:

| finale year | n | 1m | 3m | 6m | 12m | 18m | 24m | 36m | 48m | 60m | 84m |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 2018 | 106 | 0.213 | 0.210 | 0.177 | 0.097 | 0.096 | 0.105 | 0.036 | 0.020 | 0.012 | 0.008 |
| 2019 | 96 | 0.174 | 0.141 | 0.120 | 0.105 | 0.061 | 0.034 | 0.020 | 0.011 | 0.010 | 0.006 |
| 2020 | 100 | 0.218 | 0.150 | 0.107 | 0.059 | 0.032 | 0.027 | 0.016 | 0.012 | 0.012 | – |
| 2021 | 121 | 0.149 | 0.097 | 0.067 | 0.045 | 0.034 | 0.027 | 0.019 | 0.016 | 0.013 | – |
| 2022 | 139 | 0.151 | 0.110 | 0.078 | 0.051 | 0.038 | 0.033 | 0.031 | 0.024 | – | – |
| 2023 | 149 | 0.148 | 0.099 | 0.066 | 0.055 | 0.047 | 0.041 | 0.038 | – | – | – |
| 2024 | 136 | 0.142 | 0.102 | 0.080 | 0.049 | 0.046 | 0.043 | – | – | – | – |
| 2025 | 167 | 0.154 | 0.095 | 0.072 | 0.056 | 0.046 | – | – | – | – | – |
| 2026 | 11 | 0.216 | 0.180 | 0.080 | – | – | – | – | – | – | – |

### us: implied weekly multiplier of the deflated tail (medians, no sequel) vs the game's ×0.94/week

| segment | × per week (platform index) | × per week (per user) |
|---|---|---|
| 1→3 months | 0.9647 | 0.9593 |
| 3→6 months | 0.9804 | 0.9763 |
| 6→12 months | 0.9910 | 0.9865 |
| 12→24 months | 0.9944 | 0.9914 |
| 12→36 months | 0.9943 | 0.9919 |
| 36→60 months | 0.9958 | 0.9929 |
| 36→96 months | 0.9982 | 0.9963 |
| 60→96 months | 0.9998 | 0.9985 |

### us: attention after the finale relative to the in-season mean (2·adds + trending)

| months after finale | n | p25 | median | p75 | median, no sequel ever | n no-seq | adds only | trending only |
|---|---|---|---|---|---|---|---|---|
| 1 | 1635 | 0.144 | 0.189 | 0.296 | 0.172 | 1024 | 0.297 | 0.141 |
| 2 | 1635 | 0.100 | 0.138 | 0.238 | 0.124 | 1024 | 0.242 | 0.092 |
| 3 | 1635 | 0.102 | 0.145 | 0.246 | 0.127 | 1024 | 0.273 | 0.092 |
| 4.5 | 1635 | 0.076 | 0.117 | 0.214 | 0.100 | 1024 | 0.223 | 0.066 |
| 6 | 1635 | 0.078 | 0.119 | 0.212 | 0.102 | 1024 | 0.242 | 0.065 |
| 9 | 1624 | 0.072 | 0.107 | 0.208 | 0.090 | 1014 | 0.231 | 0.056 |
| 12 | 1575 | 0.062 | 0.097 | 0.205 | 0.081 | 978 | 0.213 | 0.050 |
| 15 | 1507 | 0.059 | 0.096 | 0.207 | 0.079 | 928 | 0.208 | 0.049 |
| 18 | 1458 | 0.057 | 0.093 | 0.217 | 0.080 | 890 | 0.201 | 0.045 |
| 21 | 1395 | 0.058 | 0.093 | 0.223 | 0.079 | 848 | 0.210 | 0.046 |
| 24 | 1345 | 0.054 | 0.095 | 0.241 | 0.077 | 813 | 0.211 | 0.044 |
| 30 | 1234 | 0.053 | 0.093 | 0.241 | 0.076 | 744 | 0.206 | 0.043 |
| 36 | 1114 | 0.052 | 0.092 | 0.237 | 0.073 | 670 | 0.198 | 0.043 |
| 42 | 1002 | 0.049 | 0.092 | 0.243 | 0.074 | 597 | 0.196 | 0.042 |
| 48 | 893 | 0.050 | 0.096 | 0.239 | 0.074 | 532 | 0.199 | 0.042 |
| 60 | 690 | 0.057 | 0.115 | 0.265 | 0.086 | 395 | 0.212 | 0.053 |
| 72 | 479 | 0.073 | 0.158 | 0.352 | 0.112 | 279 | 0.283 | 0.066 |
| 84 | 309 | 0.078 | 0.185 | 0.455 | 0.111 | 180 | 0.297 | 0.086 |
| 96 | 138 | 0.125 | 0.260 | 0.559 | 0.216 | 82 | 0.379 | 0.131 |

### us: measured vs the game's post-finale curve (relative to the last 4 in-season weeks)

| months | n (no sequel) | measured median | game curve | measured / curve |
|---|---|---|---|---|
| 1 | 1024 | 0.220 | 0.1799 | 1.2 |
| 2 | 1024 | 0.152 | 0.1350 | 1.1 |
| 3 | 1024 | 0.161 | 0.0990 | 1.6 |
| 4.5 | 1024 | 0.124 | 0.0682 | 1.8 |
| 6 | 1024 | 0.126 | 0.0442 | 2.8 |
| 9 | 1014 | 0.113 | 0.0197 | 5.7 |
| 12 | 978 | 0.103 | 0.0088 | 11.6 |

### us: tail decay per year, no-sequel shows (log-slope of deflated attention, months 12 → last window ≥ 36)

| segment | n | median log-slope /yr | × per year | half-life (years) |
|---|---|---|---|---|
| all | 670 | -0.196 | 0.822 | 3.5 |
| 12-24 | 595 | -0.338 | 0.713 | 2.1 |
| 36+ | 595 | -0.025 | 0.975 | 27.2 |

### us: deflated tail level (mean of months 24–36) by popularity percentile within the finale year, no-sequel shows

| popularity at finale (quintile) | n | p25 | median | p75 | in-season att/day (median) | tail att/day (median) |
|---|---|---|---|---|---|---|
| 0.00–0.21 | 134 | 0.046 | 0.099 | 0.232 | 40.713 | 3.5 |
| 0.21–0.38 | 134 | 0.026 | 0.039 | 0.071 | 140 | 5.2 |
| 0.38–0.55 | 134 | 0.023 | 0.035 | 0.059 | 312 | 10.1 |
| 0.55–0.72 | 134 | 0.031 | 0.044 | 0.059 | 917 | 32.7 |
| 0.72–0.99 | 134 | 0.033 | 0.050 | 0.092 | 1521 | 70.3 |

### us: what predicts the tail level (OLS on log deflated ratio, months 24–36; all shows incl. sequels)

| predictor | coef | se | × effect |
|---|---|---|---|
| const | -5.535 | 0.196 | 0.00 |
| pop percentile in finale year | -1.035 | 0.115 | 0.36 |
| score/10 | 0.452 | 0.031 | 1.57 |
| light novel | 0.197 | 0.074 | 1.22 |
| original | -0.148 | 0.069 | 0.86 |
| game/VN/other | -0.085 | 0.066 | 0.92 |
| ONA | 0.697 | 0.067 | 2.01 |
| 2+ cours | 0.185 | 0.070 | 1.20 |
| sequel aired by month 36 | 0.317 | 0.053 | 1.37 |
| Comedy | 0.036 | 0.051 | 1.04 |
| Romance | 0.322 | 0.060 | 1.38 |
| Action | 0.202 | 0.056 | 1.22 |
| Slice of Life | -0.200 | 0.062 | 0.82 |
| Fantasy | -0.004 | 0.055 | 1.00 |
| year-2020 | -0.152 | 0.014 | 0.86 |
| is itself a sequel | -0.101 | 0.055 | 0.90 |

n = 1114, R² = 0.45, residual sd (log) = 0.76

### Sequel rates after the finale (TV/ONA shows; any SEQUEL relation)

Columns: share of shows with the event within 1/2/3 years of the finale (cohort size: shows whose finale is at least that long ago).

| event | n | ≤1y | ≤2y | ≤3y |
|---|---|---|---|---|
| announced (any TV/ONA/film sequel) | 1688 | 28% (1627) | 32% (1394) | 34% (1161) |
| aired: any TV/ONA/film sequel | 1688 | 14% (1627) | 24% (1394) | 30% (1161) |
| aired: TV/ONA sequel | 1688 | 13% (1627) | 22% (1394) | 27% (1161) |
| aired: film sequel | 1688 | 1% (1627) | 2% (1394) | 2% (1161) |

#### First seasons vs shows that are themselves sequels (announcement)

|  | n | ≤1y | ≤2y | ≤3y |
|---|---|---|---|---|
| first season (no PREQUEL) | 1189 | 22% (1144) | 26% (984) | 26% (812) |
| itself a sequel | 499 | 42% (483) | 48% (410) | 51% (349) |

#### By popularity decile within the finale year (announcement of any TV/ONA/film sequel; first seasons only)

| popularity | n | ≤1y | ≤2y | ≤3y |
|---|---|---|---|---|
| D1 | 123 | 28% (120) | 35% (103) | 43% (87) |
| D2 | 109 | 19% (106) | 24% (88) | 25% (68) |
| D3 | 118 | 19% (112) | 19% (95) | 18% (83) |
| D4 | 112 | 14% (106) | 16% (93) | 17% (77) |
| D5 | 131 | 12% (126) | 16% (108) | 18% (83) |
| D6 | 127 | 13% (119) | 16% (104) | 14% (87) |
| D7 | 135 | 17% (130) | 21% (111) | 19% (94) |
| D8 | 134 | 25% (130) | 26% (108) | 27% (91) |
| D9 | 118 | 29% (114) | 33% (103) | 31% (83) |
| D10 | 82 | 54% (81) | 61% (71) | 59% (59) |

#### By score (first seasons)

| AniList score | n | ≤1y | ≤2y | ≤3y |
|---|---|---|---|---|
| 0–64 | 433 | 12% (424) | 14% (374) | 14% (327) |
| 65–69 | 275 | 21% (256) | 25% (213) | 25% (170) |
| 70–74 | 267 | 23% (256) | 29% (220) | 33% (175) |
| 75–79 | 139 | 35% (135) | 42% (117) | 42% (89) |
| 80–100 | 75 | 52% (73) | 60% (60) | 57% (51) |

#### By source (first seasons)

| source | n | ≤1y | ≤2y | ≤3y |
|---|---|---|---|---|
| LIGHT_NOVEL | 232 | 23% (220) | 29% (182) | 31% (134) |
| MANGA | 531 | 24% (509) | 27% (429) | 26% (351) |
| ORIGINAL | 195 | 12% (189) | 17% (167) | 17% (145) |
| OTHER (web novel, novel, VN, …) | 145 | 27% (142) | 30% (127) | 33% (110) |
| VIDEO_GAME | 86 | 20% (84) | 23% (79) | 26% (72) |

#### Manga source still running at the anime finale vs already finished (first seasons)

| source state | n | ≤1y | ≤2y | ≤3y |
|---|---|---|---|---|
| manga source releasing/hiatus | 620 | 24% (595) | 28% (500) | 28% (408) |
| manga source finished | 211 | 20% (199) | 22% (169) | 21% (124) |
| no manga source relation | 358 | 21% (350) | 24% (315) | 26% (280) |

#### Studios with ≥ 15 shows (announcement within 3 years)

| studio | n (3y cohort) | announced ≤3y |
|---|---|---|
| A-1 Pictures | 23 | 57% |
| Bandai Namco Pictures | 16 | 56% |
| TMS Entertainment | 26 | 54% |
| bones | 18 | 50% |
| Production I.G | 26 | 46% |
| OLM | 24 | 46% |
| CloverWorks | 21 | 43% |
| Sunrise | 15 | 40% |
| J.C.STAFF | 51 | 37% |
| Studio DEEN | 22 | 36% |
| WIT STUDIO | 15 | 33% |
| SILVER LINK. | 23 | 30% |
| MADHOUSE | 20 | 25% |
| MAPPA | 28 | 25% |
| LIDENFILMS | 33 | 18% |
| Doga Kobo | 21 | 10% |

#### Logistic model: sequel (TV/ONA/film) announced within 3 years of the finale

| predictor | coef | se | odds × |
|---|---|---|---|
| const (median popularity, score 70, no manga relation, first season) | -0.589 | 0.151 | 0.55 |
| popularity percentile (0→1) | 0.034 | 0.275 | 1.03 |
| score (per 10) | 0.867 | 0.096 | 2.38 |
| light novel | 0.443 | 0.192 | 1.56 |
| original | -0.394 | 0.200 | 0.67 |
| manga source running at finale | -0.384 | 0.180 | 0.68 |
| manga source finished by finale | -0.531 | 0.224 | 0.59 |
| itself a sequel | 0.626 | 0.148 | 1.87 |

n = 1161, base rate = 34%

### Announcement timing and lag (TV/ONA/film sequels; first AniList trend day as the announcement proxy)

|  | n | p10 | p25 | median | p75 | p90 |
|---|---|---|---|---|---|---|
| announcement − original finale (days) | 583 | -31.800 | 0.000 | 9.000 | 287 | 738 |
| air − announcement, TV/ONA (days) | 472 | 28.000 | 132 | 290 | 505 | 735 |
| air − announcement, film (days) | 40 | 130 | 179 | 382 | 588 | 1002 |

Share of announcements on or before the finale day + 7: 49%

### Announcement effect on the original show's us attention (daily, around the sequel's first trend day A)

| window vs A−28..A−1 | n | p25 | median | p75 | share > 1.5× | adds median | trending median |
|---|---|---|---|---|---|---|---|
| pre (A−45..A−29) | 306 | 0.880 | 1.077 | 1.221 | 10% | 1.068 | 1.074 |
| A..A+6 | 306 | 1.043 | 1.286 | 1.767 | 36% | 1.306 | 1.294 |
| A+7..A+20 | 306 | 0.881 | 1.058 | 1.264 | 13% | 1.037 | 1.061 |
| A+21..A+45 | 306 | 0.819 | 0.980 | 1.213 | 13% | 0.977 | 0.989 |

### cn: Bangumi episode comments vs subject (collection) comments after the finale

| months | n ep | ep comments: median ratio | pooled ratio | share zero | n col | collection comments: median | p75 | us att median | us adds median |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 390 | 0.0272 | 0.0529 | 40% | 369 | 0.366 | 0.624 | 0.189 | 0.297 |
| 2 | 390 | 0.0000 | 0.0303 | 57% | 369 | 0.250 | 0.438 | 0.138 | 0.242 |
| 3 | 390 | 0.0000 | 0.0232 | 62% | 369 | 0.222 | 0.398 | 0.145 | 0.273 |
| 4.5 | 390 | 0.0000 | 0.0183 | 71% | 369 | 0.152 | 0.243 | 0.117 | 0.223 |
| 6 | 390 | 0.0000 | 0.0179 | 71% | 369 | 0.150 | 0.233 | 0.119 | 0.242 |
| 9 | 390 | 0.0000 | 0.0096 | 83% | 369 | 0.112 | 0.209 | 0.107 | 0.231 |
| 12 | 374 | 0.0000 | 0.0091 | 83% | 354 | 0.099 | 0.180 | 0.097 | 0.213 |
| 15 | 356 | 0.0000 | 0.0078 | 85% | 337 | 0.103 | 0.182 | 0.096 | 0.208 |
| 18 | 348 | 0.0000 | 0.0076 | 86% | 329 | 0.094 | 0.172 | 0.093 | 0.201 |
| 21 | 337 | 0.0000 | 0.0067 | 85% | 318 | 0.093 | 0.173 | 0.093 | 0.210 |
| 24 | 323 | 0.0000 | 0.0068 | 89% | 305 | 0.102 | 0.180 | 0.095 | 0.211 |
| 30 | 297 | 0.0000 | 0.0071 | 90% | 282 | 0.094 | 0.186 | 0.093 | 0.206 |
| 36 | 272 | 0.0000 | 0.0106 | 89% | 258 | 0.090 | 0.179 | 0.092 | 0.198 |
| 42 | 249 | 0.0000 | 0.0077 | 89% | 235 | 0.103 | 0.192 | 0.092 | 0.196 |
| 48 | 226 | 0.0000 | 0.0077 | 88% | 213 | 0.086 | 0.177 | 0.096 | 0.199 |
| 60 | 175 | 0.0000 | 0.0119 | 84% | 164 | 0.109 | 0.206 | 0.115 | 0.212 |
| 72 | 121 | 0.0000 | 0.0050 | 92% | 111 | 0.118 | 0.227 | 0.158 | 0.283 |
| 84 | 72 | 0.0000 | 0.0030 | 92% | 64 | 0.082 | 0.190 | 0.185 | 0.297 |
| 96 | 35 | 0.0000 | 0.0025 | 94% | 34 | 0.120 | 0.252 | 0.260 | 0.379 |

<!-- Bangumi subjects: 404, with episode comments 404, with subject comments 406 -->

## Design checks

### us tail level (deflated, months 24–36, no sequel) with windows overlapping 2020-04..2021-06 dropped

|  | months | n | p25 | median | p75 |
|---|---|---|---|---|---|
| all windows | 12 | 978 | 0.047 | 0.074 | 0.122 |
| all windows | 18 | 890 | 0.039 | 0.063 | 0.106 |
| all windows | 24 | 813 | 0.033 | 0.056 | 0.097 |
| all windows | 36 | 670 | 0.022 | 0.041 | 0.082 |
| all windows | 48 | 532 | 0.019 | 0.032 | 0.060 |
| all windows | 60 | 395 | 0.014 | 0.026 | 0.047 |
| all windows | 84 | 180 | 0.010 | 0.021 | 0.051 |
| lockdown windows dropped | 12 | 861 | 0.046 | 0.071 | 0.111 |
| lockdown windows dropped | 18 | 767 | 0.036 | 0.060 | 0.095 |
| lockdown windows dropped | 24 | 676 | 0.032 | 0.052 | 0.089 |
| lockdown windows dropped | 36 | 626 | 0.021 | 0.040 | 0.078 |
| lockdown windows dropped | 48 | 532 | 0.019 | 0.032 | 0.060 |
| lockdown windows dropped | 60 | 395 | 0.014 | 0.026 | 0.047 |
| lockdown windows dropped | 84 | 180 | 0.010 | 0.021 | 0.051 |

### Sequel season vs its prequel: in-season us attention ratio (platform-index deflated)

| gap (prequel finale → sequel premiere) | n | p25 | median | p75 | geo mean |
|---|---|---|---|---|---|
| all | 331 | 0.371 | 0.591 | 0.902 | 0.577 |
| gap < 1y | 148 | 0.434 | 0.635 | 0.882 | 0.598 |
| 1–2y | 100 | 0.425 | 0.684 | 0.960 | 0.658 |
| 2–4y | 68 | 0.260 | 0.433 | 0.931 | 0.514 |
| 4y+ | 15 | 0.208 | 0.296 | 0.413 | 0.283 |

### Sequel announced within 5 years, by manga source state (first seasons; censoring check)

| source state | ≤3y | ≤4y | ≤5y | ≤6y |
|---|---|---|---|---|
| manga running at finale | 28% (408) | 30% (329) | 30% (254) | 27% (182) |
| manga finished by finale | 21% (124) | 24% (92) | 25% (73) | 29% (59) |
| no manga relation | 26% (280) | 25% (241) | 28% (185) | 29% (120) |

### Hazard of a sequel announcement by year since the finale (first seasons, all)

| years after finale | at risk | announced in the year |
|---|---|---|
| 0–1 | 1034 | 13.7% |
| 1–2 | 759 | 3.6% |
| 2–3 | 611 | 2.0% |
| 3–4 | 490 | 1.8% |
| 4–5 | 367 | 0.3% |
| 5–6 | 261 | 0.8% |
| 6–7 | 171 | 1.2% |

