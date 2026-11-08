"""Market-wide AniList gaps (engine/economy.py): a long gap is found, and the days in it are drawn between the
measured days around it."""
from datetime import date, timedelta

from engine import economy as E


def day(n: int) -> date:
    return date(2026, 3, 1) + timedelta(days=n)


def market(n_shows: int, days: int, holes: set[int], partial: dict[int, float] = {}) -> dict[int, dict[date, dict]]:
    """n_shows shows with a row a day (trending 100, popularity +10 a day, an episode every 7th day), except on the
    `holes` (no rows at all) and `partial` days (trending times the factor)."""
    out: dict[int, dict[date, dict]] = {}
    for m in range(1, n_shows + 1):
        t = {}
        for i in range(days):
            if i in holes:
                continue
            row = {"trending": 100 * partial.get(i, 1.0), "popularity": 1000 + 10 * i, "inProgress": 50}
            if i % 7 == 0:
                row["episode"] = i // 7 + 1
            t[day(i)] = row
        out[m] = t
    return out


def test_a_gap_longer_than_a_week_is_found():
    holes = set(range(60, 77))            # 17 days, like AniList's 2026-04-02..18
    gaps = E.find_gap_days(market(30, 100, holes, {59: 0.1}))
    assert {day(i) for i in holes} <= gaps
    assert day(59) in gaps                # the partial day before it
    assert day(58) not in gaps and day(77) not in gaps


def test_a_normal_weekly_low_is_not_a_gap():
    lows = {i: 0.6 for i in range(40, 100, 7)}
    assert E.find_gap_days(market(30, 100, set(), lows)) == set()


def test_gap_days_are_drawn_between_the_days_around_them():
    holes = set(range(60, 77))
    trends = market(5, 100, holes)
    gaps = E.find_gap_days(trends)
    s = E.Show(id=1, title="x", title_en=None, fmt="TV", episodes=None, popularity_end=0, studio=None, adult=False,
               promo=False, source=None, genres=[])
    E._fill_grid(s, trends[1], day(99), gaps)
    i = (day(68) - s.days[0]).days
    assert s.trending[i] == 100                                  # not 0
    assert s.popularity[i] == 1000 + 10 * 68                     # the adds spread over the gap, no jump after it
    assert s.popularity[i + 1] - s.popularity[i] == 10
    # episodes of days 63 and 70 were not booked (holes): the count rises through the gap to the next booked one
    assert s.aired[(day(59) - s.days[0]).days] == 9
    assert s.aired[(day(77) - s.days[0]).days] == 12
    assert 9 < s.aired[i] < 12
    airing = sum(s.aired[k] > s.aired[k - 14] for k in range(i - 7, i + 7))
    assert airing == 14                                          # a weekly show stays airing through the gap
