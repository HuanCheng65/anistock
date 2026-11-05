"""Compare two game databases table by table and two result files line by line.

    python backend/difftest/compare.py A.db B.db [A.results B.results [OPS.jsonl]]

Exit status 0 if identical. Prints the first differences per table. `holdings` is also compared on its
hidden rowid (the engine replays v1's delete / re-insert sequence, so even those agree).
"""

import json
import sqlite3
import sys

TABLES = {
    "accounts": "id", "companies": "id", "holdings": "account_id, company_id", "lots": "id", "orders": "id",
    "trades": "id", "ledger": "id", "news": "id", "state": "key", "candles": "company_id, day",
    "ipo_applications": "company_id, account_id", "company_days": "company_id, day", "macro_days": "day",
}
EXTRA = {"holdings": "rowid AS _rowid, "}


def rows(path, table):
    c = sqlite3.connect(path)
    cur = c.execute(f"SELECT {EXTRA.get(table, '')}* FROM {table} ORDER BY {TABLES[table]}")
    cols = [d[0] for d in cur.description]
    return cols, cur.fetchall()


def main(a, b, ra=None, rb=None, ops=None) -> int:
    bad = 0
    for t in TABLES:
        ca, xa = rows(a, t)
        cb, xb = rows(b, t)
        if ca != cb:
            print(f"{t}: columns differ {ca} vs {cb}")
            bad += 1
            continue
        if xa == xb:
            print(f"  {t:17s} {len(xa):7d} rows identical")
            continue
        bad += 1
        print(f"{t}: {len(xa)} vs {len(xb)} rows")
        shown = 0
        for i in range(max(len(xa), len(xb))):
            ra_ = xa[i] if i < len(xa) else None
            rb_ = xb[i] if i < len(xb) else None
            if ra_ != rb_:
                print(f"   python: {dict(zip(ca, ra_)) if ra_ else None}")
                print(f"   rust:   {dict(zip(cb, rb_)) if rb_ else None}")
                shown += 1
                if shown >= 3:
                    break
    if ra:
        la = [json.loads(x) for x in open(ra)]
        lb = [json.loads(x) for x in open(rb)]
        opl = open(ops).read().splitlines() if ops else []
        diffs = [i for i in range(max(len(la), len(lb))) if (la[i:i + 1] or [None]) != (lb[i:i + 1] or [None])]
        if diffs:
            bad += 1
            print(f"results: {len(diffs)} of {len(la)} operations differ; first:")
            for i in diffs[:3]:
                print(f"   op #{i}: {opl[i] if i < len(opl) else ''}")
                print(f"   python: {la[i] if i < len(la) else None}")
                print(f"   rust:   {lb[i] if i < len(lb) else None}")
        else:
            print(f"  results           {len(la):7d} ops identical")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:]))
