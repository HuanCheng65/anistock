"""Apply a difftest operation file to a game.db with v1's server/exchange.py.

    uv run python backend/difftest/driver.py GAME.db OPS.jsonl RESULTS.jsonl

Each operation runs in its own savepoint and is rolled back if it raises, like the v1 API's transaction
per request. The 'auction' op is v1 `app.ensure_opening_auction` (copied: importing server.app would start
the whole server).
"""

import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from server import db as gdb  # noqa: E402
from server import exchange as ex  # noqa: E402

FEES = ex.Fees()
RULES = ex.Rules()


def ensure_opening_auction(c, today: str, ts: str) -> None:
    if gdb.get_state(c, "auction_day") == today:
        return
    for co in c.execute("SELECT * FROM companies WHERE status='listed'").fetchall():
        price = ex.run_auction(c, co["id"], day=today, ts=ts, fees=FEES)
        c.execute("UPDATE companies SET open_ref=?, vcm_until=NULL, vcm_count=0 WHERE id=?",
                  (price or co["prev_close"], co["id"]))
    gdb.set_state(c, "auction_day", today)


def edge_price(c, op) -> int:
    co = c.execute("SELECT * FROM companies WHERE id=?", (op["co"],)).fetchone()
    if co is None:
        return 100
    what, end = op["edge"].split("_")
    if what == "band" and co["vcm_low"] is not None:
        return (co["vcm_high"] if end == "hi" else co["vcm_low"]) + op["off"]
    if what == "last":
        ref, thr = co["last_price"], RULES.vcm_threshold
    else:
        ref, thr = (co["open_ref"] or co["prev_close"]), RULES.vcm_threshold * (co["vcm_count"] + 1)
    if end == "hi":
        return math.floor(ref * (1 + thr) + 1e-9) + op["off"]
    return math.ceil(ref * (1 - thr) - 1e-9) + op["off"]


def main(db_path: str, ops_path: str, out_path: str) -> None:
    c = gdb.connect(db_path)
    placed: list[tuple[int, int]] = []     # (order_id, account_id) of every accepted order
    out = open(out_path, "w")
    c.execute("BEGIN")
    for line in open(ops_path):
        op = json.loads(line)
        k = op["op"]
        c.execute("SAVEPOINT op")
        try:
            if k == "place":
                price = edge_price(c, op) if "edge" in op else op["price"]
                res = ex.place_order(c, account_id=op["acct"], company_id=op["co"], side=op["side"], kind=op["kind"],
                                     qty=op["qty"], price=price, day=op["day"], ts=op["ts"], fees=FEES,
                                     rules=RULES, match=op["match"])
                placed.append((res["order_id"], op["acct"]))
            elif k in ("cancel", "cancel_raw"):
                if k == "cancel":
                    if not placed:
                        oid, owner = 10**9, op["acct"]
                    else:
                        pool = placed[-20:] if op["recent"] else placed
                        oid, owner = pool[op["idx"] % len(pool)]
                    acct = owner if op["own"] else op["acct"]
                else:
                    oid, acct = op["order_id"], op["acct"]
                ex.cancel_order(c, account_id=acct, order_id=oid, ts=op["ts"])
                res = {"ok": True, "order_id": oid}
            elif k == "cancel_all":
                for o in c.execute("SELECT * FROM orders WHERE account_id=? AND company_id=? AND status='open'",
                                   (op["acct"], op["co"])).fetchall():
                    ex._close(c, o, "cancelled", op["ts"])
                res = {"ok": True}
            elif k == "restart":
                res = {"ok": True}
            elif k == "auction":
                ensure_opening_auction(c, op["day"], op["ts"])
                res = {"ok": True}
            elif k == "transfer":
                amount = op["raw"] if op["raw"] is not None else int(ex.available_cash(c, op["from"]) * op["frac"])
                ex.transfer(c, op["from"], op["to"], amount, op["reason"], day=op["day"], ts=op["ts"])
                res = {"ok": True}
            elif k == "move_shares":
                q = int(ex.available_shares(c, op["from"], op["co"]) * op["frac"])
                taken = ex.remove_shares(c, op["from"], op["co"], q) if q > 0 else []
                for tq, cost, _ in taken:
                    ex.add_shares(c, op["to"], op["co"], tq, cost, op["day"])
                res = {"taken": [list(t) for t in taken]}
            elif k == "remove_too_many":
                r = c.execute("SELECT qty FROM holdings WHERE account_id=? AND company_id=?",
                              (op["acct"], op["co"])).fetchone()
                ex.remove_shares(c, op["acct"], op["co"], (r[0] if r else 0) + 1)
                res = {"ok": True}
            else:
                raise SystemExit(f"unknown op {k}")
            c.execute("RELEASE op")
        except (ex.OrderError, RuntimeError) as e:
            c.execute("ROLLBACK TO op")
            c.execute("RELEASE op")
            res = {"error": str(e)}
        out.write(json.dumps(res, ensure_ascii=False) + "\n")
    c.execute("COMMIT")
    out.close()
    try:
        gdb.check_invariants(c)
        print("python: invariants ok")
    except AssertionError as e:
        print(f"python: INVARIANTS FAILED: {e}")


if __name__ == "__main__":
    main(*sys.argv[1:4])
