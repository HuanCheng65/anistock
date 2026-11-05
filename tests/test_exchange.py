"""Order book and settlement: fills, partial fills, cancels, freezing, price limits, FIFO lots, conservation."""

import pytest

from server import db as gdb
from server import exchange as ex

DAY, TS = "2026-11-05", "2026-11-05T10:00:00+08:00"
FEES = ex.Fees()
RULES = ex.Rules()


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


def order(conn, acct, side, qty, price=None, kind="limit", ts=TS, match=True):
    return ex.place_order(conn, account_id=acct, company_id=1, side=side, kind=kind, qty=qty, price=price,
                          day=DAY, ts=ts, fees=FEES, rules=RULES, match=match)


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
    order(conn, b, "sell", 5, 560)        # 12% above the reference: the volatility control stops here
    r = order(conn, a, "buy", 20, kind="market")
    assert r["filled"] == 10 and r["status"] == "cancelled"
    c = conn.execute("SELECT * FROM companies WHERE id=1").fetchone()
    assert ex.cooling(c, TS) == (450, 550) and c["vcm_count"] == 1
    gdb.check_invariants(conn)


def test_rejects_absurd_price_and_overspend(conn):
    a = player(conn, "a", cash=1_000)
    b = player(conn, "b", shares=10)
    seed_rest(conn)
    with pytest.raises(ex.OrderError, match="离谱"):
        order(conn, a, "buy", 1, 500 * 20 + 1)
    with pytest.raises(ex.OrderError, match="资金"):
        order(conn, a, "buy", 10, 500)
    with pytest.raises(ex.OrderError, match="股数"):
        order(conn, b, "sell", 11, 500)
    order(conn, b, "sell", 10, 500)
    with pytest.raises(ex.OrderError, match="股数"):      # already frozen by the open sell
        order(conn, b, "sell", 1, 500)


def test_vcm_cools_then_anchors_on_the_open(conn):
    a = player(conn, "a", cash=1_000_000)
    b = player(conn, "b", shares=200)
    seed_rest(conn)
    order(conn, b, "sell", 10, 540)
    order(conn, b, "sell", 10, 600)
    r = order(conn, a, "buy", 20, 650)            # 5.40 fills, 6.00 is +20% from 5.00: cooling starts
    assert r["filled"] == 10
    during = "2026-11-05T10:03:00+08:00"
    assert order(conn, a, "buy", 5, 650, ts=during)["filled"] == 0      # 6.00 is outside 4.50-5.50
    after = "2026-11-05T10:06:00+08:00"
    # second trigger anchors on the day's open (5.00) at ±20%: 6.00 is exactly +20%, allowed
    assert order(conn, a, "buy", 10, 650, ts=after)["filled"] == 10
    order(conn, b, "sell", 10, 610)
    assert order(conn, a, "buy", 10, 650, ts=after)["filled"] == 0      # 6.10 > +20%: cools again
    c = conn.execute("SELECT * FROM companies WHERE id=1").fetchone()
    assert c["vcm_count"] == 2 and ex.cooling(c, after) == (400, 600)
    gdb.check_invariants(conn)


def test_opening_auction_clears_at_one_price(conn):
    a = player(conn, "a", cash=1_000_000)
    c_ = player(conn, "c", cash=1_000_000)
    b = player(conn, "b", shares=100)
    d = player(conn, "d", shares=100)
    seed_rest(conn)
    order(conn, a, "buy", 30, 520, match=False)
    order(conn, c_, "buy", 20, 500, match=False)
    order(conn, b, "sell", 25, 490, match=False)
    order(conn, d, "sell", 30, 510, match=False)
    assert conn.execute("SELECT COUNT(*) FROM trades").fetchone()[0] == 0     # nothing matches while collecting
    price, vol = ex.auction_price(conn, 1, 500)
    assert (price, vol) == (510, 30)      # 5.10: demand 30, supply 55 -> 30; 5.00 gives 25 only
    assert ex.run_auction(conn, 1, day=DAY, ts=TS, fees=FEES) == 510
    prices = {r[0] for r in conn.execute("SELECT price FROM trades")}
    assert prices == {510}
    assert sum(r[0] for r in conn.execute("SELECT qty FROM trades")) == 30
    with pytest.raises(ex.OrderError, match="集合竞价"):
        order(conn, a, "buy", 1, kind="market", match=False)
    gdb.check_invariants(conn)


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
