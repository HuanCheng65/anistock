//! 融资融券: margin buying and short selling (docs/v2-margin.md).
//!
//! The player's own account is the credit account: cash and every holding are collateral. What it owes is
//! kept per stock: `margin_debts` (financing: principal and unpaid interest) and `short_positions` (shares
//! borrowed from the company's rights holders, the short-sale proceeds locked in `cash_frozen`, unpaid fees).
//! Every cash movement is a ledger transfer, so the ledger keeps summing to zero; borrowed shares really move
//! from the lender to the borrower, so every listed company's shares stay fully held.
//!
//! Credit orders are limit orders tagged with a [`Mode`] (table `margin_orders`); the exchange core calls
//! three hooks: [`Engine::margin_tag`] when the order is created, [`Engine::margin_on_fill`] when it trades,
//! [`Engine::margin_on_close`] when it ends.
//!
//! - margin_buy: the loan is drawn when the order is placed (world -> player, `margin_loan`) and frozen in
//!   the order like any buy; what the order does not spend is repaid when it ends.
//! - short_sell: price at or above the last trade. The shares are borrowed when the order is placed and rest
//!   in the player's holdings, frozen by the sell order; the net proceeds of each fill are locked; unfilled
//!   shares go back to the lender when the order ends.
//! - buy_return: locked proceeds move into the order's reserve first; bought shares go straight to the lender.
//! - sell_repay: an ordinary sell whose net proceeds repay financing (interest first, this stock first).
//!
//! Maintenance (维持担保比例 = gross assets / liabilities): a margin call at the daily close under
//! `call_ratio`, liquidation at the next close if still under, or at once intraday under `liquidate_ratio` or
//! with negative equity. Liquidation runs in [`Engine::margin_sweep`] (continuous trading, once a minute):
//! protected limit orders, immediate-or-cancel. A default (穿仓) settles what is left in cash and leaves the
//! shortfall as `bad_debt`, written off by restructuring.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, NewOrder, Placed};
use crate::exchange::{quote_band, sanity_band, OrderError};
use crate::model::{OrderStatus, Side};
use crate::store::persist::Change;

pub const NORMAL: &str = "normal";
pub const CALL: &str = "call";
pub const LIQUIDATING: &str = "liquidating";
pub const DEFAULTED: &str = "defaulted";

/// `[margin]` of game.toml (docs/v2-margin.md §11).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct MarginRules {
    pub enabled: bool,
    /// days since registration before a player may open credit
    pub min_days: i64,
    /// net assets at least this share of the starting cash
    pub min_assets: f64,
    /// a stock is marginable this many days after listing
    pub listed_days: i64,
    /// collateral value of a marginable stock (折算率)
    pub haircut: f64,
    /// initial margin for financing and for short sales (保证金比例)
    pub margin_ratio: f64,
    /// financing rate = policy rate + this
    pub interest_spread: f64,
    /// lending fee = policy rate + this
    pub fee_spread: f64,
    pub call_ratio: f64,
    pub liquidate_ratio: f64,
    pub target_ratio: f64,
    /// liquidation orders: limit at last price × (1 ± protect)
    pub protect: f64,
    /// share of the rights holders' stock (held + lent) they lend
    pub lend_frac: f64,
    /// a stock's financing balance / short interest at most this share of its float
    pub stock_cap: f64,
    /// one stock's financing + short value at most this multiple of net assets
    pub concentration: f64,
    pub npc_short_frac: f64,
    pub npc_short_extra: f64,
    pub npc_short_size: f64,
}

impl Default for MarginRules {
    fn default() -> MarginRules {
        MarginRules {
            enabled: true,
            min_days: 7,
            min_assets: 0.5,
            listed_days: 5,
            haircut: 0.7,
            margin_ratio: 1.0,
            interest_spread: 0.03,
            fee_spread: 0.05,
            call_ratio: 1.3,
            liquidate_ratio: 1.1,
            target_ratio: 1.5,
            protect: 0.05,
            lend_frac: 0.5,
            stock_cap: 0.25,
            concentration: 1.0,
            npc_short_frac: 0.3,
            npc_short_extra: 0.10,
            npc_short_size: 0.10,
        }
    }
}

