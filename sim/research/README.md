Research scripts behind docs/design/valuation.md §3.13–3.14 (2026-11-07). Not part of the pipeline: they read the
replay outputs under data/fast/ (value-only marketsim runs, the filter states dumped by a debug build) and the
AniList history DB (data/history.db, sim/history_backfill.py).

- arith.py, revs.py: daily value total return by weeks since the premiere (arithmetic, show-clustered bootstrap)
- exante.py, underreact.py, bymarket.py: does the heat on a day predict later revisions
- hist_shape.py, hist_gamma.py: the weekly fade and its dependence on the level in the 2018-2025 history
- fit_gamma.py: the level-dependent fade by maximum likelihood on the calibration seasons (not adopted)
