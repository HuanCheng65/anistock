"""The daily inputs' AniList status and next episode (docs/design/corporate.md §2.2–2.3)."""
from datetime import date

from engine.daily_inputs import airing_on


def test_airing_on_takes_the_latest_snapshot_by_the_day():
    snaps = {1: [(date(2026, 11, 1), "RELEASING", {"episode": 5, "date": "2026-11-06"}),
                 (date(2026, 11, 3), "HIATUS", None)]}
    assert airing_on(snaps, 1, date(2026, 10, 31)) == {}
    assert airing_on(snaps, 1, date(2026, 11, 2)) == {"next_airing": {"episode": 5, "date": "2026-11-06"}}
    assert airing_on(snaps, 1, date(2026, 11, 3)) == {"hiatus": True}
    assert airing_on(snaps, 2, date(2026, 11, 3)) == {}
