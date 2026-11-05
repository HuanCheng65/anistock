"""Seed a game.db (and a tiny collector db) for the HTTP API difftest, using v1's own code.

    uv run python backend/difftest/api_setup.py GAME.db COLLECTOR.db MATCHES.json [--secret-in-db]

Starts from `setup.py` (accounts, companies, holdings, lots) and adds what the read endpoints show:
state rows, password hashes for some players (others keep pw_hash NULL), company accounts with cash and
an IPO sell order, IPO subscriptions (open and allotted), candles (live and replay), company_days over
three weeks, macro_days, news, dividends, resting orders that cross (so the first order of the day runs
the opening auction), a cooling-off period, and yesterday's trades.
"""

import hashlib
import json
import sqlite3
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1]))

import setup  # noqa: E402
from server import db as gdb  # noqa: E402
from server import exchange as ex  # noqa: E402

YDAY, YTS = "2026-11-04", "2026-11-04T15:00:00+08:00"
FEES, RULES = ex.Fees(), ex.Rules()


def hash_pw(pw: str, salt: bytes) -> str:          # v1 app.hash_pw with a fixed salt
    return salt.hex() + ":" + hashlib.scrypt(pw.encode(), salt=salt, n=2 ** 14, r=8, p=1).hex()


def main(out: str, collector: str, matches: str, secret_in_db: bool) -> None:
    meta = Path(out).with_suffix(".meta.json")
    setup.main(out, str(meta), 7)
    c = gdb.connect(out)
    c.execute("BEGIN")
    world = gdb.special(c, "world")
    players = json.loads(meta.read_text())["players"]
    for k, v in (("rate", "0.0315"), ("last_closed", YDAY), ("launched_on", "2026-10-01"), ("a_ref", "1.25"),
                 ("window_start", "2026-07-01")):
        gdb.set_state(c, k, v)
    if secret_in_db:
        gdb.set_state(c, "session_secret", "5e" * 32)
    for i, pid in enumerate(players[:5]):
        c.execute("UPDATE accounts SET pw_hash=? WHERE id=?", (hash_pw(f"secret{i}", bytes([i + 1]) * 16), pid))
    c.execute("UPDATE accounts SET username='老玩家' WHERE id=?", (players[5],))

    # company accounts: cash (book value) and, for 101, an IPO sell order still open
    for cid in (101, 102, 106):
        acct = c.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('company', ?, ?, ?)",
                         (f"co{cid}", cid, YTS)).lastrowid
        ex.transfer(c, world, acct, 123457 * (cid - 100), "ipo", day=YDAY, ts=YTS, company_id=cid)
        if cid == 101:
            holder = c.execute("SELECT id FROM accounts WHERE kind='committee' AND company_id=101").fetchone()[0]
            ex.remove_shares(c, holder, 101, 300)
            ex.add_shares(c, acct, 101, 300, 500, YDAY)
            ex.place_order(c, account_id=acct, company_id=101, side="sell", kind="limit", qty=300, price=510,
                           day=YDAY, ts=YTS, fees=FEES, rules=RULES, match=False)

    # IPOs: 106 subscribing now, 104 allotted last week (and listed recently: "new")
    c.execute("UPDATE companies SET sub_start='2026-11-03', sub_end='2026-11-06', listed_on='2026-11-07', ipo_price=400, "
              "title_ja='サブスク', title_en='Subscribing', studio='Studio X', episodes=12 WHERE id=106")
    c.execute("UPDATE companies SET sub_start='2026-10-28', sub_end='2026-10-30', listed_on='2026-11-01', "
              "episodes=NULL, long_runner=1 WHERE id=104")
    c.execute("UPDATE companies SET title_ja='葬送のフリーレン', title_en='Frieren', studio='Madhouse', episodes=28 WHERE id=101")
    c.execute("""INSERT INTO companies(id, code, title, first_air, listed_on, status, sub_start, sub_end, shares,
                 ipo_price, prev_close, last_price) VALUES (107, '100099', '未来认购', '2026-11-12', '2026-11-11',
                 'subscribing', '2026-11-08', '2026-11-10', 1000, 250, 250, 250)""")
    for pid, q in ((players[0], 100), (players[1], 250)):
        frozen = FEES.buy_reserve(400, q)
        c.execute("UPDATE accounts SET cash_frozen=cash_frozen+? WHERE id=?", (frozen, pid))
        c.execute("INSERT INTO ipo_applications(company_id, account_id, qty, frozen, created_at) VALUES (106,?,?,?,?)",
                  (pid, q, frozen, YTS))
    for pid, q, a in ((players[0], 300, 120), (players[2], 50, 50)):
        c.execute("INSERT INTO ipo_applications(company_id, account_id, qty, frozen, allotted, created_at) "
                  "VALUES (104,?,?,0,?,?)", (pid, q, a, "2026-10-29T12:00:00+08:00"))

    # history: candles (replay before launch, live after), company_days, macro_days, news, dividends
    for cid, base in ((101, 500), (102, 1234), (104, 88)):
        for d in range(15, 32 + 4):
            day = f"2026-10-{d:02d}" if d <= 31 else f"2026-11-{d - 31:02d}"
            o = base + (d * 7) % 23 - 11
            c.execute("INSERT INTO candles VALUES (?,?,?,?,?,?,?,?)",
                      (cid, day, o, o + 9, o - 8, o + (d % 5) - 2, 1000 + d * 13, "replay" if d < 20 else "live"))
            rev = 50000 + (d * 997 * cid) % 40000
            c.execute("INSERT INTO company_days VALUES (?,?,?,?,?,?,?,?)",
                      (cid, day, rev, rev // 2, rev // 3, rev - rev // 2 - rev // 3, (d - 14) // 7, 3 if d % 7 == 0 else 0))
    for i, d in enumerate(("2026-11-02", "2026-11-03", "2026-11-04")):
        c.execute("INSERT INTO macro_days VALUES (?,?,?,?,?,?,?,?)",
                  (d, 0.03 + i * 0.0005, 123456789 + i * 1000, None if i == 0 else 0.001 * i, 5000 * i, 3333, 777, 12))
    for i, (cid, kind, title, body) in enumerate([
            (101, "premiere", "《葬送的芙莉莲》开播", "首播热度很高"), (None, "rate", "央行调整利率", None),
            (106, "ipo", "新股认购：认购中", "认购期 11-03 至 11-06"), (102, "dividend", "派息", "每股 0.03 币"),
            (None, "system", "欢迎", 'quote " and \\ and \n newline')]):
        c.execute("INSERT INTO news(ts, day, kind, company_id, title, body) VALUES (?,?,?,?,?,?)",
                  (f"2026-11-0{i + 1}T07:00:00+08:00", f"2026-11-0{i + 1}", kind, cid, title, body))
    ex.transfer(c, world, players[0], 1234, "dividend", day=YDAY, ts=YTS, company_id=101)
    ex.transfer(c, world, players[1], 99, "dividend", day=YDAY, ts=YTS, company_id=102)
    ex.transfer(c, players[0], world, 77, "dividend_tax", day=YDAY, ts=YTS, company_id=101)

    # yesterday's trades (continuous), then today's resting orders that cross at the opening auction
    def place(pid, cid, side, qty, price, day, ts, match):
        return ex.place_order(c, account_id=pid, company_id=cid, side=side, kind="limit", qty=qty, price=price,
                              day=day, ts=ts, fees=FEES, rules=RULES, match=match)

    def holders(cid):
        return [r[0] for r in c.execute("SELECT account_id FROM holdings h JOIN accounts a ON a.id=h.account_id "
                                        "WHERE h.company_id=? AND a.kind='player' AND qty-qty_frozen>=20 ORDER BY account_id",
                                        (cid,))]
    rich = [p for p in players if ex.available_cash(c, p) > 200000]
    for cid, pc in ((101, 500), (102, 1234)):
        sellers = holders(cid)
        for k in range(3):
            s = sellers[k % len(sellers)]
            b = next(p for p in rich if p != s)
            place(s, cid, "sell", 10, pc + k, YDAY, f"2026-11-04T14:{10 + k:02d}:00+08:00", True)
            place(b, cid, "buy", 10, pc + k, YDAY, f"2026-11-04T14:{10 + k:02d}:30+08:00", True)
    today_ts = "2026-11-05T08:50:00+08:00"
    for cid, pc in ((101, 500), (102, 1234), (104, 88)):
        sellers = holders(cid)
        s = sellers[0]
        b = next(p for p in rich if p != s)
        place(s, cid, "sell", 15, pc - 2, "2026-11-05", today_ts, False)
        place(b, cid, "buy", 20, pc + 3, "2026-11-05", today_ts, False)
        place(b, cid, "buy", 5, pc - 20, "2026-11-05", today_ts, False)
    # a cooling-off period that is still running at 10:00 (cleared by the opening auction)
    c.execute("UPDATE companies SET vcm_until='2026-11-05T10:03:00+08:00', vcm_low=1110, vcm_high=1357, vcm_count=1 "
              "WHERE id=102")
    c.execute("COMMIT")
    gdb.check_invariants(c)
    c.close()

    # collector db: AniList / Bangumi / Annict snapshots for 101 (and a broken one for 102)
    Path(collector).unlink(missing_ok=True)
    k = sqlite3.connect(collector)
    k.execute("""CREATE TABLE snapshots (source TEXT NOT NULL, kind TEXT NOT NULL, external_id TEXT NOT NULL,
                 day TEXT NOT NULL, fetched_at TEXT NOT NULL, data TEXT NOT NULL,
                 PRIMARY KEY (source, kind, external_id, day))""")
    rows = [
        ("anilist", "media", "101", "2026-11-03", {"popularity": 1, "averageScore": 50}),
        ("anilist", "media", "101", "2026-11-04", {"popularity": 123456, "averageScore": 91,
                                                    "nextAiringEpisode": {"episode": 9, "airingAt": 1762300000},
                                                    "genres": ["Adventure", "Drama", "Fantasy", "Slice of Life", "X"],
                                                    "coverImage": {"large": "https://example.org/c.jpg"}}),
        ("bgm", "subject", "400602", "2026-11-04", {"collection": {"wish": 10, "doing": 2000}, "rating": {"score": 9.1},
                                                     "name_cn": "葬送的芙莉莲"}),
        ("annict", "work", "9999", "2026-11-04", {"watchersCount": 4321}),
        ("anilist", "media", "102", "2026-11-04", {"popularity": 5, "genres": None}),
        ("anilist", "media", "104", "2026-11-04", {"popularity": 7, "averageScore": 1.5e-07, "coverImage": None}),
    ]
    for src, kind, eid, day, data in rows:
        k.execute("INSERT INTO snapshots VALUES (?,?,?,?,?,?)", (src, kind, eid, day, day + "T06:00:00+08:00",
                                                                 json.dumps(data, ensure_ascii=False)))
    k.commit()
    k.close()
    Path(matches).write_text(json.dumps({"matches": {"101": {"anilist": 101, "bgm": 400602, "annict": 9999},
                                                     "104": {"anilist": 104, "bgm": 0}}}))


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    main(*args, secret_in_db="--secret-in-db" in sys.argv)
