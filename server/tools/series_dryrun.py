"""Dry run of the series share swap (docs/design/series.md §7): which listed companies would merge and at what ratio.

    python -m server.tools.series_dryrun GAME_DB [--collector anistock.db] [--fetch] [--json OUT]

Reads a COPY of the game database (made here with SQLite's backup API into a temporary file; the given database is
only read) and groups the listed companies by series (engine/series.py: AniList SEQUEL/PREQUEL relations plus
sim/series.json). Relations come from the collector's 'relations' snapshots; --fetch asks AniList for the companies
that have none (read-only GraphQL; honours HTTPS_PROXY). Nothing is written to any database.

Per series with two or more listed companies: the survivor is the earliest listed; each other company's shares swap
at its last close ÷ the survivor's last close (§7: market prices, no premium, holders' market value unchanged);
fractions are paid in cash at the close. The report lists the ratios, the shares and holders involved and the cash
for fractions. The swap itself is NOT implemented: it waits for the user's approval.
"""

from __future__ import annotations

import argparse
import json
import math
import sqlite3
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT))

from engine.daily_inputs import load_relations  # noqa: E402
from engine.series import load_exceptions, series_keys  # noqa: E402

RELATIONS_QUERY = """
query ($ids: [Int]) { Page(page: 1, perPage: 50) { media(id_in: $ids, type: ANIME) { id
  relations { edges { relationType node { id type format status startDate { year month day } title { native } } } } } } }
"""


def copy_db(src: Path, dst: Path) -> None:
    a = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
    b = sqlite3.connect(dst)
    a.backup(b)
    a.close()
    b.close()


def fetch(ids: list[int]) -> dict[int, list]:
    import httpx
    out = {}
    with httpx.Client(timeout=30, trust_env=True) as c:
        for i in range(0, len(ids), 50):
            r = c.post("https://graphql.anilist.co", json={"query": RELATIONS_QUERY, "variables": {"ids": ids[i:i + 50]}})
            r.raise_for_status()
            for m in r.json()["data"]["Page"]["media"]:
                out[m["id"]] = m["relations"]["edges"]
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("game_db", type=Path)
    ap.add_argument("--collector", type=Path, default=ROOT / "data" / "anistock.db")
    ap.add_argument("--fetch", action="store_true")
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        copy = Path(tmp) / "game.db"
        copy_db(a.game_db, copy)
        db = sqlite3.connect(copy)
        cos = db.execute("SELECT id, code, title, listed_on, last_price, shares FROM companies WHERE status='listed'").fetchall()
        close = dict(db.execute("""SELECT company_id, close FROM candles c WHERE day = (SELECT max(day) FROM candles
                                   WHERE company_id = c.company_id)""").fetchall())
        holdings = db.execute("""SELECT h.company_id, h.account_id, h.qty, a.kind FROM holdings h
                                 JOIN accounts a ON a.id = h.account_id WHERE h.qty > 0""").fetchall()
        db.close()
    rel = {i: snaps[-1][1] for i, snaps in load_relations(a.collector).items()} if a.collector.exists() else {}
    ids = [c[0] for c in cos]
    missing = [i for i in ids if i not in rel]
    if a.fetch and missing:
        rel.update(fetch(missing))
        missing = [i for i in ids if i not in rel]
    keys = series_keys(rel, ids, load_exceptions())
    groups: dict[int, list] = {}
    for c in cos:
        groups.setdefault(keys[c[0]], []).append(c)
    out = []
    for k, g in sorted(groups.items()):
        if len(g) < 2:
            continue
        g.sort(key=lambda c: (c[3], c[0]))
        surv = g[0]
        ps = close.get(surv[0]) or surv[4]
        for c in g[1:]:
            pc = close.get(c[0]) or c[4]
            ratio = pc / ps
            hs = [h for h in holdings if h[0] == c[0]]
            frac_cash = sum((h[2] * ratio - math.floor(h[2] * ratio)) * ps for h in hs)
            out.append({"series": k, "survivor": {"id": surv[0], "code": surv[1], "title": surv[2], "close": ps},
                        "merged": {"id": c[0], "code": c[1], "title": c[2], "close": pc, "shares": c[5]},
                        "ratio": round(ratio, 6), "holders": len(hs),
                        "players": sum(1 for h in hs if h[3] == "player"),
                        "new_shares": sum(math.floor(h[2] * ratio) for h in hs),
                        "fraction_cash_cents": round(frac_cash)})
    print(f"{len(cos)} listed companies, {len(groups)} series; relations missing for {len(missing)}"
          + (" (run with --fetch)" if missing and not a.fetch else ""))
    if not out:
        print("no listed companies share a series")
    for x in out:
        s, m = x["survivor"], x["merged"]
        print(f"{m['code']} {m['title']} -> {s['code']} {s['title']}: 1 share = {x['ratio']:.4f} shares "
              f"(closes {m['close'] / 100:.2f} / {s['close'] / 100:.2f}); {x['holders']} holders ({x['players']} players), "
              f"{x['new_shares']} new shares, fractions {x['fraction_cash_cents'] / 100:.2f} in cash")
    if a.json:
        a.json.write_text(json.dumps({"missing_relations": missing, "swaps": out}, ensure_ascii=False, indent=1),
                          encoding="utf-8")


if __name__ == "__main__":
    main()
