"""A second collector run on the same day keeps what the first stored (collect.py `Store`): the day's data stays what
the scheduled run saw, whatever is run by hand later."""

from datetime import datetime, timedelta, timezone

import collect


def at(monkeypatch, hh, mm):
    t = datetime(2026, 11, 10, hh, mm, tzinfo=timezone(timedelta(hours=8)))
    monkeypatch.setattr(collect, "now_local", lambda: t)
    monkeypatch.setattr(collect, "today", lambda: "2026-11-10")


def data(store, sid):
    return store.db.execute("SELECT data FROM snapshots WHERE external_id=?", (str(sid),)).fetchone()[0]


def test_a_later_run_keeps_the_days_snapshots_and_trends(tmp_path, monkeypatch):
    db = tmp_path / "c.db"
    at(monkeypatch, 6, 0)
    s = collect.Store(db)
    run = s.run_start("anilist")
    s.snapshot("bgm", "subject", 1, {"v": 1})
    s.snapshot("bgm", "subject", 1, {"v": 2})        # the same run replaces its own
    s.trend(7, {"date": 1794182400, "episode": None, "trending": 5})
    s.run_end(run, "ok")
    s.commit()
    assert data(s, 1) == '{"v":2}'

    at(monkeypatch, 16, 16)
    s = collect.Store(db)
    s.snapshot("bgm", "subject", 1, {"v": 3})        # kept: the 06:00 run's
    s.snapshot("bgm", "subject", 2, {"v": 1})        # a new work: stored
    s.trend(7, {"date": 1794182400, "episode": None, "trending": 99})
    s.commit()
    assert data(s, 1) == '{"v":2}' and data(s, 2) == '{"v":1}'
    assert '"trending":5' in s.db.execute("SELECT data FROM anilist_trends").fetchone()[0]
    assert s.has_snapshot("bgm", "subject", 2) and not s.has_snapshot("bgm", "subject", 3)

    at(monkeypatch, 17, 0)
    s = collect.Store(db, refresh=True)
    s.snapshot("bgm", "subject", 1, {"v": 4})
    s.trend(7, {"date": 1794182400, "episode": None, "trending": 99})
    s.commit()
    assert data(s, 1) == '{"v":4}'
    assert '"trending":99' in s.db.execute("SELECT data FROM anilist_trends").fetchone()[0]
