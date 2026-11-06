//! 破产重整 (voluntary restructuring, docs/game-design.md). A player whose total assets are under
//! `game.restructure_threshold` of the starting cash may, at most once per season, start over: open orders
//! are cancelled, IPO applications withdrawn, every holding is sold to the world account at the last price,
//! and cash is topped up to the starting amount from the world account. One engine operation.
//!
//! The record still counts: the 总榜 return is time-weighted (chain-linked), as fund performance reporting
//! links sub-period returns across cash flows. Each restructuring closes a segment; the return is
//! `return_factor * (assets / segment_base) - 1`, where `return_factor` is the product of the closed
//! segments' end / start and `segment_base` the current segment's starting assets.

use crate::engine::Engine;
use crate::model::{OrderStatus, PlayerReturn, Restructure};
use crate::pyfmt::{fixed_grouped, int_grouped};
use crate::store::persist::Change;

/// Why a player cannot restructure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// total assets at or above the threshold; carries the threshold in cents
    NotPoor(i64),
    ThisSeason,
    MarketClosed,
    /// still owes for margin trading (融资融券) other than a default's shortfall
    Margin,
    /// the engine's state is inconsistent (lots do not cover a holding): nothing was changed
    Broken(String),
}

impl Refusal {
    /// The player-facing message.
    pub fn message(&self) -> String {
        match self {
            Refusal::NotPoor(cents) => {
                let amount =
                    if cents % 100 == 0 { int_grouped(cents / 100) } else { fixed_grouped(*cents as f64 / 100.0, 2) };
                format!("总资产低于 {amount} 星辉才能申请重整")
            }
            Refusal::ThisSeason => "本季已经重整过一次".into(),
            Refusal::MarketClosed => "休市时不能申请".into(),
            Refusal::Margin => "先了结融资融券负债".into(),
            Refusal::Broken(why) => why.clone(),
        }
    }
}

/// The anime season (quarter starting 1 Jan / Apr / Jul / Oct) of a trading day `YYYY-MM-DD`.
pub fn season(day: &str) -> Option<(i32, u32)> {
    let year: i32 = day.get(0..4)?.parse().ok()?;
    let month: u32 = day.get(5..7)?.parse().ok()?;
    (1..=12).contains(&month).then_some((year, (month - 1) / 3))
}

/// The threshold in cents: total assets must be strictly below it.
pub fn threshold_cents(initial_cents: i64, threshold: f64) -> i64 {
    (initial_cents as f64 * threshold).round() as i64
}

/// Eligibility apart from trading hours: assets under the threshold, and no restructuring yet this season
/// (`last_day` is the trading day of the latest one).
pub fn check(
    total_cents: i64,
    initial_cents: i64,
    threshold: f64,
    last_day: Option<&str>,
    today: &str,
) -> Result<(), Refusal> {
    let limit = threshold_cents(initial_cents, threshold);
    if total_cents >= limit {
        return Err(Refusal::NotPoor(limit));
    }
    if last_day.is_some_and(|d| season(d).is_some() && season(d) == season(today)) {
        return Err(Refusal::ThisSeason);
    }
    Ok(())
}

/// The chain-linked return. Written so that with factor 1 it is exactly v1's `(total - initial) / initial`
/// (yuan floats), so players who never restructured rank and display as before.
pub fn linked_return(factor: f64, base_cents: i64, total_cents: i64) -> f64 {
    let t = total_cents as f64 / 100.0;
    let b = base_cents as f64 / 100.0;
    factor * ((t - b) / b) + (factor - 1.0)
}

impl Engine {
    /// Net assets: cash (frozen included) plus holdings at the last price, minus margin liabilities, cents.
    pub fn total_assets(&self, account_id: i64) -> i64 {
        self.net_assets(account_id)
    }

    /// A player's chain-linked return state, if they have ever restructured.
    pub fn player_return(&self, account_id: i64) -> Option<&PlayerReturn> {
        self.st.player_returns.get(&account_id)
    }

