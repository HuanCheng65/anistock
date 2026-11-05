"""Order book and settlement: fills, partial fills, cancels, freezing, price limits, FIFO lots, conservation."""

import pytest

from server import db as gdb
from server import exchange as ex

DAY, TS = "2026-11-05", "2026-11-05T10:00:00+08:00"
FEES = ex.Fees()


@pytest.fixture
def conn():
    c = gdb.connect(":memory:")
    gdb.init(c)
    for kind in ("world", "cb", "gov"):
        c.execute("INSERT INTO accounts(kind, name, created_at) VALUES (?,?,?)", (kind, kind, TS))
    c.execute("""INSERT INTO companies(id, code, title, first_air, listed_on, status, shares, ipo_price, prev_close, last_price)
                 VALUES (1, '100001', 'テスト', '2026-10-01', '2026-09-10', 'listed', 1000, 500, 500, 500)""")
    return c


def player(conn, name, cash=0, shares=0):
    cur = conn.execute("INSERT INTO accounts(kind, name, username, created_at) VALUES ('player', ?, ?, ?)", (name, name, TS))
    pid = cur.lastrowid
    if cash:
        ex.transfer(conn, gdb.special(conn, "world"), pid, cash, "grant", day=DAY, ts=TS)
    if shares:
        ex.add_shares(conn, pid, 1, shares, 500, "2026-10-01")
    return pid


def seed_rest(conn):
    """Give the remaining shares to an inert holder so the share-count invariant holds."""
    held = conn.execute("SELECT COALESCE(SUM(qty),0) FROM holdings WHERE company_id=1").fetchone()[0]
    holder = player(conn, "holder")
    ex.add_shares(conn, holder, 1, 1000 - held, 500, "2026-09-10")


def band(conn):
    c = conn.execute("SELECT * FROM companies WHERE id=1").fetchone()
    return ex.price_band(c, DAY, 0.10, 5)


def order(conn, acct, side, qty, price=None, kind="limit"):
    return ex.place_order(conn, account_id=acct, company_id=1, side=side, kind=kind, qty=qty, price=price,
                          day=DAY, ts=TS, fees=FEES, band=band(conn))


def test_limit_orders_cross_at_resting_price(conn):
    a = player(conn, "a", cash=100_000)
    b = player(conn, "b", shares=100)
    seed_rest(conn)
    sell = order(conn, b, "sell", 40, 520)
    assert sell["status"] == "open" and sell["filled"] == 0
    buy = order(conn, a, "buy", 40, 540)
    assert buy["status"] == "filled" and buy["avg_price"] == 520      # resting price wins
    notional = 40 * 520
    a_cash = conn.execute("SELECT cash, cash_frozen FROM accounts WHERE id=?", (a,)).fetchone()
    assert a_cash["cash"] == 100_000 - notional - FEES.commission_on(notional)
    assert a_cash["cash_frozen"] == 0
    b_cash = conn.execute("SELECT cash FROM accounts WHERE id=?", (b,)).fetchone()["cash"]
    assert b_cash == notional - FEES.commission_on(notional) - FEES.stamp_on(notional)
    assert ex.available_shares(conn, a, 1) == 40 and ex.available_shares(conn, b, 1) == 60
    gdb.check_invariants(conn)


def test_partial_fill_then_cancel_releases_frozen(conn):
    a = player(conn, "a", cash=100_000)
    b = player(conn, "b", shares=100)
    seed_rest(conn)
    order(conn, b, "sell", 10, 500)
    r = order(conn, a, "buy", 30, 510)
    assert r["filled"] == 10 and r["status"] == "open"
    frozen = conn.execute("SELECT cash_frozen FROM accounts WHERE id=?", (a,)).fetchone()[0]
    assert frozen > 0
    ex.cancel_order(conn, account_id=a, order_id=r["order_id"], ts=TS)
    assert conn.execute("SELECT cash_frozen FROM accounts WHERE id=?", (a,)).fetchone()[0] == 0
    with pytest.raises(ex.OrderError):
        ex.cancel_order(conn, account_id=a, order_id=r["order_id"], ts=TS)
    gdb.check_invariants(conn)


def test_price_time_priority(conn):
    a = player(conn, "a", cash=100_000)
    s1 = player(conn, "s1", shares=50)
    s2 = player(conn, "s2", shares=50)
    seed_rest(conn)
    order(conn, s1, "sell", 10, 505)
    order(conn, s2, "sell", 10, 500)      # better price, later
    order(conn, s1, "sell", 10, 500)      # same price, even later
    order(conn, a, "buy", 15, 505)
    trades = conn.execute("SELECT seller_id, price, qty FROM trades ORDER BY id").fetchall()
    assert [tuple(t) for t in trades] == [(s2, 500, 10), (s1, 500, 5)]
    gdb.check_invariants(conn)


def test_market_buy_sweeps_and_cancels_rest(conn):
    a = player(conn, "a", cash=100_000)
    b = player(conn, "b", shares=100)
    seed_rest(conn)
    order(conn, b, "sell", 5, 500)
    order(conn, b, "sell", 5, 530)
    with pytest.raises(ex.OrderError, match="涨跌幅"):
        order(conn, b, "sell", 5, 560)    # above the +10% cap of 5.50
    r = order(conn, a, "buy", 20, kind="market")
    assert r["filled"] == 10 and r["status"] == "cancelled"
    gdb.check_invariants(conn)


def test_rejects_outside_limit_and_overspend(conn):
    a = player(conn, "a", cash=1_000)
    b = player(conn, "b", shares=10)
    seed_rest(conn)
    # listed 2026-09-10, so the 5 free days are long over: ±10% around 5.00
    with pytest.raises(ex.OrderError, match="涨跌幅"):
        order(conn, a, "buy", 1, 551)
    with pytest.raises(ex.OrderError, match="资金"):
        order(conn, a, "buy", 10, 500)
    with pytest.raises(ex.OrderError, match="股数"):
        order(conn, b, "sell", 11, 500)
    order(conn, b, "sell", 10, 500)
    with pytest.raises(ex.OrderError, match="股数"):      # already frozen by the open sell
        order(conn, b, "sell", 1, 500)


def test_no_limit_in_first_days_after_listing(conn):
    conn.execute("UPDATE companies SET listed_on='2026-11-03' WHERE id=1")
    lo, hi, free = band(conn)
    assert free and hi >= 500 * 2


def test_fifo_lots(conn):
    a = player(conn, "a")
    ex.add_shares(conn, a, 1, 10, 400, "2026-10-01")
    ex.add_shares(conn, a, 1, 10, 600, "2026-10-20")
    taken = ex.remove_shares(conn, a, 1, 15)
    assert taken == [(10, 400, "2026-10-01"), (5, 600, "2026-10-20")]
    left = conn.execute("SELECT qty, cost FROM lots WHERE account_id=?", (a,)).fetchall()
    assert [tuple(r) for r in left] == [(5, 600)]


def test_no_self_trade(conn):
    a = player(conn, "a", cash=100_000, shares=50)
    seed_rest(conn)
    order(conn, a, "sell", 10, 500)
    r = order(conn, a, "buy", 10, 500)
    assert r["filled"] == 0
    gdb.check_invariants(conn)
