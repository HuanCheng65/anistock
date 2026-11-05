"""Daily close pieces that don't need the collector: dividends with holding-period tax, order expiry."""

import pytest

from server import daily
from server import db as gdb
from server import exchange as ex
from server.config import load

TS = "2026-11-08T06:30:00+08:00"
DAY = "2026-11-08"   # a Sunday


@pytest.fixture
def conn(tmp_path):
    gdb.init(tmp_path / "game.db")
    c = gdb.connect(tmp_path / "game.db")
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


def _subscribing(conn, cfg):
    conn.execute("UPDATE companies SET status='delisted' WHERE id=1")   # the fixture's other company is not used here
    conn.execute("""INSERT INTO companies(id, code, title, first_air, listed_on, status, sub_start, sub_end, shares,
                    ipo_price, prev_close, last_price) VALUES (2, '100002', '新番', '2026-12-01', '2026-11-10',
                    'subscribing', '2026-11-06', '2026-11-09', 10000, 500, 500, 500)""")
    for kind in ("company", "committee"):
        conn.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES (?, ?, 2, ?)", (kind, kind, TS))
    committee = conn.execute("SELECT id FROM accounts WHERE kind='committee' AND company_id=2").fetchone()[0]
    ex.add_shares(conn, committee, 2, 7500, 500, "2026-11-10")
    return committee


def _apply(conn, name, qty, cash=10_000_000):
    world = conn.execute("SELECT id FROM accounts WHERE kind='world'").fetchone()[0]
    pid = conn.execute("INSERT INTO accounts(kind, name, username, created_at) VALUES ('player',?,?,?)", (name, name, TS)).lastrowid
    ex.transfer(conn, world, pid, cash, "grant", day=DAY, ts=TS)
    frozen = ex.Fees().buy_reserve(500, qty)
    conn.execute("UPDATE accounts SET cash_frozen = cash_frozen + ? WHERE id=?", (frozen, pid))
    conn.execute("INSERT INTO ipo_applications(company_id, account_id, qty, frozen, created_at) VALUES (2,?,?,?,?)",
                 (pid, qty, frozen, TS + name))
    return pid


def test_undersubscribed_ipo_fills_everyone_and_founders_keep_the_rest(conn):
    cfg = load()
    committee = _subscribing(conn, cfg)
    a, b = _apply(conn, "a", 600), _apply(conn, "b", 400)
    daily.allot(conn, cfg, dict(conn.execute("SELECT * FROM companies WHERE id=2").fetchone()), DAY, TS, {"exported": 0})
    assert ex.available_shares(conn, a, 2) == 600 and ex.available_shares(conn, b, 2) == 400
    assert ex.available_shares(conn, committee, 2) == 7500 + 1500
    assert conn.execute("SELECT SUM(cash_frozen) FROM accounts WHERE kind='player'").fetchone()[0] == 0
    assert conn.execute("SELECT status FROM companies WHERE id=2").fetchone()[0] == "listed"
    gdb.check_invariants(conn)


def test_oversubscribed_ipo_claws_back_and_ballots(conn):
    cfg = load()
    _subscribing(conn, cfg)
    # base public tranche 1250; 25,000 applied = 20x -> clawback to 70% of the 2,500 float = 1,750
    pids = [_apply(conn, f"p{i}", q) for i, q in enumerate([10_000, 10_000, 4_000, 1_000])]
    daily.allot(conn, cfg, dict(conn.execute("SELECT * FROM companies WHERE id=2").fetchone()), DAY, TS, {"exported": 0})
    got = [ex.available_shares(conn, p, 2) for p in pids]
    assert sum(got) == 1750
    assert [round(x / 25_000 * 1750) for x in (10_000, 10_000, 4_000, 1_000)] == [700, 700, 280, 70]
    assert all(abs(g - e) <= 1 for g, e in zip(got, [700, 700, 280, 70]))
    assert conn.execute("SELECT SUM(cash_frozen) FROM accounts WHERE kind='player'").fetchone()[0] == 0
    news = conn.execute("SELECT title FROM news WHERE kind='allotment'").fetchone()[0]
    assert "20.0 倍" in news
    gdb.check_invariants(conn)
