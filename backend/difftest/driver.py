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

_CFG = None


def cfg():
    """Game config (only the close and NPC ops need it; importing server.daily pulls in the model)."""
    global _CFG
    if _CFG is None:
        from server.config import load
        _CFG = load()
    return _CFG

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


def _withdraw(c, company_id: int, account_id: int) -> None:      # v1 app._withdraw
    old = c.execute("SELECT frozen FROM ipo_applications WHERE company_id=? AND account_id=?", (company_id, account_id)).fetchone()
    if old:
        c.execute("UPDATE accounts SET cash_frozen = cash_frozen - ? WHERE id=?", (old["frozen"], account_id))
        c.execute("DELETE FROM ipo_applications WHERE company_id=? AND account_id=?", (company_id, account_id))


def subscribe(c, account_id, company_id, qty, today, ts):      # v1 app.subscribe (HTTPException -> OrderError)
    co = c.execute("SELECT * FROM companies WHERE id=?", (company_id,)).fetchone()
    if co is None or co["status"] != "subscribing":
        raise ex.OrderError("这支新股不在认购期")
    if not (co["sub_start"] <= today <= co["sub_end"]):
        raise ex.OrderError(f"认购时间是 {co['sub_start']} 至 {co['sub_end']}")
    _withdraw(c, company_id, account_id)
    frozen = 0
    if qty > 0:
        frozen = FEES.buy_reserve(co["ipo_price"], qty)
        if ex.available_cash(c, account_id) < frozen:
            raise ex.OrderError(f"可用资金不足：认购 {qty:,} 股需要冻结 {frozen / 100:,.2f} 币")
        c.execute("UPDATE accounts SET cash_frozen = cash_frozen + ? WHERE id=?", (frozen, account_id))
        c.execute("INSERT INTO ipo_applications(company_id, account_id, qty, frozen, created_at) VALUES (?,?,?,?,?)",
                  (company_id, account_id, qty, frozen, ts))
    return {"ok": True, "qty": qty, "frozen": frozen}


def unsubscribe(c, account_id, company_id):                    # v1 app.unsubscribe
    co = c.execute("SELECT status FROM companies WHERE id=?", (company_id,)).fetchone()
    if co is None or co["status"] != "subscribing":
        raise ex.OrderError("认购已经截止，不能撤回")
    _withdraw(c, company_id, account_id)
    return {"ok": True}


def subscribing(c, idx):
    ids = [r[0] for r in c.execute("SELECT id FROM companies WHERE status='subscribing' ORDER BY id")]
    return ids[idx % len(ids)] if ids else 999999999


def run_close(c, day, ts, inputs_path):
    from server import daily
    try:
        res = daily.run_day(c, cfg(), day, ts, inputs=json.load(open(inputs_path, encoding="utf-8")))
    except Exception as e:                     # run_day rolled back its own transaction
        print(f"python: close {day} failed: {e!r}")
        return {"error": "close failed"}
    return res


def main(db_path: str, ops_path: str, out_path: str) -> None:
    c = gdb.connect(db_path)
    placed: list[tuple[int, int]] = []     # (order_id, account_id) of every accepted order
    rng = None
    out = open(out_path, "w")
    c.execute("BEGIN")
    for line in open(ops_path):
        op = json.loads(line)
        k = op["op"]
        if k in ("close", "catch_up"):           # run_day manages its own transaction
            c.execute("COMMIT")
            if k == "close":
                res = run_close(c, op["day"], op["ts"], op["inputs"])
            else:                                 # v1 app.catch_up (without the backup)
                from datetime import date as _d, timedelta as _td
                res = []
                d = _d.fromisoformat(gdb.get_state(c, "last_closed")) + _td(days=1)
                y = _d.fromisoformat(op["today"]) - _td(days=1)
                while d <= y:
                    r = run_close(c, d.isoformat(), op["ts"], f"{op['inputs']}/{d.isoformat()}.json")
                    res.append(r)
                    if "error" in r:
                        break
                    d += _td(days=1)
            c.execute("BEGIN")
            out.write(json.dumps(res, ensure_ascii=False) + "\n")
            continue
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
            elif k == "rng":
                import random
                rng = random.Random(op["seed"])
                res = {"ok": True}
            elif k == "npc_tick":
                from server import npc
                npc.tick(c, cfg(), op["day"], op["ts"], rng, auction=op["auction"])
                res = {"ok": True}
            elif k == "place_rel":
                ids = [r[0] for r in c.execute("SELECT id FROM companies WHERE status='listed' ORDER BY id")]
                co = ids[op["co_idx"] % len(ids)]
                last = c.execute("SELECT last_price FROM companies WHERE id=?", (co,)).fetchone()[0]
                price = max(1, round(last * op["rel"])) if op["kind"] == "limit" else None
                res = ex.place_order(c, account_id=op["acct"], company_id=co, side=op["side"], kind=op["kind"],
                                     qty=op["qty"], price=price, day=op["day"], ts=op["ts"], fees=FEES,
                                     rules=RULES, match=op["match"])
                placed.append((res["order_id"], op["acct"]))
            elif k == "subscribe":
                res = subscribe(c, op["acct"], subscribing(c, op["idx"]), op["qty"], op["today"], op["ts"])
            elif k == "unsubscribe":
                res = unsubscribe(c, op["acct"], subscribing(c, op["idx"]))
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