impl MarginRules {
    pub fn from_table(t: Option<&toml::Value>) -> Result<MarginRules, String> {
        match t {
            None => Ok(MarginRules::default()),
            Some(v) => v.clone().try_into().map_err(|e| format!("[margin]: {e}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    MarginBuy,
    ShortSell,
    SellRepay,
    BuyReturn,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::MarginBuy => "margin_buy",
            Mode::ShortSell => "short_sell",
            Mode::SellRepay => "sell_repay",
            Mode::BuyReturn => "buy_return",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        [Mode::MarginBuy, Mode::ShortSell, Mode::SellRepay, Mode::BuyReturn].into_iter().find(|m| m.as_str() == s)
    }
    pub fn side(self) -> Side {
        match self {
            Mode::MarginBuy | Mode::BuyReturn => Side::Buy,
            Mode::ShortSell | Mode::SellRepay => Side::Sell,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreditAccount {
    pub account_id: i64,
    pub opened_at: String,
    pub status: String,
    pub call_day: Option<String>,
    pub bad_debt: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Debt {
    pub account_id: i64,
    pub company_id: i64,
    pub principal: i64,
    pub interest: i64,
    pub opened_on: String,
    /// shares bought with this financing still held (融资买入证券数量; V8). Every sale of the stock reduces it.
    pub qty: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Short {
    pub account_id: i64,
    pub company_id: i64,
    pub lender_id: i64,
    pub qty: i64,
    pub proceeds: i64,
    pub fee: i64,
    pub opened_on: String,
    /// 融券卖出金额 of the shares sold and still owed (net of fees; V8): reduced pro rata as they are returned
    pub amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarginOrder {
    pub order_id: i64,
    pub mode: Mode,
    pub from_locked: i64,
}

/// Everything margin-related in the engine's state (cloned with it for rollback).
#[derive(Debug, Clone, Default)]
pub struct MarginState {
    pub credit: BTreeMap<i64, CreditAccount>,
    pub debts: BTreeMap<(i64, i64), Debt>,
    pub shorts: BTreeMap<(i64, i64), Short>,
    /// modes of open credit orders (and of orders closed since the last daily close)
    pub orders: HashMap<i64, MarginOrder>,
    /// set while a credit order is being placed: its mode and the locked cash moved into it
    pending: Option<(Mode, i64)>,
}

/// One row change, applied by the persister.
#[derive(Debug, Clone, PartialEq)]
pub enum MarginChange {
    Credit(CreditAccount),
    CreditDelete(i64),
    Debt(Debt),
    DebtDelete(i64, i64),
    Short(Short),
    ShortDelete(i64, i64),
    Order(MarginOrder),
}

pub fn apply(conn: &Connection, c: &MarginChange) -> rusqlite::Result<()> {
    match c {
        MarginChange::Credit(a) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO credit_accounts(account_id, opened_at, status, call_day, bad_debt) VALUES (?,?,?,?,?)",
            )?
            .execute(params![a.account_id, a.opened_at, a.status, a.call_day, a.bad_debt])?;
        }
        MarginChange::CreditDelete(id) => {
            conn.prepare_cached("DELETE FROM credit_accounts WHERE account_id=?")?.execute([id])?;
        }
        MarginChange::Debt(d) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO margin_debts(account_id, company_id, principal, interest, opened_on, qty) VALUES (?,?,?,?,?,?)",
            )?
            .execute(params![d.account_id, d.company_id, d.principal, d.interest, d.opened_on, d.qty])?;
        }
        MarginChange::DebtDelete(a, c) => {
            conn.prepare_cached("DELETE FROM margin_debts WHERE account_id=? AND company_id=?")?.execute([a, c])?;
        }
        MarginChange::Short(s) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO short_positions(account_id, company_id, lender_id, qty, proceeds, fee, opened_on, amount)
                 VALUES (?,?,?,?,?,?,?,?)",
            )?
            .execute(params![s.account_id, s.company_id, s.lender_id, s.qty, s.proceeds, s.fee, s.opened_on, s.amount])?;
        }
        MarginChange::ShortDelete(a, c) => {
            conn.prepare_cached("DELETE FROM short_positions WHERE account_id=? AND company_id=?")?.execute([a, c])?;
        }
        MarginChange::Order(o) => {
            conn.prepare_cached("INSERT OR REPLACE INTO margin_orders(order_id, mode, from_locked) VALUES (?,?,?)")?
                .execute(params![o.order_id, o.mode.as_str(), o.from_locked])?;
        }
    }
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", [name], |_| Ok(())).optional()?.is_some())
}

/// Load the margin tables (empty if the database predates migration V7, e.g. an old simulation world).
pub fn load(conn: &Connection) -> rusqlite::Result<MarginState> {
    let mut s = MarginState::default();
    if !table_exists(conn, "credit_accounts")? {
        return Ok(s);
    }
    // V8 added the financed quantity and the short-sale amount (the collateral formula)
    let v8 = conn.prepare("SELECT qty FROM margin_debts LIMIT 0").is_ok();
    for r in conn
        .prepare("SELECT account_id, opened_at, status, call_day, bad_debt FROM credit_accounts")?
        .query_map([], |r| {
            Ok(CreditAccount { account_id: r.get(0)?, opened_at: r.get(1)?, status: r.get(2)?, call_day: r.get(3)?, bad_debt: r.get(4)? })
        })?
    {
        let r = r?;
        s.credit.insert(r.account_id, r);
    }
    for r in conn
        .prepare(&format!("SELECT account_id, company_id, principal, interest, opened_on, {} FROM margin_debts", if v8 { "qty" } else { "0" }))?
        .query_map([], |r| {
            Ok(Debt { account_id: r.get(0)?, company_id: r.get(1)?, principal: r.get(2)?, interest: r.get(3)?, opened_on: r.get(4)?, qty: r.get(5)? })
        })?
    {
        let r = r?;
        s.debts.insert((r.account_id, r.company_id), r);
    }
    for r in conn
        .prepare(&format!(
            "SELECT account_id, company_id, lender_id, qty, proceeds, fee, opened_on, {} FROM short_positions",
            if v8 { "amount" } else { "proceeds" }
        ))?
        .query_map([], |r| {
            Ok(Short {
                account_id: r.get(0)?,
                company_id: r.get(1)?,
                lender_id: r.get(2)?,
                qty: r.get(3)?,
                proceeds: r.get(4)?,
                fee: r.get(5)?,
                opened_on: r.get(6)?,
                amount: r.get(7)?,
            })
        })?
    {
        let r = r?;
        s.shorts.insert((r.account_id, r.company_id), r);
    }
    for r in conn
        .prepare(
            "SELECT m.order_id, m.mode, m.from_locked FROM margin_orders m JOIN orders o ON o.id = m.order_id WHERE o.status='open'",
        )?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
    {
        let (id, mode, from_locked) = r?;
        if let Some(mode) = Mode::parse(&mode) {
            s.orders.insert(id, MarginOrder { order_id: id, mode, from_locked });
        }
    }
    Ok(s)
}

/// A credit account's numbers at last prices (cents).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CreditNumbers {
    pub cash: i64,
    /// holdings at last prices (borrowed shares not yet sold included)
    pub stock_value: i64,
    pub debt_principal: i64,
    pub debt_interest: i64,
    pub short_value: i64,
    pub short_fees: i64,
    pub locked: i64,
    pub bad_debt: i64,
    pub liabilities: i64,
    /// new financing or short sales this account can still back (保证金可用余额)
    pub available_margin: i64,
}

impl CreditNumbers {
    pub fn gross(&self) -> i64 {
        self.cash + self.stock_value
    }
    pub fn net(&self) -> i64 {
        self.gross() - self.liabilities
    }
    /// 维持担保比例; None without liabilities.
    pub fn ratio(&self) -> Option<f64> {
        (self.liabilities > 0).then(|| self.gross() as f64 / self.liabilities as f64)
    }
}

/// Is a stock 融资融券标的 today, and why not.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StockMargin {
    pub eligible: bool,
    /// None when eligible; else one of not_listed, new_listing, penny, delisting
    pub reason: Option<&'static str>,
    pub haircut: f64,
    /// shares the rights holders can still lend
    pub lendable: i64,
    /// financing (cents) the stock can still take before its cap
    pub financing_room: i64,
}

fn days_between(a: &str, b: &str) -> i64 {
    match (crate::vclose::day_no(a.get(..10).unwrap_or(a)), crate::vclose::day_no(b.get(..10).unwrap_or(b))) {
        (Some(x), Some(y)) => y - x,
        _ => 0,
    }
}

fn err(msg: impl Into<String>) -> OrderError {
    OrderError::new(msg)
}

impl Engine {
    // ------------------------------------------------------------ reads

    pub fn credit(&self, account_id: i64) -> Option<&CreditAccount> {
        self.st.margin.credit.get(&account_id)
    }
    pub fn credit_accounts(&self) -> impl Iterator<Item = &CreditAccount> {
        self.st.margin.credit.values()
    }
    pub fn margin_mode(&self, order_id: i64) -> Option<Mode> {
        self.st.margin.orders.get(&order_id).map(|o| o.mode)
    }
    pub fn debts_of(&self, account_id: i64) -> impl Iterator<Item = &Debt> {
        self.st.margin.debts.range((account_id, i64::MIN)..=(account_id, i64::MAX)).map(|(_, d)| d)
    }
    pub fn shorts_of(&self, account_id: i64) -> impl Iterator<Item = &Short> {
        self.st.margin.shorts.range((account_id, i64::MIN)..=(account_id, i64::MAX)).map(|(_, s)| s)
    }
    pub fn short_of(&self, account_id: i64, company_id: i64) -> Option<&Short> {
        self.st.margin.shorts.get(&(account_id, company_id))
    }
    /// Shares owed (borrowed) by `account_id` in `company_id`.
    pub fn short_qty(&self, account_id: i64, company_id: i64) -> i64 {
        self.short_of(account_id, company_id).map_or(0, |s| s.qty)
    }

    fn policy_rate(&self) -> f64 {
        self.state("rate").and_then(crate::pyfmt::parse_float).unwrap_or(0.0)
    }
    /// Annual financing rate.
    pub fn financing_rate(&self) -> f64 {
        (self.policy_rate() + self.margin.interest_spread).max(0.0)
    }
    /// Annual lending fee rate.
    pub fn lending_rate(&self) -> f64 {
        (self.policy_rate() + self.margin.fee_spread).max(0.0)
    }

    fn last_price(&self, company_id: i64) -> i64 {
        self.company(company_id).map_or(0, |c| c.last_price)
    }

    /// Open credit orders of an account: (order id, mode, company, shares left, cash frozen).
    fn open_credit_orders(&self, account_id: i64) -> Vec<(i64, Mode, i64, i64, i64)> {
        self.open_orders_of(account_id)
            .into_iter()
            .filter_map(|o| self.margin_mode(o.id).map(|m| (o.id, m, o.company_id, o.left(), o.frozen)))
            .collect()
    }

    /// Borrowed shares of (account, company) not sold yet (resting in open short-sell orders).
    fn unsold(&self, account_id: i64, company_id: i64) -> i64 {
        self.open_credit_orders(account_id)
            .iter()
            .filter(|o| o.1 == Mode::ShortSell && o.2 == company_id)
            .map(|o| o.3)
            .sum()
    }

