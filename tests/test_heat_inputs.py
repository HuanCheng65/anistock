"""The heat index in the daily inputs, and the Annict records counted once per account and episode."""
import json
import sqlite3
import time
from datetime import date

import collect as C
from engine import heat_index as H
from engine.daily_inputs import load_heat


def test_load_heat(tmp_path):
    path = tmp_path / "heat_index.db"
    db = sqlite3.connect(path)
    db.executescript(H.SCHEMA)
    db.execute("INSERT INTO heat_index VALUES (7,'2026-11-05',1.0,2.0,NULL,1,1,1,47.5,?, 'shadow-2','t')",
               (json.dumps({"jp": "missing"}),))
    db.commit()
    assert load_heat(date(2026, 11, 5), path) == {
        "7": {"idx": 47.5, "us": 1.0, "cn": 2.0, "jp": None, "flags": {"jp": "missing"}, "version": "shadow-2"}}
    assert load_heat(date(2026, 11, 6), path) == {}
    assert load_heat(date(2026, 11, 5), tmp_path / "none.db") == {}


class FakeAnnict:
    """Serves each episode's records newest first, in pages, like the GraphQL endpoint."""

    def __init__(self, records: dict[int, list[tuple[str, int]]]):
        self.records, self.calls = records, 0

    def graphql(self, url, query, v):
        self.calls += 1
        nodes = []
        for eid in v["ids"]:
            rs = sorted(self.records[eid], reverse=True)
            start = int(v.get("after") or 0)
            page = rs[start:start + v["first"]]
            nodes.append({"annictId": eid, "records": {
                "pageInfo": {"hasNextPage": start + v["first"] < len(rs), "endCursor": str(start + v["first"])},
                "nodes": [{"createdAt": t, "user": {"annictId": u}} for t, u in page]}})
        return {"searchEpisodes": {"nodes": nodes}}


def test_viewers_count_each_account_once(tmp_path):
    store = C.Store(tmp_path / "c.db")
    recs = {1: [(f"2026-11-01T00:{i:02d}", i % 40) for i in range(60)],   # 60 records by 40 accounts
            2: [("2026-11-01T01:00", 5), ("2026-11-01T02:00", 5)]}       # one account twice
    api = FakeAnnict(recs)
    eps = [{"annictId": 1, "recordsCount": 60}, {"annictId": 2, "recordsCount": 2}]
    v = C.Viewers(api, store, time.monotonic() + 60)
    assert v.sync(9, eps) == 41
    calls = api.calls
    assert v.sync(9, eps) == 41 and api.calls == calls          # nothing changed: no request
    recs[2].append(("2026-11-02T00:00", 6))                        # one new account on episode 2
    assert v.sync(9, [eps[0], {"annictId": 2, "recordsCount": 3}]) == 42
    assert C.Viewers(api, store, time.monotonic() - 1).sync(9, [{"annictId": 3, "recordsCount": 1}]) is None
