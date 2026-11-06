"""Post-finale (long-tail) data from the history backfill (sim/history_backfill.py), for sim/fit_valuation.py.

The history DB holds AniList daily trends of ~1,700 TV/ONA shows (2018-2025): in season, and in 14-day windows
1-96 months after the finale; and a Bangumi sample (episode comments, subject = collection comments). This module
turns it into per-show post-finale factors in the form the valuation uses (engine/valuation.py `post_finale_factor`):

    factor_i(t) = attention_i(t) / (level_i at the finale × finale bump) × platform(finale) / platform(t)

t = days after the end of the finale's week (finale + 7). The level at the finale is the mean attention of the four
weeks before the finale's week. The platform index (median in-season attention of the quarter's shows, smoothed over
three quarters) deflates platform growth: the game's revenue is a share of the platform (docs/v2-series.md §6), so
the tail is measured as a share. Windows from the first trend day of a sequel on (the announcement proxy) are left
out: the valuation's tail is the tail of a show with no sequel announced; the sequel is a separate option (§5).

us attention as the game defines it (sim/params.toml): 2 × daily list adds + trending.
cn: Bangumi subject (collection) comments per day, the proxy for new collections (sim/calibration/long_tail_sequel.md).
"""

from __future__ import annotations

import json
import math
import sqlite3
from collections import defaultdict
from datetime import date, datetime, timedelta, timezone

JST = timezone(timedelta(hours=9))
SERIES = {"TV", "TV_SHORT", "ONA"}


def fdate(d):
    if not d or not d.get("year") or not d.get("month"):
        return None
    return date(d["year"], d["month"], d.get("day") or 15)


