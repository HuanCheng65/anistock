"""Daily close pieces that don't need the collector: dividends with holding-period tax, order expiry."""

import pytest

from server import daily
from server import db as gdb
from server import exchange as ex
from server.config import load

TS = "2026-11-08T06:30:00+08:00"
DAY = "2026-11-08"   # a Sunday


@pytest.fixture
def conn():
    c = gdb.connect(":memory:")
    gdb.init(c)
    for kind in ("world", "cb", "gov"):
        c.execute("INSERT INTO accounts(kind, name, created_at) VALUES (?,?,?)", (kind, kind, TS))
    c.execute("""INSERT INTO companies(id, code, title, first_air, listed_on, status, shares, ipo_price, prev_close,
                 last_price, profit_week) VALUES (1, '100001', 'テスト', '2026-07-01', '2026-06-10', 'listed', 1000, 500, 500, 500, 0)""")
    for kind in ("company", "committee"):
        c.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES (?, ?, 1, ?)", (kind, kind, TS))
    return c


def acct(conn, kind):
    return conn.execute("SELECT id FROM accounts WHERE kind=?", (kind,)).fetchone()[0]


def test_dividend_tax_by_lot_age_and_founders_leave(conn):
    cfg = load()
    world, gov = acct(conn, "world"), acct(conn, "gov")
    company, committee = acct(conn, "company"), acct(conn, "committee")
    p = conn.execute("INSERT INTO accounts(kind, name, username, created_at) VALUES ('player','p','p',?)", (TS,)).lastrowid
    ex.add_shares(conn, committee, 1, 750, 500, "2026-06-10")
    ex.add_shares(conn, p, 1, 100, 500, "2026-06-20")    # held > 1 season: tax free
    ex.add_shares(conn, p, 1, 150, 500, "2026-11-05")    # held 3 days: 20%
    # a week of profit: 10,000 cents after tax -> 90% paid out = 9 cents a share
    ex.transfer(conn, world, company, 10_000, "revenue", day=DAY, ts=TS, company_id=1)
    conn.execute("UPDATE companies SET profit_week=10000 WHERE id=1")
    world_before = conn.execute("SELECT cash FROM accounts WHERE id=?", (world,)).fetchone()[0]
    summary = {"exported": 0, "taxes": 0}
    companies = [dict(r) for r in conn.execute("SELECT * FROM companies")]
    daily.pay_dividends(conn, cfg, DAY, TS, companies, summary)

    dps = 9
    gross_p = dps * 250
    tax = int(dps * 150 * 0.20)
    assert conn.execute("SELECT cash FROM accounts WHERE id=?", (p,)).fetchone()[0] == gross_p - tax
    assert conn.execute("SELECT cash FROM accounts WHERE id=?", (gov,)).fetchone()[0] == tax
    assert conn.execute("SELECT cash FROM accounts WHERE id=?", (world,)).fetchone()[0] == world_before + dps * 750
    assert summary["exported"] == dps * 750
    assert conn.execute("SELECT dividend_ps FROM company_days WHERE company_id=1 AND day=?", (DAY,)).fetchone()[0] == dps
    gdb.check_invariants(conn)


def test_holding_tax_schedule():
    cfg = load()
    assert daily.holding_tax_rate(cfg, "2026-11-05", "2026-11-08") == 0.20
    assert daily.holding_tax_rate(cfg, "2026-10-01", "2026-11-08") == 0.10
    assert daily.holding_tax_rate(cfg, "2026-06-01", "2026-11-08") == 0.0