    /// Liabilities of an account (cents): financing principal and interest, borrowed shares at the last
    /// price, unpaid fees, default shortfall. 0 for accounts without credit.
    pub fn liabilities(&self, account_id: i64) -> i64 {
        let Some(ca) = self.credit(account_id) else { return 0 };
        let debt: i64 = self.debts_of(account_id).map(|d| d.principal + d.interest).sum();
        let short: i64 = self.shorts_of(account_id).map(|s| s.qty * self.last_price(s.company_id) + s.fee).sum();
        debt + short + ca.bad_debt
    }

    /// Cash + holdings at last prices − liabilities (净资产). For accounts without credit this is the old
    /// "total assets".
    pub fn net_assets(&self, account_id: i64) -> i64 {
        let cash = self.account(account_id).map_or(0, |a| a.cash);
        let stock: i64 = self.holdings_of(account_id).map(|h| h.qty * self.last_price(h.company_id)).sum();
        cash + stock - self.liabilities(account_id)
    }

    /// Why a stock is not marginable on trading day `day` (None: it is).
    pub fn margin_ineligible(&self, company_id: i64, day: &str) -> Option<&'static str> {
        let c = match self.company(company_id) {
            Some(c) if c.status == "listed" => c,
            _ => return Some("not_listed"),
        };
        if days_between(&c.listed_on, day) < self.margin.listed_days {
            return Some("new_listing");
        }
        if c.prev_close < crate::daily::PENNY_CENTS || self.st.penny_days.get(&company_id).is_some_and(|n| *n > 0) {
            return Some("penny");
        }
        if c.low_weeks > 0 {
            return Some("delisting");
        }
        None
    }

    fn haircut_of(&self, company_id: i64, day: &str) -> f64 {
        if self.margin_ineligible(company_id, day).is_none() {
            self.margin.haircut
        } else {
            0.0
        }
    }

    fn holding_qty(&self, account_id: i64, company_id: i64) -> i64 {
        self.holding(account_id, company_id).map_or(0, |h| h.qty)
    }

    /// The rights holders' account lending this stock.
    fn lender_of(&self, company_id: i64) -> Option<i64> {
        self.account_of("committee", company_id)
    }

    fn lent(&self, company_id: i64) -> i64 {
        self.st.margin.shorts.values().filter(|s| s.company_id == company_id).map(|s| s.qty).sum()
    }

    fn financed(&self, company_id: i64) -> i64 {
        self.st.margin.debts.values().filter(|d| d.company_id == company_id).map(|d| d.principal).sum()
    }

    /// Shares in public hands: all shares but the rights holders' and the company's own.
    fn float_qty(&self, company_id: i64) -> i64 {
        let Some(c) = self.company(company_id) else { return 0 };
        let com = self.lender_of(company_id).map_or(0, |a| self.holding_qty(a, company_id));
        let own = self.account_of("company", company_id).map_or(0, |a| self.holding_qty(a, company_id));
        (c.shares - com - own).max(0)
    }

    /// Shares the rights holders can lend now.
    pub fn lendable(&self, company_id: i64) -> i64 {
        let Some(lender) = self.lender_of(company_id) else { return 0 };
        let lent = self.lent(company_id);
        let free = self.available_shares(lender, company_id);
        let held = self.holding_qty(lender, company_id);
        let by_holder = (self.margin.lend_frac * (held + lent) as f64).floor() as i64 - lent;
        let by_cap = (self.margin.stock_cap * self.float_qty(company_id) as f64).floor() as i64 - lent;
        by_holder.min(by_cap).min(free).max(0)
    }

    /// Financing the stock can still take (cents).
    pub fn financing_room(&self, company_id: i64) -> i64 {
        let cap = self.margin.stock_cap * (self.float_qty(company_id) * self.last_price(company_id)) as f64;
        (cap.floor() as i64 - self.financed(company_id)).max(0)
    }

    pub fn stock_margin(&self, company_id: i64, day: &str) -> StockMargin {
        let reason = self.margin_ineligible(company_id, day);
        StockMargin {
            eligible: reason.is_none(),
            reason,
            haircut: self.haircut_of(company_id, day),
            lendable: self.lendable(company_id),
            financing_room: self.financing_room(company_id),
        }
    }

    /// Financed shares of (account, company) as the formula counts them: what the debt bought, at most what is
    /// held (borrowed shares not yet sold excluded).
    fn financed_qty(&self, account_id: i64, company_id: i64, unsold: i64) -> i64 {
        let held = self.holding_qty(account_id, company_id) - unsold;
        self.st.margin.debts.get(&(account_id, company_id)).map_or(0, |d| d.qty).clamp(0, held.max(0))
    }

    /// The account's credit numbers on trading day `day` (haircuts depend on it).
    ///
    /// 保证金可用余额 by the exchanges' formula (上交所、深交所《融资融券交易实施细则》):
    ///
    ///   现金 + Σ 充抵保证金证券市值 × 折算率
    ///   + Σ (融资买入证券市值 − 融资买入金额) × 折算率
    ///   + Σ (融券卖出金额 − 融券卖出证券市值) × 折算率
    ///   − Σ 融券卖出金额 − Σ 融资买入金额 × 融资保证金比例 − Σ 融券卖出证券市值 × 融券保证金比例
    ///   − 利息及费用
    ///
    /// where a loss in either floating term counts in full (折算率 100%). Financed shares are the debt's `qty`;
    /// the rest of a holding is ordinary collateral. A resting margin buy has drawn its loan into the order:
    /// the frozen cash is in 现金 and the loan in 融资买入金额 with no shares yet, a loss that cancels the cash.
    /// A resting short sale's borrowed shares are not collateral; they are owed (the requirement) with
    /// nothing sold yet. With cash C and nothing else, at most C can be financed or sold short.
    pub fn credit_numbers(&self, account_id: i64, day: &str) -> CreditNumbers {
        let mut n = CreditNumbers { cash: self.account(account_id).map_or(0, |a| a.cash), ..Default::default() };
        let pending = self.open_credit_orders(account_id);
        let unsold = |c: i64| -> i64 { pending.iter().filter(|o| o.1 == Mode::ShortSell && o.2 == c).map(|o| o.3).sum() };
        let m = self.margin.margin_ratio;
        let floating = |x: f64, h: f64| if x > 0.0 { x * h } else { x };
        let mut avail = n.cash as f64;
        for h in self.holdings_of(account_id) {
            let p = self.last_price(h.company_id);
            n.stock_value += h.qty * p;
            let u = unsold(h.company_id);
            let own = h.qty - u - self.financed_qty(account_id, h.company_id, u);
            avail += (own * p) as f64 * self.haircut_of(h.company_id, day);
        }
        for d in self.debts_of(account_id) {
            n.debt_principal += d.principal;
            n.debt_interest += d.interest;
            let p = self.last_price(d.company_id);
            let fq = self.financed_qty(account_id, d.company_id, unsold(d.company_id));
            avail += floating((fq * p - d.principal) as f64, self.haircut_of(d.company_id, day));
            avail -= m * d.principal as f64;
        }
        for s in self.shorts_of(account_id) {
            let p = self.last_price(s.company_id);
            n.short_value += s.qty * p;
            n.short_fees += s.fee;
            n.locked += s.proceeds;
            let sold = s.qty - unsold(s.company_id);
            avail += floating((s.amount - sold * p) as f64, self.haircut_of(s.company_id, day));
            avail -= s.amount as f64;
            avail -= m * (s.qty * p) as f64;
        }
        n.bad_debt = self.credit(account_id).map_or(0, |c| c.bad_debt);
        n.liabilities = n.debt_principal + n.debt_interest + n.short_value + n.short_fees + n.bad_debt;
        avail -= (n.debt_interest + n.short_fees + n.bad_debt) as f64;
        n.available_margin = avail.floor() as i64;
        n
    }

