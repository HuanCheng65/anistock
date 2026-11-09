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


def test_a_mal_id_shared_by_split_cours_joins_only_the_part_in_season(tmp_path):
    import json
    import sqlite3
    db = tmp_path / "c.db"
    c = sqlite3.connect(db)
    c.execute("CREATE TABLE snapshots (source TEXT, kind TEXT, external_id TEXT, day TEXT, fetched_at TEXT, data TEXT)")
    rows = [("anilist", "media", "900001", {"idMal": 61469, "startDate": {"year": 2026, "month": 3}}),
            ("anilist", "media", "900002", {"idMal": 61469, "startDate": {"year": 2026, "month": 9}}),
            ("annict", "work", "15878", {"malAnimeId": "61469", "seasonYear": 2026, "seasonName": "WINTER"})]
    c.executemany("INSERT INTO snapshots VALUES (?,?,?,?,?,?)",
                  [(s, k, i, "2026-11-09", "", json.dumps(d)) for s, k, i, d in rows])
    c.commit()
    c.close()
    m = E.effective_matches(db)
    assert m["900001"]["annict"] == 15878              # 1st part: winter work, premiere in March
    assert not m.get("900002", {}).get("annict")       # 2nd part: six months later, not joined


def _matches(root, data):
    import json
    (root / "sim").mkdir(parents=True)
    (root / "sim" / "matches.json").write_text(json.dumps({"matches": {}, "manual": {}, **data}))


def test_archive_matches_fill_a_missing_subject_but_never_take_one_in_use(tmp_path, monkeypatch):
    _matches(tmp_path, {"matches": {"1": {"anilist": 1, "bgm": 500}},
                        "archive": {"2": {"bgm": 600, "rule": "exact"}, "3": {"bgm": 500, "rule": "prefix"}}})
    monkeypatch.setattr(E, "ROOT", tmp_path)
    m = E.effective_matches(tmp_path / "none.db")
    assert m["2"]["bgm"] == 600 and m["2"]["bgm_via"] == "archive"
    assert not m["3"].get("bgm")        # 500 is show 1's: one subject under two shows would count its viewers twice


def test_a_subject_in_the_archive_counts_from_its_first_dump(tmp_path, monkeypatch):
    import sqlite3

    import pytest

    from engine import daily_inputs as D
    _matches(tmp_path, {"archive": {"7": {"bgm": 700, "rule": "exact"}}})
    monkeypatch.setattr(E, "ROOT", tmp_path)
    col = tmp_path / "c.db"
    c = sqlite3.connect(col)
    c.execute("CREATE TABLE snapshots (source TEXT, kind TEXT, external_id TEXT, day TEXT, fetched_at TEXT, data TEXT)")
    # the collector starts on 11-01 and meets the subject on 11-10
    c.executemany("INSERT INTO snapshots VALUES (?,?,?,?,?,?)",
                  [("anilist", "media", "9", "2026-11-01", "", "{}"), ("bgm", "subject", "700", "2026-11-10", "", "{}")])
    c.commit()
    c.close()
    arc = tmp_path / "heat_backfill.db"
    a = sqlite3.connect(arc)
    a.execute("CREATE TABLE bgm_anime_weekly (subject_id INTEGER, at TEXT, wish INTEGER, done INTEGER, doing INTEGER, "
              "on_hold INTEGER, dropped INTEGER, score REAL)")
    a.execute("INSERT INTO bgm_anime_weekly VALUES (700, '2025-06-03T21:02:51Z', 0, 0, 0, 0, 0, 0)")
    a.commit()
    a.close()
    assert D.source_dates(col, arc)[7]["cn"] == date(2025, 6, 4)       # not 11-10: the dumps cover it since June
    with pytest.raises(FileNotFoundError):                             # an archive match without the archive
        D.source_dates(col, tmp_path / "missing.db")


def test_the_collector_skips_adult_works_and_promos():
    import collect as C
    base = {"id": 1, "title": {"native": "作品"}, "genres": [], "isAdult": False}
    assert not C.excluded_media(base)
    assert C.excluded_media({**base, "isAdult": True})
    assert C.excluded_media({**base, "genres": ["Hentai"]})
    assert C.excluded_media({**base, "title": {"native": "作品 PV"}})


def test_the_us_platform_takes_the_rebuilt_history_for_the_days_it_covers(tmp_path, monkeypatch):
    import json

    import pytest

    from engine import daily_inputs as D
    monkeypatch.setenv("ANISTOCK_DERIVED_DIR", str(tmp_path))
    day = date(2026, 11, 10)
    rv = {"platform_fixed_cn": 10000.0, "platform_fixed_jp": 1000.0, "platform_history": "platform_us.json"}
    with pytest.raises(FileNotFoundError):          # configured but missing: not an empty history
        D.platform_size([], {}, day, {"revenue": rv})
    (tmp_path / "platform_us.json").write_text(json.dumps(
        {"us": {(day - timedelta(days=k)).isoformat(): 100.0 for k in range(1, 400)}}))
    plat, n = D.platform_size([], {}, day, {"revenue": rv})
    assert n == 365 and plat == {"us": 100.0, "cn": 10000.0, "jp": 1000.0}
