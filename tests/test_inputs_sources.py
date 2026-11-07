from datetime import date, timedelta

from engine import economy as E
from engine.daily_inputs import airing_sources, batch_aired


def test_a_whole_season_released_at_once_counts_as_aired():
    sched = ["2026-10-20"] * 10
    assert batch_aired(sched, date(2026, 10, 19), 1) == (1, None)
    assert batch_aired(sched, date(2026, 10, 20), 1) == (10, "2026-10-20")
    # not premiered: nothing
    assert batch_aired(sched, date(2026, 10, 20), 0) == (0, None)


def test_a_triple_premiere_then_weekly():
    sched = ["2026-10-03"] * 3 + ["2026-10-10", "2026-10-17"]
    assert batch_aired(sched, date(2026, 10, 3), 1)[0] == 3
    # weekly episodes after it come from AniList, not the schedule
    assert batch_aired(sched, date(2026, 10, 10), 3)[0] == 3
    assert batch_aired(sched, date(2026, 10, 10), 4)[0] == 4


def test_weekly_and_announced_dates_are_left_alone():
    sched = ["2026-10-05", "2026-10-12", "2027-04", "2027-04"]
    assert batch_aired(sched, date(2026, 10, 20), 2) == (2, None)


def show(first: date, aired_by_day: list[int], episodes: int | None = 12, long_runner: bool = False) -> E.Show:
    s = E.Show(id=1, title="t", title_en=None, fmt="TV", episodes=episodes, popularity_end=0, studio=None,
               first_air=first, long_runner=long_runner)
    s.days = [first + timedelta(days=i) for i in range(len(aired_by_day))]
    s.aired = aired_by_day
    return s


def test_a_break_in_the_broadcast_keeps_the_source():
    first = date(2026, 7, 1)
    # weekly to episode 8 on day 49, a month's break, episode 9 on day 90
    aired = [min(8, d // 7 + 1) for d in range(90)] + [9] * 10
    s = show(first, aired, episodes=10)
    data_to = first + timedelta(days=56)        # cn data until a week after episode 8
    markets = {1: {(first + timedelta(days=d)).isoformat(): {"cn": 1.0} for d in range(57)}}
    day = first + timedelta(days=89)            # 33 days without cn data, no episode in them
    assert (day - data_to).days > 28
    assert airing_sources(s, day, markets, {"cn": True, "jp": False}) == {"cn": True, "jp": False}


def test_a_dead_source_while_episodes_keep_airing_is_dropped():
    first = date(2026, 1, 1)
    aired = [d // 7 + 1 for d in range(200)]
    s = show(first, aired, episodes=None, long_runner=True)
    markets = {1: {(first + timedelta(days=d)).isoformat(): {"cn": 1.0} for d in range(100)}}
    assert airing_sources(s, first + timedelta(days=110), markets, {"cn": True})["cn"] is True    # 10 days: too soon
    assert airing_sources(s, first + timedelta(days=150), markets, {"cn": True})["cn"] is False


def test_a_source_that_never_reported_keeps_the_inferred_level():
    first = date(2026, 7, 1)
    s = show(first, [d // 7 + 1 for d in range(60)], episodes=12)
    assert airing_sources(s, first + timedelta(days=50), {1: {}}, {"cn": True, "jp": True}) == {"cn": True, "jp": True}


def test_drift_periods_three_or_six():
    from engine.valuation import drift_seg
    assert [drift_seg(3, w) for w in (0, 3, 4, 7, 8, 20)] == [0, 0, 1, 1, 2, 2]
    assert [drift_seg(6, w) for w in (0, 1, 2, 3, 4, 7, 8, 20)] == [0, 1, 2, 3, 4, 4, 5, 5]