    /// One stock's financing + short value of an account (the concentration limit).
    fn exposure(&self, account_id: i64, company_id: i64) -> i64 {
        let debt = self.st.margin.debts.get(&(account_id, company_id)).map_or(0, |d| d.principal);
        debt + self.short_qty(account_id, company_id) * self.last_price(company_id)
    }

    // ------------------------------------------------------------ change helpers

    fn emit_credit(&mut self, account_id: i64) {
        if let Some(c) = self.st.margin.credit.get(&account_id) {
            self.out.push(Change::Margin(MarginChange::Credit(c.clone())));
        }
    }

    fn set_status(&mut self, account_id: i64, status: &str, call_day: Option<String>) {
        if let Some(c) = self.st.margin.credit.get_mut(&account_id) {
            if c.status != status || c.call_day != call_day {
                c.status = status.to_string();
                c.call_day = call_day;
                self.emit_credit(account_id);
            }
        }
    }

    fn emit_debt(&mut self, key: (i64, i64)) {
        match self.st.margin.debts.get(&key) {
            Some(d) if d.principal == 0 && d.interest == 0 => {
                self.st.margin.debts.remove(&key);
                self.out.push(Change::Margin(MarginChange::DebtDelete(key.0, key.1)));
            }
            Some(d) => self.out.push(Change::Margin(MarginChange::Debt(d.clone()))),
            None => {}
        }
    }

    fn emit_short(&mut self, key: (i64, i64)) {
        match self.st.margin.shorts.get(&key) {
            Some(s) if s.qty == 0 && s.proceeds == 0 && s.fee == 0 => {
                self.st.margin.shorts.remove(&key);
                self.out.push(Change::Margin(MarginChange::ShortDelete(key.0, key.1)));
            }
            Some(s) => self.out.push(Change::Margin(MarginChange::Short(s.clone()))),
            None => {}
        }
    }

    // ------------------------------------------------------------ opening

    /// Why this player cannot open credit now (None: they can).
    pub fn credit_open_refusal(&self, account_id: i64, initial_cents: i64, day: &str) -> Option<String> {
        if !self.margin.enabled {
            return Some("暂未开放融资融券".into());
        }
        if let Some(c) = self.credit(account_id) {
            return Some(if c.status == DEFAULTED { "穿仓后需要先申请重整".into() } else { "信用账户已经开通".into() });
        }
        let Some(a) = self.account(account_id) else { return Some("找不到账户".into()) };
        if days_between(&a.created_at, day) < self.margin.min_days {
            return Some(format!("注册满 {} 天后才能开通", self.margin.min_days));
        }
        let need = (initial_cents as f64 * self.margin.min_assets).round() as i64;
        if self.net_assets(account_id) < need {
            return Some(format!("净资产不低于 {} 币才能开通", crate::pyfmt::int_grouped(need / 100)));
        }
        None
    }

    /// Open credit for a player (`initial_cents`: the starting cash, for the asset condition).
    pub fn open_credit(&mut self, account_id: i64, initial_cents: i64, day: &str, ts: &str) -> Result<(), OrderError> {
        if let Some(why) = self.credit_open_refusal(account_id, initial_cents, day) {
            return Err(OrderError(why));
        }
        self.do_open_credit(account_id, ts);
        self.commit();
        Ok(())
    }

    /// Open credit without the player conditions (NPCs).
    pub fn open_credit_unchecked(&mut self, account_id: i64, ts: &str) {
        if self.credit(account_id).is_none() {
            self.do_open_credit(account_id, ts);
            self.commit();
        }
    }

    fn do_open_credit(&mut self, account_id: i64, ts: &str) {
        let c = CreditAccount { account_id, opened_at: ts.to_string(), status: NORMAL.into(), call_day: None, bad_debt: 0 };
        self.st.margin.credit.insert(account_id, c);
        self.emit_credit(account_id);
    }

    // ------------------------------------------------------------ credit orders

    /// Place a credit order (limit only, continuous trading only). Validates everything `do_place_order`
    /// would, before changing anything, so the preparation (loan, borrowed shares, unlocked proceeds) never
    /// has to be undone.
    pub fn place_credit(&mut self, req: &NewOrder, mode: Mode) -> Result<Placed, OrderError> {
        let r = self.do_place_credit(req, mode, false);
        self.commit();
        r
    }

    fn do_place_credit(&mut self, req: &NewOrder, mode: Mode, forced: bool) -> Result<Placed, OrderError> {
        let (acct, cid, qty) = (req.account_id, req.company_id, req.qty);
        let status = self.credit(acct).map(|c| c.status.clone()).ok_or_else(|| err("尚未开通信用账户"))?;
        if status == DEFAULTED {
            return Err(err("信用账户已穿仓"));
        }
        if !req.match_ {
            return Err(err("信用委托只在连续交易时段接受"));
        }
        if req.kind != "limit" {
            return Err(err("信用委托只接受限价"));
        }
        if Side::parse(req.side) != Some(mode.side()) {
            return Err(err("买卖方向不对"));
        }
        if qty <= 0 {
            return Err(err("数量必须是正整数"));
        }
        let co = match self.company(cid) {
            Some(c) if c.status == "listed" => c.clone(),
            _ => return Err(err("这支股票不存在或已退市")),
        };
        let p = match req.price {
            Some(p) if p > 0 => p,
            _ => return Err(err("限价单需要价格")),
        };
        let (_, hi) = sanity_band(&co, &self.rules);
        if p > hi {
            return Err(OrderError(format!("价格太离谱了：不能超过昨收的 {} 倍（{}）", self.rules.sanity_mult, crate::exchange::yuan_str(hi))));
        }
        let opening = matches!(mode, Mode::MarginBuy | Mode::ShortSell);
        if opening && !forced && status != NORMAL {
            return Err(err("追保或强制平仓期间不能新增融资融券"));
        }
        if opening && !forced {
            if self.margin_ineligible(cid, req.day).is_some() {
                return Err(err("这支股票不是融资融券标的"));
            }
        }
        let numbers = self.credit_numbers(acct, req.day);
        let last = co.last_price.max(1);
        let mut from_locked = 0;
        match mode {
            Mode::MarginBuy => {
                if self.short_qty(acct, cid) > 0 {
                    return Err(err("欠这支股票的券时，买入请用买券还券"));
                }
                let need = self.fees.buy_reserve(p, qty);
                if need > self.financing_room(cid) {
                    return Err(err("这支股票的融资额度已满"));
                }
                if self.exposure(acct, cid) + need > (self.margin.concentration * numbers.net() as f64) as i64 {
                    return Err(err("单支股票的融资融券不能超过净资产"));
                }
                // the new contract: shares worth qty × last price against `need` borrowed (a gain at the haircut,
                // a loss in full), plus the margin on `need`
                let fresh = (qty * last - need) as f64;
                let fresh = if fresh > 0.0 { fresh * self.haircut_of(cid, req.day) } else { fresh };
                let after = numbers.available_margin as f64 + fresh - self.margin.margin_ratio * need as f64;
                if after < 0.0 {
                    return Err(err("可用保证金不足"));
                }
                let world = self.special("world").ok_or_else(|| err("no world account"))?;
                self.do_transfer(world, acct, need, "margin_loan", req.day, req.ts, Some(cid), None);
                let d = self.st.margin.debts.entry((acct, cid)).or_insert_with(|| Debt {
                    account_id: acct,
                    company_id: cid,
                    principal: 0,
                    interest: 0,
                    opened_on: req.day.to_string(),
                    qty: 0,
                });
                d.principal += need;
                self.emit_debt((acct, cid));
            }
            Mode::ShortSell => {
                if self.holding_qty(acct, cid) > self.unsold(acct, cid) {
                    return Err(err("持有这支股票时不能融券卖出"));
                }
                if p < co.last_price {
                    return Err(OrderError(format!("融券卖出价格不能低于最新价 {}", crate::exchange::yuan_str(co.last_price))));
                }
                if self.lendable(cid) < qty {
                    return Err(OrderError(format!("可融券数量不足：最多 {} 股", self.lendable(cid))));
                }
                let value = qty * p.max(last);
                if !forced {
                    if self.exposure(acct, cid) + value > (self.margin.concentration * numbers.net() as f64) as i64 {
                        return Err(err("单支股票的融资融券不能超过净资产"));
                    }
                    if (numbers.available_margin as f64) < self.margin.margin_ratio * value as f64 {
                        return Err(err("可用保证金不足"));
                    }
                }
                let lender = self.lender_of(cid).ok_or_else(|| err("这支股票暂无券源"))?;
                self.do_remove_shares(lender, cid, qty, 0).map_err(err)?;
                self.do_add_shares(acct, cid, qty, last, req.day);
                let s = self.st.margin.shorts.entry((acct, cid)).or_insert_with(|| Short {
                    account_id: acct,
                    company_id: cid,
                    lender_id: lender,
                    qty: 0,
                    proceeds: 0,
                    fee: 0,
                    opened_on: req.day.to_string(),
                    amount: 0,
                });
                s.qty += qty;
                self.emit_short((acct, cid));
            }
            Mode::SellRepay => {
                if numbers.debt_principal + numbers.debt_interest <= 0 {
                    return Err(err("没有融资欠款"));
                }
                if self.available_shares(acct, cid) < qty {
                    return Err(err("可卖股数不足"));
                }
            }
            Mode::BuyReturn => {
                let owed = self.short_qty(acct, cid) - self.unsold(acct, cid);
                let pending: i64 = self
                    .open_credit_orders(acct)
                    .iter()
                    .filter(|o| o.1 == Mode::BuyReturn && o.2 == cid)
                    .map(|o| o.3)
                    .sum();
                if owed <= 0 {
                    return Err(err("没有这支股票的融券负债"));
                }
                if qty > owed - pending {
                    return Err(OrderError(format!("还券数量超过欠券：最多 {} 股", (owed - pending).max(0))));
                }
                let need = self.fees.buy_reserve(p, qty);
                let locked = self.short_of(acct, cid).map_or(0, |s| s.proceeds);
                from_locked = locked.min(need);
                if self.available_cash(acct) + from_locked < need {
                    return Err(err("可用资金不足"));
                }
                if from_locked > 0 {
                    self.add_cash(acct, 0, -from_locked);
                    self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().proceeds -= from_locked;
                    self.emit_short((acct, cid));
                }
            }
        }
        self.st.margin.pending = Some((mode, from_locked));
        let r = self.do_place_order(req);
        self.st.margin.pending = None;
        r
    }

