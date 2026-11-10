"""Sequel announcements from AniList relations (engine/daily_inputs.py `series_facts`, docs/design/series.md §5.2)."""

from datetime import date

from engine.daily_inputs import SEQUEL_ID_BASE, apply_sequel_override, series_facts


def edge(kind, nid, status="NOT_YET_RELEASED", start=None, fmt="TV"):
    return {"relationType": kind, "node": {"id": nid, "type": "ANIME", "format": fmt, "status": status,
                                           "startDate": start or {}}}


def test_announcement_is_the_first_day_the_sequel_is_seen():
    rel = {1: [(date(2026, 9, 1), [edge("PREQUEL", 9, "FINISHED")]),
               (date(2026, 9, 20), [edge("PREQUEL", 9, "FINISHED"), edge("SEQUEL", 5)]),
               (date(2026, 9, 21), [edge("PREQUEL", 9, "FINISHED"), edge("SEQUEL", 5, start={"year": 2027, "month": 4})])]}
    # before the relation appears: no sequel; itself a sequel
    f = series_facts(1, rel, date(2026, 9, 10), [100, 300])
    assert f == {"prequel": True, "sequel": None, "relations_known": True}
    # no date yet: the announcement plus each lag
    f = series_facts(1, rel, date(2026, 9, 20), [100, 300])
    assert f["sequel"] == {"id": 5, "announced": "2026-09-20", "premieres": ["2026-12-29", "2027-07-17"], "exact": False}
    # a month: its middle; the announcement day stays the first sighting
    f = series_facts(1, rel, date(2026, 10, 1), [100, 300])
    assert f["sequel"] == {"id": 5, "announced": "2026-09-20", "premieres": ["2027-04-15"], "exact": False}


def test_aired_sequels_side_stories_and_unknown_shows_are_ignored():
    rel = {1: [(date(2026, 9, 1), [edge("SEQUEL", 5, "FINISHED", {"year": 2025, "month": 1, "day": 3}),
                                   edge("SIDE_STORY", 6), edge("SEQUEL", 7, fmt="MUSIC")])]}
    assert series_facts(1, rel, date(2026, 9, 2), [100]) == {"prequel": False, "sequel": None, "relations_known": True}
    assert series_facts(2, rel, date(2026, 9, 2), [100]) == {"prequel": False, "sequel": None, "relations_known": False}


def test_an_override_announces_before_anilist_and_keeps_its_id_once_listed():
    # docs/design/series.md §5.4: one sequel id from the override's day on, so one notice and one production cost
    ov = {"announced": "2026-11-20", "premiere": "2027-04", "source": "https://example.org/a"}
    rel = {1: [(date(2026, 11, 1), [edge("PREQUEL", 9, "FINISHED")]),
               (date(2026, 11, 25), [edge("PREQUEL", 9, "FINISHED"), edge("SEQUEL", 5, start={"year": 2027, "month": 7})])]}
    before = apply_sequel_override(series_facts(1, rel, date(2026, 11, 19), [100]), 1, ov, date(2026, 11, 19), [100])
    assert before["sequel"] is None
    early = apply_sequel_override(series_facts(1, rel, date(2026, 11, 20), [100]), 1, ov, date(2026, 11, 20), [100])
    assert early["sequel"] == {"id": SEQUEL_ID_BASE + 1, "announced": "2026-11-20", "premieres": ["2027-04-15"],
                               "exact": False, "source": "https://example.org/a"}
    # AniList lists it later: the same id and announcement day, AniList's dates
    listed = apply_sequel_override(series_facts(1, rel, date(2026, 11, 26), [100]), 1, ov, date(2026, 11, 26), [100])
    assert listed["sequel"] == {"id": SEQUEL_ID_BASE + 1, "announced": "2026-11-20", "premieres": ["2027-07-15"],
                                "exact": False, "source": "https://example.org/a"}
    # no premiere given: the fitted lags from the announcement
    bare = apply_sequel_override(series_facts(1, rel, date(2026, 11, 20), [100]), 1,
                                 {"announced": "2026-11-20", "source": "u"}, date(2026, 11, 20), [100])
    assert bare["sequel"]["premieres"] == ["2027-02-28"]


