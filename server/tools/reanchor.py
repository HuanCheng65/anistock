"""Before the first trading day of a new game: open every stock at the Rust server's valuation.

    python -m server.tools.reanchor GAME_DB GOLIVE_DB

server.seed prices the opening at the v1 (Python) valuation, but the server starts its own filtered valuation
at go-live (`valuation_go_live`) and does not move prices, so a fresh game opened far from what the market makers
and the value agents quote and repriced over the first weeks (the 2026-11 launch: median value/price 2.27).

GOLIVE_DB is a copy of GAME_DB on which the server has started once (point ANISTOCK_GAME_DB at the copy, another
ANISTOCK_PORT, stop it after "valuation: started the filter from the inputs history"): its fair values are what
the server will compute again on GAME_DB, the same data. Here every listed stock's opening price (prev_close,
last_price, open_ref) becomes that value, and its pre-launch candles, IPO price and lot costs are scaled by the
same factor (the rule server.seed uses), so the K-line joins the opening price without a gap. The last replay
close is set to the value exactly. Refuses a game that has traded (live candles).
"""

from __future__ import annotations

import sqlite3
import statistics as st
import sys


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__.split("\n\n")[1])
    game, golive = sys.argv[1], sys.argv[2]
    src = sqlite3.connect(f"file:{golive}?mode=ro", uri=True)
    fv = {i: f for i, f in src.execute("SELECT id, fair_value FROM companies WHERE status='listed' AND fair_value > 0")}
    src.close()
    if not fv:
        sys.exit(f"{golive} has no fair values: start the server on it once first")

    conn = sqlite3.connect(game, isolation_level=None)
    launched = conn.execute("SELECT value FROM state WHERE key='launched_on'").fetchone()[0]
    live = conn.execute("SELECT COUNT(*) FROM candles WHERE source != 'replay'").fetchone()[0]
    if live:
        sys.exit(f"{game} has {live} live candles: not a fresh game")
    conn.execute("BEGIN IMMEDIATE")
    ks, skipped = [], []
    for cid, last in conn.execute("SELECT id, last_price FROM companies WHERE status='listed'").fetchall():
        if cid not in fv:
            skipped.append(cid)
            continue
        k = fv[cid] / max(1, last)
        ks.append(k)
        conn.execute("""UPDATE candles SET open=MAX(1, ROUND(open*?)), high=MAX(1, ROUND(high*?)), low=MAX(1, ROUND(low*?)),
                        close=MAX(1, ROUND(close*?)) WHERE company_id=? AND source='replay'""", (k, k, k, k, cid))
        conn.execute("""UPDATE candles SET close=?, high=MAX(high, ?), low=MIN(low, ?) WHERE company_id=? AND source='replay'
                        AND day=(SELECT MAX(day) FROM candles WHERE company_id=? AND source='replay')""",
                     (fv[cid], fv[cid], fv[cid], cid, cid))
        conn.execute("UPDATE companies SET ipo_price=MAX(1, ROUND(ipo_price*?)) WHERE id=?", (k, cid))
        conn.execute("UPDATE lots SET cost=MAX(1, ROUND(cost*?)) WHERE company_id=?", (k, cid))
        conn.execute("UPDATE companies SET prev_close=?, last_price=?, fair_value=?, open_ref=? WHERE id=?",
                     (fv[cid], fv[cid], fv[cid], fv[cid], cid))
    conn.execute("COMMIT")
    ks.sort()
    print(f"opening {launched}: {len(ks)} stocks at the server's value; scale median {st.median(ks):.2f}, "
          f"range {ks[0]:.2f}..{ks[-1]:.2f}" + (f"; no value for {skipped}" if skipped else ""))


if __name__ == "__main__":
    main()
