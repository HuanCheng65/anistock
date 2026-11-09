"""The China market's daily attention: episodes watched by Bangumi users, rebuilt from the show's collection counts.

Bangumi publishes no per-episode watch counts, only each subject's collection counts (看过 done, 在看 doing, 搁置 on
hold, 抛弃 dropped) and each episode's comment count. Each count alone is a poor measure of viewing
(sim/research/cn_views/, 2026-11-09):

- 在看 keeps growing through a season and never falls back after the finale: about half of it is never updated;
- new marks come at the premiere and the finale, not week by week;
- 看过 jumps at the finale, when people who watched all along update their mark;
- comments are a burst at episode 1 and at controversial episodes.

So a show's viewing is rebuilt from the flows:

    followers    everyone who marked the show (new done/doing/on hold/dropped), less the explicit 搁置 and 抛弃,
                 and less a silent drop of H_PER_EPISODE per episode aired (people who stop without updating)
    views        followers watch each new episode;
                 a newcomer catches up on the episodes aired so far (after the finale, the whole show):
                 COMPLETE of newcomers watch them all, the rest about a third (at most BACKLOG_CAP episodes);
                 after the finale, a 在看 that turns into something else is a late follower finishing: LAG_EPISODES

The series is weekly, like the Japan market's: an episode's week (from its air day to the next episode, at most 7
days; then 7-day weeks through a break and after the finale) has one value, the views of those days per day. A week
is reported once it is complete and the last complete week is carried until then, so a value never changes once
reported (the valuation reads a changed value as a new observation; 热度快讯 covers the week in between). Units:
episodes watched per day by Bangumi users.

Constants measured on Bangumi alone (never fitted to Annict, so a show with no Japanese data, a Chinese work, gets the
same treatment): H_PER_EPISODE from how many 在看 at the finale turn into 看过 (completers) and, independently, from
per-episode comments after episode 1 (4.7%); COMPLETE is the share of all marks that end as 看过 12 weeks after the
finale. Checked against Annict watch records on 62 shows: flat in season, the finale week's peak, a tail about twice
Japan's after 4 weeks (Chinese viewers catch up more), weekly shape r = 0.89, level across shows r = 0.80.
"""

from __future__ import annotations

from datetime import date, timedelta

H_PER_EPISODE = 0.05
COMPLETE = 0.52
PARTIAL = 1 / 3
LAG_EPISODES = 3
FINISHED_AFTER = 7        # days after the last listed episode: the show counts as finished (catch-up = whole show)
BACKLOG_CAP = 13          # a newcomer's catch-up counts at most one cour: a long-runner's newcomer does not watch
                          # its hundreds of episodes that week (Conan's list was counted as 100 episodes each)
# how a week's viewing of an episode spreads over the days from its air day (the heat index's daily figure): Annict's
# watch records of weekly shows, July-October 2026 (1053 episode weeks; AniList's peaks on the same days). Chinese
# viewers watch the fansubbed episode within a day or two of the Japanese broadcast, as Annict's users do.
WEEK_SHAPE = (0.081, 0.236, 0.253, 0.131, 0.113, 0.101, 0.085)
INIT_ACTIVE = 0.5         # first counts after the premiere, no earlier history: share of 在看 still following


def _daily(stocks: dict[date, tuple[float, float, float, float]]) -> dict[date, tuple[float, ...]]:
    """Per day after the first snapshot, the change of (done, doing, on hold, dropped) since the day before, spread
    evenly over the days between two snapshots."""
    days = sorted(stocks)
    out = {}
    for a, b in zip(days, days[1:]):
        n = (b - a).days
        if n <= 0:
            continue
        inc = tuple((y - x) / n for x, y in zip(stocks[a], stocks[b]))
        for k in range(1, n + 1):
            out[a + timedelta(days=k)] = inc
    return out


def raw_views(stocks: dict[date, tuple[float, float, float, float]], air: list[date], until: date,
             h: float = H_PER_EPISODE) -> dict[date, float]:
    """Episodes watched on each day (the followers' part on the episode's air day), see `parts`."""
    follow, rest = parts(stocks, air, until, h)
    return {d: follow.get(d, 0.0) + v for d, v in rest.items()}


def daily_index(stocks: dict[date, tuple[float, float, float, float]], air: list[date], until: date,
                h: float = H_PER_EPISODE) -> dict[date, float]:
    """The heat index's China figure (engine/heat_index.py v2) for each day D up to `until` - 1, from the counts up to
    D + 1 (the snapshots of D and D + 1) and the episodes aired by D: the followers' views of each episode spread over
    the seven days from its air day by `WEEK_SHAPE` (most of it the day after airing and the next), plus the
    newcomers' catch-up and the late finishers of the counts' change from D to D + 1. A week's total is the same as
    an even spread's."""
    follow, rest = parts(stocks, air, until, h)
    out = {}
    if not air:
        return out
    d = min(air)
    while d < until:
        out[d] = sum(follow.get(d - timedelta(days=k), 0.0) * WEEK_SHAPE[k] for k in range(7)) + rest.get(d + timedelta(days=1), 0.0)
        d += timedelta(days=1)
    return out