def test_an_official_no_sequel_drops_the_sequel():
    ov = {"cancelled": "2026-12-01", "source": "https://example.org/c"}
    rel = {1: [(date(2026, 11, 1), [edge("SEQUEL", 5, start={"year": 2027})])]}
    f = apply_sequel_override(series_facts(1, rel, date(2026, 11, 30), [100]), 1, ov, date(2026, 11, 30), [100])
    assert f["sequel"]["id"] == 5 and "sequel_cancelled" not in f
    f = apply_sequel_override(series_facts(1, rel, date(2026, 12, 1), [100]), 1, ov, date(2026, 12, 1), [100])
    assert f["sequel"] is None and f["sequel_cancelled"] == "https://example.org/c"


def test_tail_ingest_links_increments_to_the_season():
    import sys
    from pathlib import Path
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sim"))
    import reconstruct as R
    from datetime import timedelta
    d0 = date(2026, 9, 1)
    snaps = {d0 + timedelta(days=k): {"collection": {"wish": 100 + 10 * k, "doing": 0, "collect": 0}} for k in range(30)}
    inc = R.daily_increments(snaps, R.collection_total)
    assert inc[d0 + timedelta(days=5)] == 10.0 and d0 not in inc
    season = {d0 + timedelta(days=k): 50.0 for k in range(1, 20)}
    assert R.tail_link(season, inc) == 5.0
    assert R.tail_link({d0 + timedelta(days=1): 1.0}, inc) is None   # too few days


def rel(kind, nid, fmt="TV"):
    return {"relationType": kind, "node": {"id": nid, "type": "ANIME", "format": fmt}}


def test_cut_merge_and_attached_specials():
    """engine/series.py: a cut relation is not followed; a merge joins works AniList relates otherwise; an OVA or
    special joins the one series it hangs from (none when it hangs from two); a recap (SUMMARY) never."""
    from engine.series import members, series_keys
    edges = {
        10: [rel("SEQUEL", 11), rel("SEQUEL", 20)],     # 10 -> 11 (same line), 10 -> 20 (cut below)
        11: [rel("PREQUEL", 10), rel("SIDE_STORY", 12, "OVA"), rel("SUMMARY", 13, "SPECIAL")],
        20: [rel("PREQUEL", 10), rel("PARENT", 30)],    # 30 relates to 20 as PARENT only: merged below
        30: [rel("SIDE_STORY", 14, "SPECIAL")],
        14: [rel("PARENT", 30), rel("PARENT", 10)],     # hangs from 30's series and 10's: neither
        12: [rel("PARENT", 11)],
    }
    ex = {"subsidiary": {}, "merge": [[20, 30]], "cut": [(10, 20)]}
    ids = [10, 11, 12, 13, 14, 20, 30]
    fmt = {12: "OVA", 13: "SPECIAL", 14: "SPECIAL"}
    k = series_keys(edges, ids, ex, fmt)
    assert k[10] == k[11] == k[12] == 10
    assert k[20] == k[30] == 20
    assert k[13] == 13 and k[14] == 14
    got = members([30], lambda xs: {x: edges.get(x, []) for x in xs}, ex)
    assert got == {20, 30, 14}
    assert members([11], lambda xs: {x: edges.get(x, []) for x in xs}, ex) == {10, 11, 12}


def test_a_series_name_is_its_first_tv_works_title_or_the_table_s():
    """engine/daily_inputs.py `series_titles` (docs/design/series.md §13.7)."""
    from datetime import date
    from engine import economy as E
    from engine.daily_inputs import series_titles

    def show(i, fmt, first):
        return E.Show(id=i, title=f"t{i}", title_en=None, fmt=fmt, episodes=12, popularity_end=0, studio=None, first_air=first)
    shows = [show(1, "MOVIE", date(2013, 3, 1)), show(2, "TV", date(2015, 7, 1)), show(3, "TV", date(2026, 10, 1)),
             show(4, "TV", date(2026, 1, 1))]
    keys = {1: 1, 2: 1, 3: 1, 4: 4}
    assert series_titles(shows, keys, {2: "龙珠超", 3: "龙珠超 比鲁斯"}, {}) == {1: "龙珠超"}   # the film is a side work
    assert series_titles(shows, keys, {2: "龙珠超"}, {"names": {3: "超"}}) == {1: "超"}
