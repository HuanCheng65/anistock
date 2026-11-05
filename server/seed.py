"""Create a fresh game: replay the simulation up to yesterday so every stock opens with history.

    uv run python -m server.seed [--force]

Pre-launch candles and company revenue come from the simulation run with the game's own parameters
(marked source='replay'). Each company then starts with the rights holders (制作委员会) owning 75%
and its market maker holding the 25% float, priced at the replay's last close.
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import date, timedelta

from engine import daily_inputs as DI
from engine import economy as E

from . import db as gdb
from . import exchange as ex
from .config import ROOT, Clock, load
from .daily import create_company, revalue_all

sys.path.insert(0, str(ROOT / "sim"))
import simulate as SIM  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true", help="delete an existing game database first")
    args = ap.parse_args()
    cfg = load()
    if cfg.game_db.exists():
        if not args.force:
            sys.exit(f"{cfg.game_db} already exists; use --force to start over (this deletes every player)")
        for suffix in ("", "-wal", "-shm"):
            p = cfg.game_db.with_name(cfg.game_db.name + suffix)
            if p.exists():
                p.unlink()
    cfg.game_db.parent.mkdir(parents=True, exist_ok=True)

    p = cfg.params
    start = date.fromisoformat(p["window"]["start"])
    shows, end = E.load_shows(cfg.collector_db, p, start, None)
    mpath = ROOT / "data" / "derived" / "markets.json"
    markets = {int(k): v for k, v in json.loads(mpath.read_text(encoding="utf-8"))["series"].items()}
    aud = E.load_audiences(cfg.collector_db)
    print(f"replaying {start}..{end} with {len(shows)} shows")
    res = SIM.simulate(shows, start, end, p, markets, aud)
    by_id = {s.id: s for s in shows}
    # the model's view of the launch day, in the same form the daily close reads
    inp = DI.parse(json.loads(DI.dumps(DI.build(shows, markets, aud, E.load_cn_titles(cfg.collector_db), end, p, start))))

    # the demand index reference: AniList attention of the first week, as the simulation computes it
    rv = p["revenue"]
    a_ref = sum(sum(E.attention(s, start + timedelta(days=k), rv) for s in shows if s.list_day <= start + timedelta(days=k))
                for k in range(7)) / 7 or 1.0

    gdb.init(cfg.game_db)
    conn = gdb.connect(cfg.game_db)
    clock = Clock(cfg.tz)
    ts = clock.ts()
    day = end.isoformat()
    conn.execute("BEGIN")
    for kind, name in (("world", "外部世界"), ("cb", "中央银行"), ("gov", "财政")):
        conn.execute("INSERT INTO accounts(kind, name, created_at) VALUES (?,?,?)", (kind, name, ts))
    world = gdb.special(conn, "world")
    for i in range(cfg.npc["noise_traders"]):
        nid = conn.execute("INSERT INTO accounts(kind, name, created_at) VALUES ('noise', ?, ?)",
                           (f"散户{i + 1}", ts)).lastrowid
        ex.transfer(conn, world, nid, round(cfg.npc["noise_trader_cash"] * 100), "grant", day=day, ts=ts)

    gdb.set_state(conn, "window_start", start.isoformat())
    gdb.set_state(conn, "a_ref", a_ref)
    # the replay's rate came from the simulation's money stock, not the game's: start at neutral
    gdb.set_state(conn, "rate", p["central_bank"]["r_neutral"])
    gdb.set_state(conn, "launched_on", (end + timedelta(days=1)).isoformat())
    gdb.set_state(conn, "last_closed", day)

    n = 0
    for sid, rows in res["stocks"].items():
        sid = int(sid)
        if not rows or sid not in by_id:
            continue
        last = rows[-1]
        ipo = round(rows[0]["pc"] * 100)
        create_company(conn, cfg, inp.shows[sid], rows[0]["d"], day, ts, inp, [], res["macro"][-1]["rate"], 0,
                       ipo_price=ipo)
        # the company's cash (retained earnings + kept IPO money) carries over: it is part of its value
        book_cash = round(last.get("book", 0) * 100) * cfg.params["company"]["shares"]
        acct = conn.execute("SELECT id FROM accounts WHERE kind='company' AND company_id=?", (sid,)).fetchone()["id"]
        ex.transfer(conn, world, acct, book_cash, "grant", day=day, ts=ts, company_id=sid)
        close = max(1, round(last["c"] * 100))
        conn.execute("UPDATE companies SET prev_close=?, last_price=?, fair_value=? WHERE id=?",
                     (close, close, max(1, round(last["v"] * 100)), sid))
        for r in rows:
            conn.execute("""INSERT INTO candles(company_id, day, open, high, low, close, volume, source)
                            VALUES (?,?,?,?,?,?,0,'replay')""",
                         (sid, r["d"], round(r["o"] * 100), round(r["h"] * 100), round(r["l"] * 100), round(r["c"] * 100)))
            rm = [round(x * 100) for x in r.get("rm", [0, 0, 0])]
            conn.execute("""INSERT INTO company_days(company_id, day, revenue, rev_us, rev_cn, rev_jp, aired)
                            VALUES (?,?,?,?,?,?,?)""", (sid, r["d"], round(r["rev"] * 100), *rm, r["aired"]))
        n += 1
    # opening reference price: the game's own valuation, so the first day's limits are centred where the
    # market makers will quote (the replay's price path lags its values and used a different rate)
    revalue_all(conn, cfg, day, inp, p["central_bank"]["r_neutral"])
    # the replay's price path is a stand-in: scale each stock's whole replay (and its IPO price) so it
    # ends exactly at the opening price; shape and returns are unchanged, and the chart has no gap
    for c in conn.execute("SELECT id, fair_value, last_price FROM companies WHERE status='listed'").fetchall():
        k = c["fair_value"] / max(1, c["last_price"])
        conn.execute("""UPDATE candles SET open=MAX(1, ROUND(open*?)), high=MAX(1, ROUND(high*?)), low=MAX(1, ROUND(low*?)),
                        close=MAX(1, ROUND(close*?)) WHERE company_id=? AND source='replay'""", (k, k, k, k, c["id"]))
        conn.execute("UPDATE companies SET ipo_price=MAX(1, ROUND(ipo_price*?)) WHERE id=?", (k, c["id"]))
        conn.execute("UPDATE lots SET cost=MAX(1, ROUND(cost*?)) WHERE company_id=?", (k, c["id"]))
    conn.execute("UPDATE companies SET prev_close = fair_value, last_price = fair_value WHERE status='listed'")
    conn.execute("INSERT INTO news(ts, day, kind, title, body) VALUES (?,?, 'system', ?, ?)",
                 (ts, day, "安易开市",
                  f"{n} 支番剧股票开始交易。开市前的走势由 7 月以来的真实热度数据回放生成；"
                  "开市参考价按开市时的经营数据定价，回放走势已按比例接到这个价格上。"))
    gdb.check_invariants(conn)
    conn.execute("COMMIT")
    print(f"seeded {n} companies, last replay day {day}, policy rate {p['central_bank']['r_neutral']:.2%}")


if __name__ == "__main__":
    main()
