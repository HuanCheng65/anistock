"""The China market's viewer series (engine/cn_views.py): weekly blocks published once complete, carried through weeks
without an episode while the show is on, a finale peak and a tail after it."""
from datetime import date, timedelta

from engine import cn_views as C

D0 = date(2026, 7, 1)


def day(n: int) -> date:
    return D0 + timedelta(days=n)


def weekly_stocks(n_weeks: int, new_per_week: float = 70.0, finale_week: int | None = None) -> dict:
    """Weekly (done, doing, on hold, dropped) snapshots from two weeks before the premiere: `new_per_week` people mark
    the show 在看 each week; at `finale_week` half of 在看 turn into 看过."""
    out, done, doing = {}, 0.0, 500.0
    for w in range(-2, n_weeks):
        if finale_week is not None and w == finale_week:
            done, doing = done + doing / 2, doing / 2
        doing += new_per_week
        out[day(7 * w)] = (done, doing, 0.0, 0.0)
    return out


def test_a_week_is_published_once_complete_and_never_changes():
    air = [day(7 * k) for k in range(12)]
    st = weekly_stocks(20)
    early = C.daily_views(st, air, day(17))
    late = C.daily_views(st, air, day(60))
    # day 17: the week from day 14 is not complete, so it carries the week from day 7
    assert early[day(17)] == early[day(13)]
    for d, v in early.items():
        assert late[d] == v or d >= day(14), d


def test_a_week_without_an_episode_carries_the_last_while_the_show_is_on():
    air = [day(7 * k) for k in range(6)] + [day(7 * k) for k in range(8, 12)]   # no episode in weeks 6 and 7
    v = C.daily_views(weekly_stocks(20), air, day(90))
    assert v[day(40)] == v[day(44)] == v[day(50)]   # weeks from days 42 and 49 carry the week from day 35
    # a long runner past its latest episode: the same, while it is still airing
    w = C.daily_views(weekly_stocks(20), [day(7 * k) for k in range(6)], day(60), still_airing=True)
    assert w[day(50)] == w[day(40)]


def test_followers_watch_each_episode_and_the_finale_peaks_then_falls():
    air = [day(7 * k) for k in range(12)]
    v = C.daily_views(weekly_stocks(24, finale_week=12), air, day(160))
    mid = [v[day(7 * k)] for k in range(4, 10)]
    assert max(mid) / min(mid) < 1.5                         # roughly flat in season
    peak = v[day(77)]                                        # the finale's week
    assert peak > mid[-1]
    assert v[day(77 + 35)] < 0.5 * peak                      # the tail after it


def test_a_long_runners_newcomer_catches_up_one_cour_at_most():
    many = [day(-7 * k) for k in range(200)][::-1] + [day(7 * k) for k in range(1, 4)]
    few = [day(7 * k) for k in range(-12, 4)]
    st = weekly_stocks(6)
    a = C.raw_views(st, many, day(20))
    b = C.raw_views(st, few, day(20))
    assert max(a.values()) <= max(b.values()) * 1.05


def test_every_field_the_inputs_write_parses_back():
    # engine/daily_inputs.py: a key `build` writes that `ShowInputs` lacks broke server.seed twice (genres, prior_prev)
    from dataclasses import fields
    from engine import daily_inputs as DI
    src = open(DI.__file__, encoding="utf-8").read()
    body = src[src.index("def build("):src.index("def daily_inputs(")]
    import re
    written = set(re.findall(r'^\s+"([a-z_]+)":', body[body.index("out.append({"):body.index("plat, n = platform_size")], re.M))
    written |= {"prior_prev"} if "prior_prev(s, day" in body else set()
    assert written <= {f.name for f in fields(DI.ShowInputs)}, written - {f.name for f in fields(DI.ShowInputs)}