def parts(stocks: dict[date, tuple[float, float, float, float]], air: list[date], until: date,
          h: float = H_PER_EPISODE) -> tuple[dict[date, float], dict[date, float]]:
    """Episodes watched from the day after the first snapshot to `until`, in two parts: ({air day: the followers' views
    of its episodes}, {day: a newcomer's catch-up on the day they mark the show, and the late finishers}).

    stocks: {day: (done, doing, on hold, dropped)} snapshots (weekly or daily, any spacing).
    air:    each main episode's air date (the episodes listed so far; a date after `until` is not counted)."""
    if not stocks or not air:
        return {}, {}
    air = sorted(air)
    eps_on: dict[date, int] = {}
    for d in air:
        eps_on[d] = eps_on.get(d, 0) + 1
    n_total = len(air)
    last = air[-1]
    inc = _daily(stocks)
    first = min(stocks)
    d0 = stocks[first]
    # followers before the first daily change: everyone marked if the counts start well before the premiere,
    # else the part of 在看 still following (no history to tell them apart)
    a = d0[1] if first <= air[0] - timedelta(days=7) else d0[1] * INIT_ACTIVE
    aired = sum(1 for d in air if d <= first)
    follow: dict[date, float] = {}
    rest: dict[date, float] = {}
    d = first + timedelta(days=1)
    while d <= until:
        new = eps_on.get(d, 0)
        done, doing, hold, drop = inc.get(d, (0.0, 0.0, 0.0, 0.0))
        newcomers = max(done + doing + hold + drop, 0.0)
        finished = d >= last + timedelta(days=FINISHED_AFTER) and last <= until
        a = max(a * (1 - h) ** new + newcomers - max(hold, 0.0) - max(drop, 0.0), 0.0)
        backlog = min(n_total if finished else aired, BACKLOG_CAP)
        if new:
            follow[d] = a * new
        v = newcomers * backlog * (COMPLETE + (1 - COMPLETE) * PARTIAL)
        if finished:
            v += max(-doing, 0.0) * LAG_EPISODES
        rest[d] = v
        aired += new
        d += timedelta(days=1)
    return follow, rest


def weeks(air: list[date], until: date) -> list[date]:
    """The first day of every week of the series up to `until`: each episode's air day, and every 7 days through a
    break and after the last episode."""
    days = sorted(set(d for d in air if d <= until))
    out: list[date] = []
    for i, d in enumerate(days):
        nxt = days[i + 1] if i + 1 < len(days) else until + timedelta(days=1)
        w = d
        while w < nxt:
            out.append(w)
            w += timedelta(days=7)
    return out


def daily_views(stocks: dict[date, tuple[float, float, float, float]], air: list[date], until: date,
                h: float = H_PER_EPISODE, still_airing: bool = False) -> dict[date, float]:
    """The weekly series spread over days, {day: views per day}, from the first episode to `until`: each complete
    week's mean daily views; the days of the week in progress carry the last complete week (nothing before the first
    week is complete). A week without a new episode while the show is still on (a later episode follows, or
    `still_airing`: a long runner or a season on a break at `until`) carries the last week too: its followers have
    nothing new to watch that week, which says nothing about how many there are (Conan's weeks without an episode
    on AniList fell to a quarter and came back; BLEACH's month-long break)."""
    raw = raw_views(stocks, air, until, h)
    if not raw:
        return {}
    starts = weeks(air, until)
    out: dict[date, float] = {}
    last_val = None
    for i, w in enumerate(starts):
        end = starts[i + 1] if i + 1 < len(starts) else w + timedelta(days=7)
        n = (end - w).days
        complete = end <= until + timedelta(days=1)
        new_ep = any(w <= d < end for d in air)
        on_break = not new_ep and (still_airing or any(d >= end for d in air if d <= until))
        if complete and not (on_break and last_val is not None):
            last_val = sum(raw.get(w + timedelta(days=k), 0.0) for k in range(n)) / n
        if last_val is None:
            continue
        for k in range(n):
            if w + timedelta(days=k) <= until:
                out[w + timedelta(days=k)] = round(last_val, 3)
    return out


def load_stocks(collector: "Path", archive: "Path | None", subjects: set[int] | None = None,
                until: date | None = None) -> dict[int, dict[date, tuple[float, float, float, float]]]:
    """Bangumi subject -> {day: (done, doing, on hold, dropped)}: the collector's daily subject snapshots (up to `until`),
    and before the first of them the official archive's weekly counts (`archive`, data/derived/heat_backfill.db, when
    present; the dump is taken Tuesday ~21:00 UTC: Wednesday's counts in China); one source per stretch of days.
    `subjects`: only these."""
    import json
    import sqlite3
    from collections import defaultdict

    stocks: dict[int, dict[date, tuple]] = defaultdict(dict)
    db = sqlite3.connect(f"file:{collector}?mode=ro", uri=True)
    q = "SELECT external_id, day, data FROM snapshots WHERE source='bgm' AND kind='subject'"
    for ext, d, data in db.execute(q + (" AND day<=?" if until else ""), ((until.isoformat(),) if until else ())):
        sid = int(ext)
        if subjects is not None and sid not in subjects:
            continue
        c = (json.loads(data).get("collection") or {})
        if c:
            stocks[sid][date.fromisoformat(d)] = (float(c.get("collect") or 0), float(c.get("doing") or 0),
                                                  float(c.get("on_hold") or 0), float(c.get("dropped") or 0))
    # the first day the collector has each subject's counts, computed once before the archive rows are added (computed
    # per row, the first archive row became the "first day" and every later dump was dropped)
    first_daily = {sid: min(days) for sid, days in stocks.items() if days}
    if archive is not None and archive.exists():
        a = sqlite3.connect(f"file:{archive}?mode=ro", uri=True)
        tables = {r[0] for r in a.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        for t in ("bgm_weekly", "bgm_anime_weekly"):
            if t not in tables:
                continue
            for sid, at, done, doing, hold, drop in a.execute(f"SELECT subject_id, at, done, doing, on_hold, dropped FROM {t}"):
                if subjects is not None and sid not in subjects:
                    continue
                d = date.fromisoformat(at[:10]) + timedelta(days=1)
                fd = first_daily.get(sid)
                if (fd is None or d < fd) and (until is None or d <= until):
                    stocks[sid].setdefault(d, (done or 0, doing or 0, hold or 0, drop or 0))
    return stocks