    /// Hook: an order was just created (`do_place_order`). Tags it if it is a credit order.
    pub(crate) fn margin_tag(&mut self, order_id: i64) {
        if let Some((mode, from_locked)) = self.st.margin.pending.take() {
            let mo = MarginOrder { order_id, mode, from_locked };
            self.out.push(Change::Margin(MarginChange::Order(mo.clone())));
            self.st.margin.orders.insert(order_id, mo);
        }
    }

    /// Hook: may this ordinary order be placed? (No ordinary buy of a stock the account is short; no buying
    /// while being liquidated.)
    pub(crate) fn margin_plain_check(&self, account_id: i64, company_id: i64, side: Side) -> Result<(), OrderError> {
        if self.st.margin.pending.is_some() || side != Side::Buy {
            return Ok(());
        }
        let Some(c) = self.credit(account_id) else { return Ok(()) };
        if self.short_qty(account_id, company_id) > 0 {
            return Err(err("欠这支股票的券时，买入请用买券还券"));
        }
        if c.status == LIQUIDATING {
            return Err(err("强制平仓期间不能买入"));
        }
        Ok(())
    }

    /// Hook: a trade settled (`fill`), before either order is closed. `seller_net`: what the seller received
    /// after commission and stamp duty.
    pub(crate) fn margin_on_fill(&mut self, buy_id: i64, sell_id: i64, q: i64, tp: i64, seller_net: i64, day: &str, ts: &str) {
        if !self.st.margin.debts.is_empty() {
            // any sale of a stock bought on margin: the financed shares go first
            let key = {
                let o = &self.st.orders[&sell_id];
                (o.account_id, o.company_id)
            };
            if let Some(d) = self.st.margin.debts.get_mut(&key) {
                if d.qty > 0 {
                    d.qty -= q.min(d.qty);
                    self.emit_debt(key);
                }
            }
        }
        if self.st.margin.orders.is_empty() {
            return;
        }
        if let Some(mo) = self.st.margin.orders.get(&sell_id).cloned() {
            let (acct, cid) = { let o = &self.st.orders[&sell_id]; (o.account_id, o.company_id) };
            match mo.mode {
                Mode::ShortSell => {
                    self.add_cash(acct, 0, seller_net);
                    if let Some(s) = self.st.margin.shorts.get_mut(&(acct, cid)) {
                        s.proceeds += seller_net;
                        s.amount += seller_net;
                    }
                    self.emit_short((acct, cid));
                }
                Mode::SellRepay => {
                    let amount = seller_net.min(self.available_cash(acct));
                    self.repay(acct, Some(cid), amount, day, ts);
                }
                _ => {}
            }
        }
        if let Some(mo) = self.st.margin.orders.get(&buy_id).cloned() {
            let (acct, cid) = { let o = &self.st.orders[&buy_id]; (o.account_id, o.company_id) };
            if mo.mode == Mode::MarginBuy {
                if let Some(d) = self.st.margin.debts.get_mut(&(acct, cid)) {
                    d.qty += q;
                    self.emit_debt((acct, cid));
                }
            }
            if mo.mode == Mode::BuyReturn {
                // the returned shares' part of 融券卖出金额 goes, pro rata
                let sold = self.short_qty(acct, cid) - self.unsold(acct, cid);
                if let Some(s) = self.st.margin.shorts.get_mut(&(acct, cid)) {
                    if sold > 0 {
                        s.amount -= (s.amount as i128 * q.min(sold) as i128 / sold as i128) as i64;
                    }
                }
                let lender = self.short_of(acct, cid).map(|s| s.lender_id);
                if let Some(lender) = lender {
                    self.do_remove_shares(acct, cid, q, 0).unwrap_or_else(|e| panic!("buy_return {buy_id}: {e}"));
                    self.do_add_shares(lender, cid, q, tp, day);
                    self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().qty -= q;
                    self.settle_short(acct, cid, day, ts);
                }
            }
        }
    }

