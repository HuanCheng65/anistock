"""The state of heat futures after a close, on one screen (docs/design/futures.md §4): what to check in the first days.

    uv run python -m server.tools.futures_report [--game data/game.db] [--day YYYY-MM-DD] [--contracts 20]

Read-only. Sections:
- the heat index: the last days published into the game database and how many works each (a day the inputs
  did not carry is a gap: contracts cannot settle past it);
- contracts: listed / expiring / settled; contracts waiting for an index past their last day (stuck);
- the day's settlements: the price source of each (vwap: the last 30 minutes' trades; mid: the maker's quotes;
  prev: neither, the previous settlement kept) and the settlement against the published average so far;
- money: the market maker's cash against its funding, the futures NPCs, players' marking to market, commission,
  defaults (futures_default: losses world covered);
- players: who holds positions, open interest by kind of account, the largest positions.
"""

from __future__ import annotations

import argparse
import sqlite3
import sys

from server.config import load


def yuan(cents: int | float | None) -> str:
    return "-" if cents is None else f"{cents / 100:,.0f}"


def pts(ticks: int | float | None) -> str:
    return "-" if ticks is None else f"{ticks / 100:.2f}"


def section(title: str) -> None:
    print(f"\n== {title}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--game")
    ap.add_argument("--day", help="the trading day to report (default: the last closed)")
    ap.add_argument("--contracts", type=int, default=20, help="contracts listed in detail")
    args = ap.parse_args()
    db = sqlite3.connect(f"file:{args.game or load().game_db}?mode=ro", uri=True)
    q = lambda sql, *p: db.execute(sql, p).fetchall()  # noqa: E731
    one = lambda sql, *p: db.execute(sql, p).fetchone()  # noqa: E731
    if not one("SELECT 1 FROM sqlite_master WHERE name='futures_contracts'"):
        print("no futures tables: the server has not run V19 yet")
        return 1
    day = args.day or (one("SELECT value FROM state WHERE key='last_closed'") or [None])[0]
    print(f"heat futures report, trading day {day}")

    section("heat index (game database)")
    for d, n, src in q("SELECT day, COUNT(*), GROUP_CONCAT(DISTINCT version) FROM heat_index GROUP BY day ORDER BY day DESC LIMIT 5"):
        print(f"  {d}  {n:4d} works  {src}")
    gaps = q("""SELECT c.code, c.first_day, c.last_day,
                  (SELECT COUNT(*) FROM heat_index h WHERE h.work_id=c.work_id AND h.version NOT LIKE '%backfill' AND h.day BETWEEN c.first_day AND MIN(c.last_day, ?)),
                  julianday(MIN(c.last_day, ?)) - julianday(c.first_day) + 1
                FROM futures_contracts c WHERE c.status != 'settled' AND c.first_day <= ?""", day, day, day)
    missing = [(code, have, int(want)) for code, _, _, have, want in gaps if have < want]
    print(f"  contracts with a day of their period not published: {len(missing)}"
          + ("".join(f"\n    {code}: {have}/{want} days" for code, have, want in missing[:10])))

    section("contracts")
    for st, n in q("SELECT status, COUNT(*) FROM futures_contracts GROUP BY status"):
        print(f"  {st:9s} {n}")
    stuck = q("SELECT code, last_day FROM futures_contracts WHERE status='expiring' AND last_day < ?", day)
    print(f"  stuck (expiring, last day before {day}): {len(stuck)}" + "".join(f"\n    {c} last day {d}" for c, d in stuck))
    listed_today = q("SELECT code FROM futures_contracts WHERE listed_on=?", day)
    print(f"  listed at this close: {len(listed_today)}")

    section(f"settlements of {day}")
    rows = q("""SELECT c.code, co.title, c.period, d.settle, d.settle_source, d.volume, d.open_interest, c.first_day, c.last_day, c.work_id
                FROM futures_days d JOIN futures_contracts c ON c.id=d.contract_id JOIN companies co ON co.id=c.company_id
                WHERE d.day=? ORDER BY d.volume DESC""", day)
    by_src: dict[str, int] = {}
    for r in rows:
        by_src[r[4]] = by_src.get(r[4], 0) + 1
    print("  sources: " + (", ".join(f"{k} {v}" for k, v in sorted(by_src.items())) or "none"))
    print(f"  {'contract':10s} {'work':16s} {'k':>2s} {'settle':>8s} {'src':5s} {'avg so far':>10s} {'days':>5s} {'vol':>5s} {'OI':>5s}")
    for code, title, k, settle, src, vol, oi, first, last, work in rows[: args.contracts]:
        avg, n = one("SELECT AVG(idx), COUNT(*) FROM heat_index WHERE work_id=? AND version NOT LIKE '%backfill' AND day BETWEEN ? AND MIN(?, ?)",
                     work, first, last, day)
        print(f"  {code:10s} {title[:14]:16s} {k:2d} {pts(settle):>8s} {src:5s} {('-' if avg is None else f'{avg:.2f}'):>10s} {n:5d} {vol:5d} {oi:5d}")

    section("money")
    fmm = one("SELECT id, cash, cash_frozen FROM accounts WHERE kind='fmm'")
    flow = lambda reason, to=None, frm=None: one(  # noqa: E731
        "SELECT COALESCE(SUM(amount),0) FROM ledger WHERE reason=?" + (" AND to_id=?" if to else "") + (" AND from_id=?" if frm else ""),
        *([reason] + ([to] if to else []) + ([frm] if frm else [])))[0]
    if fmm:
        funded = flow("futures_mm_fund", to=fmm[0]) - flow("futures_mm_return", frm=fmm[0])
        print(f"  market maker: cash {yuan(fmm[1])} (frozen {yuan(fmm[2])}), funded {yuan(funded)}, "
              f"so far {yuan(fmm[1] - funded)} (before unsettled variation)")
    else:
        print("  market maker: no account yet")
    for name, cash, frozen in q("SELECT name, cash, cash_frozen FROM accounts WHERE kind='fnpc' ORDER BY name"):
        print(f"  {name:12s} cash {yuan(cash):>12s} frozen {yuan(frozen):>10s}")
    players = "SELECT id FROM accounts WHERE kind='player'"
    paid = one(f"SELECT COALESCE(SUM(amount),0) FROM ledger WHERE reason='futures_mtm' AND from_id IN ({players})")[0]
    got = one(f"SELECT COALESCE(SUM(amount),0) FROM ledger WHERE reason='futures_mtm' AND to_id IN ({players})")[0]
    fee = one(f"SELECT COALESCE(SUM(amount),0) FROM ledger WHERE reason='futures_commission' AND from_id IN ({players})")[0]
    print(f"  players: marked to market {yuan(got - paid)} (received {yuan(got)}, paid {yuan(paid)}), commission {yuan(fee)}")
    print(f"  defaults covered by world (futures_default): {yuan(flow('futures_default'))}")

    section("positions")
    for kind, n, lots in q("""SELECT a.kind, COUNT(DISTINCT p.account_id), SUM(ABS(p.qty)) FROM futures_positions p
                               JOIN accounts a ON a.id=p.account_id WHERE p.qty != 0 GROUP BY a.kind ORDER BY 3 DESC"""):
        print(f"  {kind:10s} {n:4d} accounts  {lots:6d} lots (both sides)")
    top = q("""SELECT a.username, c.code, p.qty, p.cost, c.last_price, c.tick_value, p.margin FROM futures_positions p
               JOIN accounts a ON a.id=p.account_id JOIN futures_contracts c ON c.id=p.contract_id
               WHERE a.kind='player' AND p.qty != 0 ORDER BY ABS(p.qty) * c.last_price DESC LIMIT 10""")
    for user, code, qty, cost, last, tick, margin in top:
        pnl = (qty * last - cost) * tick
        print(f"  {user[:14]:14s} {code:10s} {qty:+5d} lots at {pts(cost / qty if qty else None)}  pnl {yuan(pnl):>8s}  margin {yuan(margin)}")
    calls = q("SELECT status, COUNT(*) FROM credit_accounts WHERE status != 'normal' GROUP BY status")
    print("  credit accounts not normal: " + (", ".join(f"{s} {n}" for s, n in calls) or "none"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
