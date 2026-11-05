"""Sequel announcements from AniList relations (engine/daily_inputs.py `series_facts`, docs/v2-series.md §5.2)."""

from datetime import date

from engine.daily_inputs import series_facts


def edge(kind, nid, status="NOT_YET_RELEASED", start=None, fmt="TV"):
    return {"relationType": kind, "node": {"id": nid, "type": "ANIME", "format": fmt, "status": status,
                                           "startDate": start or {}}}


def test_announcement_is_the_first_day_the_sequel_is_seen():
    rel = {1: [(date(2026, 9, 1), [edge("PREQUEL", 9, "FINISHED")]),
               (date(2026, 9, 20), [edge("PREQUEL", 9, "FINISHED"), edge("SEQUEL", 5)]),
               (date(2026, 9, 21), [edge("PREQUEL", 9, "FINISHED"), edge("SEQUEL", 5, start={"year": 2027, "month": 4})])]}
    # before the relation appears: no sequel; itself a sequel
    f = series_facts(1, rel, date(2026, 9, 10), [100, 300])
    assert f == {"prequel": True, "sequel": None}
    # no date yet: the announcement plus each lag
    f = series_facts(1, rel, date(2026, 9, 20), [100, 300])
    assert f["sequel"] == {"id": 5, "announced": "2026-09-20", "premieres": ["2026-12-29", "2027-07-17"], "exact": False}
    # a month: its middle; the announcement day stays the first sighting
    f = series_facts(1, rel, date(2026, 10, 1), [100, 300])
    assert f["sequel"] == {"id": 5, "announced": "2026-09-20", "premieres": ["2027-04-15"], "exact": False}


def test_aired_sequels_side_stories_and_unknown_shows_are_ignored():
    rel = {1: [(date(2026, 9, 1), [edge("SEQUEL", 5, "FINISHED", {"year": 2025, "month": 1, "day": 3}),
                                   edge("SIDE_STORY", 6), edge("SEQUEL", 7, fmt="MUSIC")])]}
    assert series_facts(1, rel, date(2026, 9, 2), [100]) == {"prequel": False, "sequel": None}
    assert series_facts(2, rel, date(2026, 9, 2), [100]) == {"prequel": False, "sequel": None}