    /// Hook: an order ended (`close`); `before` is the order as it was (open, with its frozen cash).
    pub(crate) fn margin_on_close(&mut self, before: &crate::model::Order, ts: &str) {
        if before.status != OrderStatus::Open {
            return;
        }
        let Some(mo) = self.st.margin.orders.get(&before.id).cloned() else { return };
        let (acct, cid) = (before.account_id, before.company_id);
        let day = self.state("auction_day").map(str::to_string).unwrap_or_else(|| ts.get(..10).unwrap_or(ts).to_string());
        match mo.mode {
            Mode::MarginBuy => {
                // the unspent part of the loan goes back
                let left = before.frozen.min(self.st.margin.debts.get(&(acct, cid)).map_or(0, |d| d.principal));
                if left > 0 {
                    let world = self.special("world").expect("no world account");
                    self.do_transfer(acct, world, left, "margin_repay", &day, ts, Some(cid), None);
                    self.st.margin.debts.get_mut(&(acct, cid)).unwrap().principal -= left;
                    self.emit_debt((acct, cid));
                }
            }
            Mode::ShortSell => {
                // unsold borrowed shares go back to the lender
                let left = before.qty - before.filled;
                let lender = self.short_of(acct, cid).map(|s| s.lender_id);
                if let (true, Some(lender)) = (left > 0, lender) {
                    let price = self.last_price(cid);
                    self.do_remove_shares(acct, cid, left, 0).unwrap_or_else(|e| panic!("short_sell {}: {e}", before.id));
                    self.do_add_shares(lender, cid, left, price, &day);
                    self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().qty -= left;
                    self.settle_short(acct, cid, &day, ts);
                }
            }
            Mode::BuyReturn => {
                // locked proceeds the order did not spend are locked again (while shares are still owed)
                let y = before.frozen.min(mo.from_locked);
                if y > 0 && self.short_qty(acct, cid) > 0 {
                    self.add_cash(acct, 0, y);
                    self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().proceeds += y;
                    self.emit_short((acct, cid));
                }
            }
            Mode::SellRepay => {}
        }
    }

    /// Once nothing is owed in a stock: unlock what is left of the proceeds and pay the fees owed.
    fn settle_short(&mut self, acct: i64, cid: i64, day: &str, ts: &str) {
        let Some(s) = self.st.margin.shorts.get(&(acct, cid)).cloned() else { return };
        if s.qty > 0 {
            self.emit_short((acct, cid));
            return;
        }
        if s.proceeds > 0 {
            self.add_cash(acct, 0, -s.proceeds);
            self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().proceeds = 0;
        }
        self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().amount = 0;
        self.pay_fee(acct, cid, day, ts);
        self.emit_short((acct, cid));
    }

    /// Pay what can be paid of a short position's unpaid fees from free cash.
    fn pay_fee(&mut self, acct: i64, cid: i64, day: &str, ts: &str) {
        let fee = self.short_of(acct, cid).map_or(0, |s| s.fee);
        let pay = fee.min(self.available_cash(acct));
        if pay > 0 {
            let world = self.special("world").expect("no world account");
            self.do_transfer(acct, world, pay, "lending_fee", day, ts, Some(cid), None);
            self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().fee -= pay;
        }
    }

    /// Repay up to `amount` of financing from free cash: interest before principal, the contract of `first`
    /// before the others. Returns what was paid.
    fn repay(&mut self, acct: i64, first: Option<i64>, amount: i64, day: &str, ts: &str) -> i64 {
        let mut left = amount.min(self.available_cash(acct)).max(0);
        if left == 0 {
            return 0;
        }
        let world = self.special("world").expect("no world account");
        let mut keys: Vec<(i64, i64)> = self.debts_of(acct).map(|d| (d.account_id, d.company_id)).collect();
        keys.sort_by_key(|k| (Some(k.1) != first, k.1));
        let mut paid = 0;
        for pass in 0..2 {
            for key in &keys {
                if left == 0 {
                    break;
                }
                let Some(d) = self.st.margin.debts.get(key) else { continue };
                let (owed, reason) = if pass == 0 { (d.interest, "margin_interest") } else { (d.principal, "margin_repay") };
                let x = owed.min(left);
                if x <= 0 {
                    continue;
                }
                self.do_transfer(acct, world, x, reason, day, ts, Some(key.1), None);
                let d = self.st.margin.debts.get_mut(key).unwrap();
                if pass == 0 {
                    d.interest -= x;
                } else {
                    d.principal -= x;
                }
                left -= x;
                paid += x;
            }
        }
        for key in keys {
            self.emit_debt(key);
        }
        paid
    }

    /// 直接还款: repay financing with free cash (`amount` None: as much as possible). Returns what was paid.
    pub fn repay_cash(&mut self, account_id: i64, amount: Option<i64>, day: &str, ts: &str) -> Result<i64, OrderError> {
        if self.credit(account_id).is_none() {
            return Err(err("尚未开通信用账户"));
        }
        let owed: i64 = self.debts_of(account_id).map(|d| d.principal + d.interest).sum();
        if owed <= 0 {
            return Err(err("没有融资欠款"));
        }
        let want = amount.unwrap_or(owed).min(owed);
        if want <= 0 {
            return Err(err("还款金额必须大于 0"));
        }
        if self.available_cash(account_id) < want && amount.is_some() {
            return Err(err("可用资金不足"));
        }
        let paid = self.repay(account_id, None, want, day, ts);
        if paid == 0 {
            return Err(err("可用资金不足"));
        }
        self.after_repay(account_id);
        self.commit();
        Ok(paid)
    }

    /// A call is lifted as soon as the ratio is back above the call line.
    fn after_repay(&mut self, account_id: i64) {
        let Some(c) = self.credit(account_id) else { return };
        if c.status == CALL || c.status == LIQUIDATING {
            let n = self.credit_numbers(account_id, "");
            if n.ratio().is_none_or(|r| r >= self.margin.call_ratio) && c.status == CALL {
                self.set_status(account_id, NORMAL, None);
            }
            if n.liabilities == 0 {
                self.set_status(account_id, NORMAL, None);
            }
        }
    }

    // ------------------------------------------------------------ daily close

    /// Start of the daily close: cancel every open credit order (borrowed shares go back to the lender, unused
    /// loans are repaid) so neither earns a dividend or deposit interest overnight; forget closed tags.
    pub fn margin_pre_close(&mut self, ts: &str) {
        let mut ids: Vec<i64> = self
            .st
            .margin
            .orders
            .keys()
            .copied()
            .filter(|id| self.st.orders.get(id).is_some_and(|o| o.status == OrderStatus::Open))
            .collect();
        ids.sort_unstable();
        for id in ids {
            self.close(id, OrderStatus::Cancelled, ts);
        }
        self.st.margin.orders.clear();
    }

    /// A stock leaves the market paying `per` cents a share: its short sellers settle with the lender in cash
    /// at the same amount (to the world account, where the rights holders' payout goes too).
    pub fn margin_on_delist(&mut self, company_id: i64, per: i64, day: &str, ts: &str) {
        let keys: Vec<(i64, i64)> =
            self.st.margin.shorts.values().filter(|s| s.company_id == company_id && s.qty > 0).map(|s| (s.account_id, s.company_id)).collect();
        if keys.is_empty() {
            return;
        }
        let world = self.special("world").expect("no world account");
        for key in keys {
            let (acct, cid) = key;
            let s = self.st.margin.shorts.get_mut(&key).unwrap();
            let owed = s.qty * per;
            s.qty = 0;
            s.amount = 0;
            let unlock = s.proceeds;
            s.proceeds = 0;
            self.add_cash(acct, 0, -unlock);
            let pay = owed.min(self.available_cash(acct));
            self.do_transfer(acct, world, pay, "short_settle", day, ts, Some(cid), None);
            self.st.margin.shorts.get_mut(&key).unwrap().fee += owed - pay;
            self.pay_fee(acct, cid, day, ts);
            self.emit_short(key);
        }
    }

