-- 008: the exchanges' 保证金可用余额 formula (docs/v2-margin.md §4) needs, per contract, how many shares the
-- financing bought (融资买入证券数量) and how much the short sale brought in (融券卖出金额).

ALTER TABLE margin_debts ADD COLUMN qty INTEGER NOT NULL DEFAULT 0;      -- financed shares still held
ALTER TABLE short_positions ADD COLUMN amount INTEGER NOT NULL DEFAULT 0; -- cents: 融券卖出金额 of the shares owed

-- existing contracts: the financed shares are what the principal bought at the last price, at most the holding;
-- the short-sale amount is the locked proceeds (the same until a partial buy-back)
UPDATE margin_debts SET qty = MIN(
    COALESCE((SELECT h.qty FROM holdings h WHERE h.account_id = margin_debts.account_id AND h.company_id = margin_debts.company_id), 0),
    COALESCE((SELECT margin_debts.principal / NULLIF(c.last_price, 0) FROM companies c WHERE c.id = margin_debts.company_id), 0)
);
UPDATE short_positions SET amount = proceeds;