def qmed(xs):
    xs = sorted(xs)
    n = len(xs)
    return (xs[(n - 1) // 2] + xs[n // 2]) / 2 if n else float("nan")


class History:
    def __init__(self, path: str, today: date = date(2026, 11, 4)):
        db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        self.today = today
        self.media = {mid: json.loads(d) for mid, d in db.execute("SELECT id, data FROM media WHERE in_scope=1")}
        self.trend = defaultdict(dict)
        for mid, day, tr, pop in db.execute("SELECT media_id, day, trending, popularity FROM trends"):
            self.trend[mid][date.fromisoformat(day)] = (tr or 0, pop or 0)
        self.wins = defaultdict(dict)
        for mid, tag, a, b in db.execute("SELECT media_id, tag, start, end FROM windows_done"):
            self.wins[mid][tag] = (date.fromisoformat(a), date.fromisoformat(b))
        self.first_trend = {mid: date.fromisoformat(d) for mid, d in
                            db.execute("SELECT media_id, day FROM first_trend WHERE day IS NOT NULL")}
        self.bgm_map = dict(db.execute("SELECT anilist_id, bgm_id FROM bgm_map"))
        self.bgm_ep = defaultdict(list)
        for bid, sort, airdate, times in db.execute("SELECT bgm_id, sort, airdate, times FROM bgm_ep_comments"):
            self.bgm_ep[bid].append((sort, airdate, json.loads(times)))
        self.bgm_subj = defaultdict(list)
        for bid, off, total, times in db.execute("SELECT bgm_id, offset, total, times FROM bgm_subject_comments"):
            self.bgm_subj[bid].append((off, total, json.loads(times)))
        self._index()

    # ---- us attention
    def window_att(self, mid, a: date, b: date) -> float:
        """Mean daily attention (2 × list adds + trending) over [a, b]; missing days: no activity."""
        t = self.trend[mid]
        n = (b - a).days + 1
        rows = [(a + timedelta(days=k), t[a + timedelta(days=k)]) for k in range(n) if a + timedelta(days=k) in t]
        tr = sum(r[1][0] for r in rows) / n
        adds = 0.0
        if len(rows) >= 2:
            adds = max(0.0, (rows[-1][1][1] - rows[0][1][1]) / max(1, (rows[-1][0] - rows[0][0]).days))
        return 2 * adds + tr

    def _index(self):
        """Platform index: median in-season attention/day of the in-scope shows airing in each quarter."""
        per_q = defaultdict(list)
        for mid, m in self.media.items():
            s_, e_ = fdate(m["startDate"]), fdate(m["endDate"])
            if not s_ or mid not in self.wins:
                continue
            e_ = e_ if e_ and m["status"] == "FINISHED" else self.today
            e_ = min(e_, s_ + timedelta(days=120))
            x = self.window_att(mid, s_, e_)
            if x > 0:
                per_q[self.qkey(s_ + (e_ - s_) / 2)].append(math.log(x))
        self.qs = sorted(k for k, v in per_q.items() if len(v) >= 15)
        self.INDEX = {k: math.exp(qmed(per_q[k])) for k in self.qs}

    @staticmethod
    def qkey(d):
        return d.year * 4 + (d.month - 1) // 3

    def index_at(self, d):
        k = self.qkey(d)
        near = [x for x in self.qs if abs(x - k) <= 1]
        if not near:
            near = [self.qs[-1]] if k > self.qs[-1] else [self.qs[0]]
        return math.exp(sum(math.log(self.INDEX[x]) for x in near) / len(near))

    def sequel_nodes(self, m):
        return [ed["node"] for ed in m["relations"]["edges"]
                if ed["relationType"] == "SEQUEL" and ed["node"]["type"] == "ANIME" and ed["node"]["format"] != "MUSIC"]

    def announced(self, m) -> date | None:
        """The first sign of a sequel: its first AniList trend day, else its start date."""
        ds = []
        for n in self.sequel_nodes(m):
            if n["format"] not in SERIES | {"MOVIE"}:
                continue
            if n["id"] in self.first_trend:
                ds.append(self.first_trend[n["id"]])
            if fdate(n.get("startDate")):
                ds.append(fdate(n["startDate"]))
        return min(ds) if ds else None

    def finished_shows(self):
        for mid, m in self.media.items():
            s, e = fdate(m["startDate"]), fdate(m["endDate"])
            if m["status"] != "FINISHED" or not s or not e or mid not in self.wins:
                continue
            if (e - s).days > 400 or (m.get("episodes") or 0) > 60:
                continue  # long-runners: a separate population
            if (e - s).days < 28:
                continue  # a short or all-at-once release: no in-season level to compare with
            yield mid, m, s, e

    def pop_at_finale(self, mid, m, e: date) -> float:
        """AniList list entries two weeks after the finale (the first trend day from e+14 on), else today's."""
        t = self.trend.get(mid, {})
        for k in range(14, 45):
            d = e + timedelta(days=k)
            if d in t and t[d][1]:
                return t[d][1]
        return m.get("popularity") or 0

    def pop_at(self, mid, d: date) -> float | None:
        """AniList list entries on day d (the first trend day within a week after it), None without data."""
        t = self.trend.get(mid, {})
        for k in range(0, 7):
            x = t.get(d + timedelta(days=k))
            if x and x[1]:
                return x[1]
        return None

    def popularity_percentiles(self, min_pop: float = 0.0):
        """({id: percentile of its popularity at the finale among the finished shows of its finale year},
        {year: sorted popularity at the finale}) (sim/long_tail_analysis.py's definition). `min_pop`: only shows with at
        least this many list entries at the finale (the game lists a show only from 3,000; the history holds shows with
        3,000 today, so below that at the finale it is selected on a tail that grew)."""
        pops = {mid: (e.year, self.pop_at_finale(mid, m, e)) for mid, m, s, e in self.finished_shows()}
        pops = {mid: v for mid, v in pops.items() if v[1] >= min_pop}
        by_y = defaultdict(list)
        for y, p in pops.values():
            by_y[y].append(p)
        pct = {mid: sum(1 for x in by_y[y] if x < p) / len(by_y[y]) for mid, (y, p) in pops.items()}
        return pct, {y: sorted(v) for y, v in by_y.items()}

    def popularity_by_week(self, year: int, min_pop: float, weeks: int = 13):
        """[sorted list entries w weeks after the premiere, w = 0..weeks-1] over the shows finishing in `year` with at
        least `min_pop` at the finale and still airing then (the in-season reference for a show not finished yet)."""
        out = [[] for _ in range(weeks)]
        for mid, m, s, e in self.finished_shows():
            if e.year != year or self.pop_at_finale(mid, m, e) < min_pop:
                continue
            for w in range(weeks):
                d = s + timedelta(days=7 * w)
                if d >= e:
                    break
                v = self.pop_at(mid, d)
                if v:
                    out[w].append(v)
        return [sorted(v) for v in out]

    def us_tail(self, bump: float = 1.0):
        """[(show id, media, [(t days after finale+7, factor)])]: deflated factors relative to level × bump."""
        out = []
        for mid, m, s, e in self.finished_shows():
            ref = self.window_att(mid, max(s, e - timedelta(days=28)), e - timedelta(days=1))
            if ref <= 0:
                continue
            i0 = self.index_at(e - timedelta(days=14))
            ann = self.announced(m)
            pts = []
            for tag, (a, b) in self.wins[mid].items():
                if not tag.startswith("t"):
                    continue
                if ann is not None and ann <= b:
                    continue
                if b > self.today:
                    continue
                x = self.window_att(mid, a, b)
                mid_day = a + (b - a) / 2
                t = (mid_day - (e + timedelta(days=7))).days
                pts.append((t, x / ref * i0 / self.index_at(a) / bump))
            pts.sort()
            out.append((mid, m, pts))
        return out

    # ---- cn: Bangumi subject (collection) comments
    def _n_since(self, bid, t: float) -> float:
        """Number of subject comments with updatedAt >= t (unix), interpolated between the sampled pages (offsets
        0, 200, 400, 800, ... and the last page; comments newest first)."""
        pts = [(datetime.now(JST).timestamp() + 1, 0)]
        total = 0
        for off, tot, ts in self.bgm_subj[bid]:
            total = max(total, tot)
            for i, x in enumerate(ts):
                pts.append((x, off + i + 1))
        pts.sort(key=lambda p: -p[0])
        if t < pts[-1][0]:
            return total
        for (t0, r0), (t1, r1) in zip(pts, pts[1:]):
            if t1 <= t <= t0:
                return r0 + (r1 - r0) * (t0 - t) / (t0 - t1) if t0 > t1 else r1
        return total

    @staticmethod
    def _ts(d: date) -> float:
        return datetime(d.year, d.month, d.day, tzinfo=JST).timestamp()

    def cn_tail(self, windows, bump: float = 1.0, min_total: int = 50):
        """[(show id, media, [(t, factor)])]: subject comments per day in the windows [(t0, t1)] (days after the
        finale's week), relative to the rate of the four weeks before the finale's week (× bump). Windows from the
        sequel's first sign on are left out. The finale is the last episode's air date on Bangumi."""
        out = []
        for mid, m, s, e in self.finished_shows():
            bid = self.bgm_map.get(mid)
            pages = self.bgm_subj.get(bid)
            if not pages or max(p[1] for p in pages) < min_total:
                continue
            ref = (self._n_since(bid, self._ts(e - timedelta(days=28))) - self._n_since(bid, self._ts(e))) / 28
            if ref <= 0:
                continue
            ann = self.announced(m)
            pts = []
            base = e + timedelta(days=7)
            for t0, t1 in windows:
                a, b = base + timedelta(days=t0), base + timedelta(days=t1)
                if b > self.today or (ann is not None and ann <= b):
                    continue
                n = (self._n_since(bid, self._ts(a)) - self._n_since(bid, self._ts(b))) / (t1 - t0)
                pts.append(((t0 + t1) / 2, n / ref / bump))
            out.append((mid, m, pts))
        return out

    def cn_index(self, min_total: int = 50):
        """Bangumi's platform index, the counterpart of the us one: by year, the median per-day subject (collection)
        comment rate of the sampled shows over their season and the two weeks after (shows starting that year;
        ~40-50 a year, so yearly rather than quarterly). {year: rate}."""
        by = defaultdict(list)
        for mid, m, s, e in self.finished_shows():
            bid = self.bgm_map.get(mid)
            pages = self.bgm_subj.get(bid)
            if not pages or max(p[1] for p in pages) < min_total:
                continue
            ins = (self._n_since(bid, self._ts(s)) - self._n_since(bid, self._ts(e + timedelta(days=14)))) / ((e - s).days + 14)
            if ins > 0:
                by[s.year].append(math.log(ins))
        return {y: math.exp(qmed(v)) for y, v in by.items() if len(v) >= 10}

    def cn_index_at(self, idx, d: date) -> float:
        ys = sorted(idx)
        y = min(max(d.year, ys[0]), ys[-1])
        while y not in idx:
            y -= 1
        return idx[y]

    def cn_collection_vs_season(self, windows, min_total: int = 50):
        """[(show id, [(t, ratio)])]: subject comments per day in the windows (days after the finale's week, the
        finale = AniList endDate), relative to the per-day rate over the season and the two weeks after it (the
        normalisation of sim/calibration/long_tail_sequel.md), deflated by Bangumi's platform index (`cn_index`).
        Windows from the sequel's first sign on are left out."""
        idx = self.cn_index(min_total)
        out = []
        for mid, m, s, e in self.finished_shows():
            bid = self.bgm_map.get(mid)
            pages = self.bgm_subj.get(bid)
            if not pages or max(p[1] for p in pages) < min_total:
                continue
            days_in = (e - s).days + 14
            ins = (self._n_since(bid, self._ts(s)) - self._n_since(bid, self._ts(e + timedelta(days=14)))) / days_in
            if ins <= 0:
                continue
            ann = self.announced(m)
            base = e + timedelta(days=7)
            pts = []
            for t0, t1 in windows:
                a, b = base + timedelta(days=t0), base + timedelta(days=t1)
                if b > self.today or (ann is not None and ann <= b):
                    continue
                defl = self.cn_index_at(idx, s) / self.cn_index_at(idx, a)
                pts.append(((t0 + t1) / 2, (self._n_since(bid, self._ts(a)) - self._n_since(bid, self._ts(b))) / (t1 - t0) / ins * defl))
            out.append((mid, pts))
        return out

    def cn_episode_decay(self, weeks: int = 8):
        """[(show id, [(t, ratio)])]: Bangumi episode comments per day on the sampled episodes (first, middle, last)
        in each week after the finale's week, relative to their per-day rate in the 28 days after each airs (the
        in-season weekly level of the game's cn metric)."""
        out = []
        for mid, m, s, e in self.finished_shows():
            bid = self.bgm_map.get(mid)
            eps = self.bgm_ep.get(bid)
            if not eps:
                continue
            lvl, cnt = 0.0, 0
            per = []
            for sort, airdate, times in eps:
                if not airdate or len(airdate) != 10:
                    continue
                ad = date.fromisoformat(airdate)
                ds = [datetime.fromtimestamp(t, JST).date() for t in times]
                lvl += sum(1 for d in ds if 0 <= (d - ad).days < 28) / 28
                cnt += 1
                per.append(ds)
            if not cnt or lvl <= 0:
                continue
            lvl /= cnt
            base = e + timedelta(days=7)
            pts = []
            for w in range(weeks):
                a, b = base + timedelta(days=7 * w), base + timedelta(days=7 * w + 7)
                if b > self.today:
                    continue
                n = sum(sum(1 for d in ds if a <= d < b) for ds in per) / cnt / 7
                pts.append((7 * w + 3.5, n / lvl))
            out.append((mid, pts))
        return out