    /// Restructure a player (see the module doc). `initial_cents` is `game.initial_cash` in cents, `day` the
    /// trading day. Fails without changing anything; trading hours are the caller's check.
    pub fn restructure(
        &mut self,
        account_id: i64,
        initial_cents: i64,
        threshold: f64,
        day: &str,
        ts: &str,
    ) -> Result<Restructure, Refusal> {
        let world = self.special("world").ok_or_else(|| Refusal::Broken("no world account".into()))?;
        let assets_before = self.total_assets(account_id);
        // a default (穿仓) may restructure at once, whatever the season; open margin debts must be settled first
        let defaulted = self.margin_restructure_check(account_id).map_err(|()| Refusal::Margin)?;
        let last_day = if defaulted { None } else { self.player_return(account_id).map(|p| p.last_day.clone()) };
        check(assets_before, initial_cents, threshold, last_day.as_deref(), day)?;
        let holdings: Vec<(i64, i64)> = self.holdings_of(account_id).map(|h| (h.company_id, h.qty)).collect();
        for &(cid, qty) in &holdings {
            let lots: i64 = self.lots_of(account_id, cid).iter().map(|l| l.qty).sum();
            if lots != qty {
                return Err(Refusal::Broken(format!(
                    "lots {lots} != holding {qty} (account {account_id}, company {cid})"
                )));
            }
        }

        // 1. cancel open orders and withdraw pending IPO applications (their frozen cash and shares return)
        let orders: Vec<i64> = self.open_orders_of(account_id).iter().map(|o| o.id).collect();
        for id in orders {
            self.close(id, OrderStatus::Cancelled, ts);
        }
        let apps: Vec<i64> = self
            .st
            .ipo_apps
            .values()
            .filter(|a| a.account_id == account_id && a.allotted.is_none())
            .map(|a| a.company_id)
            .collect();
        for cid in apps {
            self.do_withdraw(cid, account_id);
        }
        self.rights_withdraw_all(account_id);

        // 2. every holding to the world account at the last price (a transfer, not orders: no fees, no trades)
        let mut sold = 0;
        for (cid, qty) in holdings {
            let price = self.company(cid).map_or(0, |c| c.last_price);
            let frozen = self.holding(account_id, cid).map_or(0, |h| h.qty_frozen);
            self.do_remove_shares(account_id, cid, qty, frozen).map_err(Refusal::Broken)?;
            self.do_add_shares(world, cid, qty, price, day);
            self.do_transfer(world, account_id, qty * price, "restructure", day, ts, Some(cid), None);
            sold += qty * price;
        }

        // 3. the default's shortfall is written off (the world account bears it), then cash up to the
        //    starting amount
        self.margin_write_off(account_id);
        let cash = self.account(account_id).map_or(0, |a| a.cash);
        let injected = (initial_cents - cash).max(0);
        self.do_transfer(world, account_id, injected, "restructure", day, ts, None, None);

        // 4. close the return segment and record it
        let prev = self.st.player_returns.get(&account_id).cloned();
        let (factor, base, count) =
            prev.map_or((1.0, initial_cents, 0), |p| (p.return_factor, p.segment_base, p.restructures));
        let pr = PlayerReturn {
            account_id,
            // negative net assets (a default) count as zero: the segment lost everything
            return_factor: factor * (assets_before.max(0) as f64 / base as f64),
            segment_base: initial_cents,
            restructures: count + 1,
            last_day: day.to_string(),
        };
        let rec = Restructure { account_id, day: day.to_string(), ts: ts.to_string(), assets_before, sold, injected };
        self.out.push(Change::RestructureInsert(rec.clone()));
        self.out.push(Change::PlayerReturnSet(pr.clone()));
        self.st.player_returns.insert(account_id, pr);
        self.commit();
        Ok(rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seasons_are_quarters() {
        assert_eq!(season("2026-12-31"), Some((2026, 3)));
        assert_eq!(season("2027-01-01"), Some((2027, 0)));
        assert_eq!(season("2026-04-01"), Some((2026, 1)));
        assert_eq!(season("2026-03-31"), Some((2026, 0)));
        assert_eq!(season("bad"), None);
    }

    #[test]
    fn factor_one_is_the_plain_return() {
        for total in [0, 1, 999_999, 10_000_000, 12_345_678, 1_000_000_000] {
            let plain = (total as f64 / 100.0 - 100000.0) / 100000.0;
            assert_eq!(linked_return(1.0, 10_000_000, total).to_bits(), plain.to_bits(), "{total}");
        }
    }
}