    /// After the day's dividends and delistings: manufactured dividends (Sunday), interest and lending fees,
    /// payment of what is owed from free cash, and the maintenance check (margin calls, liquidation).
    pub fn margin_post_close(&mut self, day: &str, ts: &str) {
        if self.st.margin.credit.is_empty() {
            return;
        }
        let world = self.special("world").expect("no world account");
        let (fr, lr) = (self.financing_rate(), self.lending_rate());
        // manufactured dividends
        let shorts: Vec<(i64, i64, i64)> = self.st.margin.shorts.values().filter(|s| s.qty > 0).map(|s| (s.account_id, s.company_id, s.qty)).collect();
        for &(acct, cid, qty) in &shorts {
            let dps = self.st.company_days.get(&(cid, day.to_string())).map_or(0, |r| r.dividend_ps);
            if dps > 0 {
                let owed = dps * qty;
                let pay = owed.min(self.available_cash(acct));
                self.do_transfer(acct, world, pay, "short_dividend", day, ts, Some(cid), None);
                self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().fee += owed - pay;
            }
        }
        // accrue (rounded up: a small balance is not free)
        for d in self.st.margin.debts.values_mut() {
            if d.principal > 0 {
                d.interest += (d.principal as f64 * fr / 365.0).ceil() as i64;
            }
        }
        for &(acct, cid, qty) in &shorts {
            let p = self.last_price(cid);
            let fee = ((qty * p) as f64 * lr / 365.0).ceil() as i64;
            self.st.margin.shorts.get_mut(&(acct, cid)).unwrap().fee += fee;
        }
        // pay from free cash: interest, then fees
        let accounts: Vec<i64> = self.st.margin.credit.keys().copied().collect();
        for &acct in &accounts {
            let interest: i64 = self.debts_of(acct).map(|d| d.interest).sum();
            if interest > 0 {
                let keys: Vec<(i64, i64)> = self.debts_of(acct).map(|d| (d.account_id, d.company_id)).collect();
                for key in keys {
                    let owed = self.st.margin.debts[&key].interest;
                    let pay = owed.min(self.available_cash(acct));
                    if pay > 0 {
                        self.do_transfer(acct, world, pay, "margin_interest", day, ts, Some(key.1), None);
                        self.st.margin.debts.get_mut(&key).unwrap().interest -= pay;
                    }
                }
            }
            let cids: Vec<i64> = self.shorts_of(acct).map(|s| s.company_id).collect();
            for cid in cids {
                self.pay_fee(acct, cid, day, ts);
            }
        }
        let dkeys: Vec<(i64, i64)> = self.st.margin.debts.keys().copied().collect();
        for k in dkeys {
            self.emit_debt(k);
        }
        let skeys: Vec<(i64, i64)> = self.st.margin.shorts.keys().copied().collect();
        for k in skeys {
            self.emit_short(k);
        }
        // maintenance
        for acct in accounts {
            let c = self.st.margin.credit[&acct].clone();
            if c.status == DEFAULTED {
                continue;
            }
            let n = self.credit_numbers(acct, day);
            match n.ratio() {
                None => self.set_status(acct, NORMAL, None),
                Some(r) => {
                    if n.net() < 0 {
                        self.set_status(acct, LIQUIDATING, None);
                    } else if r < self.margin.call_ratio {
                        match c.status.as_str() {
                            NORMAL => self.set_status(acct, CALL, Some(day.to_string())),
                            CALL if c.call_day.as_deref().is_some_and(|d| d < day) => self.set_status(acct, LIQUIDATING, None),
                            _ => {}
                        }
                    } else if c.status == CALL {
                        self.set_status(acct, NORMAL, None);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ intraday: liquidation

    /// Once a minute during continuous trading: lift calls that are covered, start liquidating accounts under
    /// the intraday line (or with negative equity), and run one round of every liquidation. Returns the
    /// accounts whose status changed.
    pub fn margin_sweep(&mut self, day: &str, ts: &str) -> Vec<i64> {
        if self.st.margin.credit.is_empty() {
            return vec![];
        }
        let before: Vec<(i64, String)> = self.st.margin.credit.values().map(|c| (c.account_id, c.status.clone())).collect();
        for (acct, status) in &before {
            if status == DEFAULTED {
                continue;
            }
            let n = self.credit_numbers(*acct, day);
            match n.ratio() {
                None => {
                    if status != NORMAL {
                        self.set_status(*acct, NORMAL, None);
                    }
                }
                Some(r) => {
                    if n.net() < 0 || r < self.margin.liquidate_ratio {
                        self.set_status(*acct, LIQUIDATING, None);
                    } else if status == CALL && r >= self.margin.call_ratio {
                        self.set_status(*acct, NORMAL, None);
                    }
                }
            }
            if self.credit(*acct).is_some_and(|c| c.status == LIQUIDATING) {
                if self.state("auction_day") != Some(day) {
                    self.open_market(day, ts);
                }
                self.liquidate_step(*acct, day, ts);
            }
        }
        self.commit();
        before
            .into_iter()
            .filter(|(a, s)| self.credit(*a).is_none_or(|c| &c.status != s))
            .map(|(a, _)| a)
            .collect()
    }

    /// Place a protected immediate-or-cancel order for a liquidation; returns shares filled.
    fn ioc(&mut self, acct: i64, cid: i64, mode: Option<Mode>, qty: i64, price: i64, day: &str, ts: &str) -> i64 {
        let side = match mode {
            Some(m) => m.side(),
            None => Side::Sell,
        };
        let req = NewOrder { account_id: acct, company_id: cid, side: side.as_str(), kind: "limit", qty, price: Some(price), day, ts, match_: true };
        let r = match mode {
            Some(m) => self.do_place_credit(&req, m, true),
            None => self.do_place_order(&req),
        };
        match r {
            Ok(p) => {
                if self.st.orders.get(&p.order_id).is_some_and(|o| o.status == OrderStatus::Open) {
                    self.close(p.order_id, OrderStatus::Cancelled, ts);
                }
                p.filled
            }
            Err(_) => 0,
        }
    }

    /// One round of forced liquidation (强制平仓) of `acct`.
    fn liquidate_step(&mut self, acct: i64, day: &str, ts: &str) {
        let ids: Vec<i64> = self.open_orders_of(acct).iter().map(|o| o.id).collect();
        for id in ids {
            self.close(id, OrderStatus::Cancelled, ts);
        }
        // free cash pays fees and interest, then principal
        let cids: Vec<i64> = self.shorts_of(acct).map(|s| s.company_id).collect();
        for cid in cids {
            self.pay_fee(acct, cid, day, ts);
            self.emit_short((acct, cid));
        }
        let free = self.available_cash(acct);
        self.repay(acct, None, free, day, ts);
        let n = self.credit_numbers(acct, day);
        let t = self.margin.target_ratio;
        if n.liabilities == 0 || n.ratio().is_some_and(|r| r >= t) && n.net() >= 0 {
            self.set_status(acct, NORMAL, None);
            return;
        }
        let longs: Vec<(i64, i64)> = {
            let mut v: Vec<(i64, i64)> = self
                .holdings_of(acct)
                .filter(|h| h.qty > 0)
                .map(|h| (h.qty * self.last_price(h.company_id), h.company_id))
                .collect();
            v.sort_by(|a, b| b.cmp(a));
            v
        };
        if n.net() < 0 && longs.is_empty() {
            self.default_account(acct, day, ts);
            return;
        }
        // value to unwind so that (A − x) / (L − x) reaches the target; everything when equity is negative
        let mut x = if n.net() < 0 { f64::INFINITY } else { (t * n.liabilities as f64 - n.gross() as f64) / (t - 1.0) };
        let mut shorts: Vec<(i64, i64)> = self.shorts_of(acct).filter(|s| s.qty > 0).map(|s| (s.qty * self.last_price(s.company_id), s.company_id)).collect();
        shorts.sort_by(|a, b| b.cmp(a));
        for (_, cid) in shorts {
            if x <= 0.0 {
                break;
            }
            let Some(co) = self.company(cid).cloned() else { continue };
            let lp = co.last_price.max(1);
            let (_, hi) = quote_band(&co, ts, &self.rules);
            let limit = ((lp as f64 * (1.0 + self.margin.protect)).ceil() as i64).min(hi).max(1);
            let cash = self.available_cash(acct) + self.short_of(acct, cid).map_or(0, |s| s.proceeds);
            let afford = cash / self.fees.buy_reserve(limit, 1).max(1);
            let want = if x.is_finite() { (x / lp as f64).ceil() as i64 } else { i64::MAX };
            let q = self.short_qty(acct, cid).min(want).min(afford);
            if q > 0 {
                let f = self.ioc(acct, cid, Some(Mode::BuyReturn), q, limit, day, ts);
                x -= (f * lp) as f64;
            }
        }
        for (_, cid) in longs {
            if x <= 0.0 {
                break;
            }
            let Some(co) = self.company(cid).cloned() else { continue };
            if co.status != "listed" {
                continue;
            }
            let lp = co.last_price.max(1);
            let (lo, _) = quote_band(&co, ts, &self.rules);
            let limit = ((lp as f64 * (1.0 - self.margin.protect)).floor() as i64).max(lo).max(1);
            let want = if x.is_finite() { (x / lp as f64).ceil() as i64 } else { i64::MAX };
            let q = self.available_shares(acct, cid).min(want);
            if q > 0 {
                let owes = self.debts_of(acct).any(|d| d.principal + d.interest > 0);
                let f = self.ioc(acct, cid, owes.then_some(Mode::SellRepay), q, limit, day, ts);
                x -= (f * lp) as f64;
            }
        }
        let n = self.credit_numbers(acct, day);
        if n.liabilities == 0 || n.ratio().is_some_and(|r| r >= t) && n.net() >= 0 {
            self.set_status(acct, NORMAL, None);
        }
    }

    /// 穿仓: nothing left to sell and equity negative. Remaining cash pays what it can (shares owed at the last
    /// price, then interest, then principal); the rest is the account's bad debt; credit is closed.
    fn default_account(&mut self, acct: i64, day: &str, ts: &str) {
        let world = self.special("world").expect("no world account");
        let shorts: Vec<Short> = self.shorts_of(acct).cloned().collect();
        for s in &shorts {
            if s.proceeds > 0 {
                self.add_cash(acct, 0, -s.proceeds);
            }
        }
        let mut cash = self.available_cash(acct);
        let mut unpaid = 0;
        for s in &shorts {
            let owed = s.qty * self.last_price(s.company_id) + s.fee;
            let pay = owed.min(cash);
            self.do_transfer(acct, world, pay, "short_settle", day, ts, Some(s.company_id), None);
            cash -= pay;
            unpaid += owed - pay;
            self.st.margin.shorts.remove(&(acct, s.company_id));
            self.out.push(Change::Margin(MarginChange::ShortDelete(acct, s.company_id)));
        }
        let debts: Vec<Debt> = self.debts_of(acct).cloned().collect();
        for d in &debts {
            let pi = d.interest.min(cash);
            self.do_transfer(acct, world, pi, "margin_interest", day, ts, Some(d.company_id), None);
            cash -= pi;
            let pp = d.principal.min(cash);
            self.do_transfer(acct, world, pp, "margin_repay", day, ts, Some(d.company_id), None);
            cash -= pp;
            unpaid += d.interest - pi + d.principal - pp;
            self.st.margin.debts.remove(&(acct, d.company_id));
            self.out.push(Change::Margin(MarginChange::DebtDelete(acct, d.company_id)));
        }
        let c = self.st.margin.credit.get_mut(&acct).unwrap();
        c.bad_debt += unpaid;
        c.status = DEFAULTED.into();
        c.call_day = None;
        self.emit_credit(acct);
    }

    // ------------------------------------------------------------ restructuring

    /// May this account restructure as far as credit is concerned? Err when it still owes (other than a
    /// default's bad debt). Ok(true) when it has defaulted (the season limit does not apply).
    pub fn margin_restructure_check(&self, account_id: i64) -> Result<bool, ()> {
        let Some(c) = self.credit(account_id) else { return Ok(false) };
        let owes = self.debts_of(account_id).next().is_some() || self.shorts_of(account_id).next().is_some();
        if owes {
            return Err(());
        }
        Ok(c.status == DEFAULTED)
    }

    /// Restructuring writes the bad debt off and closes the credit account.
    pub(crate) fn margin_write_off(&mut self, account_id: i64) {
        if self.st.margin.credit.remove(&account_id).is_some() {
            self.out.push(Change::Margin(MarginChange::CreditDelete(account_id)));
        }
    }

    // ------------------------------------------------------------ invariants

    /// A k-for-1 split (crate::corporate): every short position in the stock owes k times the shares, and every
    /// financing contract's bought shares (融资买入证券数量, V8) are k times as many (the amounts are unchanged).
    pub(crate) fn margin_on_split(&mut self, company_id: i64, k: i64) {
        let debts: Vec<(i64, i64)> = self.st.margin.debts.keys().filter(|key| key.1 == company_id).copied().collect();
        for key in debts {
            if let Some(d) = self.st.margin.debts.get_mut(&key) {
                d.qty *= k;
            }
            self.emit_debt(key);
        }
        let keys: Vec<(i64, i64)> =
            self.st.margin.shorts.values().filter(|s| s.company_id == company_id).map(|s| (s.account_id, s.company_id)).collect();
        for key in keys {
            if let Some(s) = self.st.margin.shorts.get_mut(&key) {
                s.qty *= k;
            }
            self.emit_short(key);
        }
    }

    /// Owed to the lender and not yet paid (a rights issue's compensation that free cash did not cover): paid
    /// with the lending fees.
    pub(crate) fn margin_add_fee(&mut self, account_id: i64, company_id: i64, amount: i64) {
        let key = (account_id, company_id);
        if let Some(s) = self.st.margin.shorts.get_mut(&key) {
            s.fee += amount;
            self.emit_short(key);
        }
    }

    pub(crate) fn margin_invariants(&self) -> Result<(), String> {
        let mut locked: HashMap<i64, i64> = HashMap::new();
        for s in self.st.margin.shorts.values() {
            if s.qty < 0 || s.proceeds < 0 || s.fee < 0 {
                return Err(format!("negative short position {s:?}"));
            }
            if !self.st.margin.credit.contains_key(&s.account_id) {
                return Err(format!("short position without credit account {s:?}"));
            }
            *locked.entry(s.account_id).or_default() += s.proceeds;
        }
        for (a, l) in locked {
            let frozen = self.account(a).map_or(0, |x| x.cash_frozen);
            if l > frozen {
                return Err(format!("locked short proceeds {l} exceed frozen cash {frozen} of account {a}"));
            }
        }
        for d in self.st.margin.debts.values() {
            if d.principal < 0 || d.interest < 0 || !self.st.margin.credit.contains_key(&d.account_id) {
                return Err(format!("bad debt row {d:?}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "margin_tests.rs"]
mod scenarios;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        let n = CreditNumbers { cash: 10000, stock_value: 3000, liabilities: 10000, ..Default::default() };
        assert_eq!(n.net(), 3000);
        assert_eq!(n.ratio(), Some(1.3));
        assert_eq!(CreditNumbers::default().ratio(), None);
        assert_eq!(Mode::parse("buy_return"), Some(Mode::BuyReturn));
        assert_eq!(Mode::ShortSell.side(), Side::Sell);
    }

    #[test]
    fn rules_from_toml() {
        let t: toml::Value = toml::from_str("haircut = 0.5\ncall_ratio = 1.4").unwrap();
        let r = MarginRules::from_table(Some(&t)).unwrap();
        assert_eq!(r.haircut, 0.5);
        assert_eq!(r.call_ratio, 1.4);
        assert_eq!(r.margin_ratio, 1.0);
    }
}
