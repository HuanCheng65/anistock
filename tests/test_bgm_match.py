import json
import sqlite3

from engine import bgm_match as B


def test_titles_that_differ_only_in_writing_match():
    pairs = [
        ("ゲルぴよ 2", "ゲルぴよ 第2期"),
        ("ねずみくんのチョッキ (TV) 第2期", "ねずみくんのチョッキ 第2期"),
        ("うまゆるふるげ〜と!", "うまゆる ふるげ～と！"),
        ("Re:ゼロから始める異世界生活 3rd season", "Re:ゼロから始める異世界生活 第3期"),
        ("呪術廻戦 第二期", "呪術廻戦 第2期"),
    ]
    for a, b in pairs:
        assert B.norm(a) == B.norm(b), (a, B.norm(a), B.norm(b))
    assert B.norm("86―エイティシックス―") != B.norm("86―エイティシックス― 2")


def media(aid, native, start="2026-10-05", pop=5000, status="RELEASING", fmt="TV"):
    y, m, d = map(int, start.split("-"))
    return {"id": aid, "title": {"native": native}, "startDate": {"year": y, "month": m, "day": d},
            "popularity": pop, "status": status, "format": fmt}


def collector(shows, subjects):
    db = sqlite3.connect(":memory:")
    db.execute("CREATE TABLE snapshots (source, kind, external_id, day, fetched_at, data)")
    for m in shows:
        db.execute("INSERT INTO snapshots VALUES ('anilist','media',?,'2026-11-07','',?)", (str(m["id"]), json.dumps(m)))
    for s in subjects:
        db.execute("INSERT INTO snapshots VALUES ('bgm','subject',?,'2026-11-07','',?)", (str(s["id"]), json.dumps(s)))
    return db


def test_matches_once_from_the_calendar_then_by_search(tmp_path):
    curated = tmp_path / "matches.json"
    curated.write_text(json.dumps({"matches": {"9": {"bgm": 900}}, "manual": {}}))
    shows = [
        media(1, "ゲルぴよ 2"),                                    # on the calendar
        media(2, "フールナイト", start="2026-11-26", status="NOT_YET_RELEASED", fmt="ONA"),  # search finds it
        media(3, "ガールズ＆パンツァー", start="2026-10-08"),        # only an OVA of the same name, a year earlier
        media(4, "小さい番組", pop=500),                            # too small to bother
        media(9, "既に済み"),                                       # sim/matches.json has it
        media(10, "ゲルぴよ 2 ミニ"),                               # would claim the same subject: not a match
    ]
    subjects = [{"id": 100, "name": "ゲルぴよ 第2期", "date": "2026-10-05"}]
    db = collector(shows, subjects)
    searched = []

    def search(kw):
        searched.append(kw)
        return {
            "フールナイト": [{"id": 200, "name": "フールナイト", "name_cn": "愚者之夜", "date": "2026-11-26"}],
            "ガールズ＆パンツァー": [{"id": 300, "name": "ガールズ＆パンツァー", "date": "2025-12-26"}],
        }.get(kw, [])

    fetched = []
    out = B.match_new(db, "2026-11-08", curated, search, lambda sid: fetched.append(sid) or {"id": sid, "name": "x"})
    rows = {a: (b, v) for a, b, _, v, _ in db.execute("SELECT * FROM bgm_matches")}
    assert rows == {1: (100, "calendar"), 2: (200, "search")}
    assert out["calendar"] == 1 and out["search"] == 1
    # the new subject is snapshotted at once; the one on the calendar already is
    assert fetched == [200]
    # no match for Girls und Panzer: tried, and not again within the week
    assert [r[0] for r in db.execute("SELECT anilist_id FROM bgm_match_tries")] == [3, 10]
    n = len(searched)
    B.match_new(db, "2026-11-10", curated, search, lambda sid: None)
    assert len(searched) == n
    B.match_new(db, "2026-11-15", curated, search, lambda sid: None)
    assert len(searched) == n + 2
    assert B.table(db) == {"1": 100, "2": 200}


def test_manual_bgm_ids(tmp_path):
    from collect import manual_bgm_ids
    f = tmp_path / "matches.json"
    f.write_text(json.dumps({"matches": {"1": {"bgm": 5}},
                             "manual": {"2": {"bgm": 639938}, "3": {"title_cn": "x"}, "4": {"bgm": "612608"}}}))
    assert manual_bgm_ids(f) == [612608, 639938]
    assert manual_bgm_ids(tmp_path / "missing.json") == []
