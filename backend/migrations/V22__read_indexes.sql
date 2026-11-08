-- Indexes for the read paths that scanned whole tables (performance review, 2026-11-08).
-- trades: the day's aggregates (WHERE day=? GROUP BY company_id), a stock's intraday (company_id=? AND day=?),
-- an order's fills (buy_order=? / sell_order=?, also the per-row sums of the order list)
CREATE INDEX IF NOT EXISTS trades_day ON trades(day, company_id);
CREATE INDEX IF NOT EXISTS trades_buy_order ON trades(buy_order);
CREATE INDEX IF NOT EXISTS trades_sell_order ON trades(sell_order);
-- holdings: a stock's holders (the primary key starts with account_id)
CREATE INDEX IF NOT EXISTS holdings_company ON holdings(company_id);
-- ledger: dividends received (to_id=? AND reason='dividend' [AND company_id=?]); a trade's fees (ref=? AND from_id=?)
CREATE INDEX IF NOT EXISTS ledger_to_reason ON ledger(to_id, reason, company_id);
CREATE INDEX IF NOT EXISTS ledger_ref ON ledger(ref);
-- news: a stock's news and a day's news, newest first
CREATE INDEX IF NOT EXISTS news_company ON news(company_id, id);
CREATE INDEX IF NOT EXISTS news_trading_day ON news(day, id);
