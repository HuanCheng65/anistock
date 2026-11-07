//! 融资融券 (docs/v2-margin.md, docs/api-v1.md §4.5.11): the credit account, opening it, direct repayment, a
//! stock's margin terms, and the liabilities every asset figure subtracts. Credit orders go through
//! `POST /me/orders` with `credit`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::Json;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::{self, sql, ErrorBody, E, R};
use super::market::initial_cents;
use super::types::*;
use super::{body_int, engine, json_body_opt, Caller};
use crate::api::AppState;
use crate::config::Phase as CPhase;
use crate::engine::Engine;

#[derive(Serialize, ToSchema, Clone)]
pub struct CreditDebt {
    pub stock: StockRef,
    /// 融资本金
    pub principal: Money,
    /// 未付利息
    pub interest: Money,
    pub opened_on: TradingDay,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct CreditShort {
    pub stock: StockRef,
    /// 欠券数量
    pub quantity: i64,
    pub price: Money,
    /// 欠券市值（数量 × 最新价）
    pub value: Money,
    /// 冻结的融券卖出所得
    pub proceeds: Money,
    /// 未付的融券费用和分红补偿
    pub fee: Money,
    pub opened_on: TradingDay,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct CreditAccount {
    pub opened: bool,
    /// 未开通时：现在能否开通
    pub can_open: bool,
    /// 不能开通的原因
    pub open_message: Option<String>,
    /// 开放枚举：normal、call（追保）、liquidating（强制平仓中）、defaulted（已穿仓）；未开通时为 null
    pub status: Option<String>,
    /// 追保通知的交易日
    pub call_day: Option<TradingDay>,
    /// 现金 + 持仓市值 − 负债
    pub net_assets: Money,
    /// 现金 + 持仓市值
    pub gross_assets: Money,
    pub liabilities: Money,
    pub debt: Money,
    pub interest: Money,
    pub short_value: Money,
    pub fees: Money,
    /// 冻结的融券卖出所得
    pub locked: Money,
    /// 穿仓欠款
    pub bad_debt: Money,
    /// 可用保证金
    pub available_margin: Money,
    /// 维持担保比例；没有负债时为 null
    pub maintenance_ratio: Option<f64>,
    /// 融资年利率
    pub financing_rate: f64,
    /// 融券年费率
    pub lending_rate: f64,
    pub margin_ratio: f64,
    pub call_ratio: f64,
    pub liquidate_ratio: f64,
    pub target_ratio: f64,
    pub debts: Vec<CreditDebt>,
    pub shorts: Vec<CreditShort>,
}

#[derive(Serialize, ToSchema)]
pub struct CreditEnvelope {
    pub credit: CreditAccount,
}

#[derive(Deserialize, ToSchema)]
pub struct RepayRequest {
    /// 还款金额，整数分；省略时用可用资金还清全部
    pub amount: Option<Money>,
}

#[derive(Serialize, ToSchema)]
pub struct RepayResponse {
    pub paid: Money,
    pub credit: CreditAccount,
}

#[derive(Serialize, ToSchema)]
pub struct StockMargin {
    /// 是否融资融券标的
    pub eligible: bool,
    /// 开放枚举：new_listing、penny、delisting、not_listed；是标的时为 null
    pub reason: Option<String>,
    /// 折算率
    pub haircut: f64,
    /// 可融券数量
    pub lendable: i64,
    /// 剩余融资额度
    pub financing_room: Money,
    /// 融资余额：全部账户在这支股票上未还的融资本金
    pub financed: Money,
    /// 融券余量：全部账户在这支股票上的欠券股数
    pub shorted: i64,
    pub financing_rate: f64,
    pub lending_rate: f64,
}

fn sref(e: &Engine, id: i64) -> StockRef {
    e.company(id)
        .map(|c| StockRef { id, code: c.code.clone(), title: c.title.clone() })
        .unwrap_or(StockRef { id, code: String::new(), title: String::new() })
}

/// The credit account view, read on the engine thread.
pub fn credit_of(e: &Engine, id: i64, initial: i64, day: &str) -> CreditAccount {
    let c = e.credit(id).cloned();
    let n = e.credit_numbers(id, day);
    let refusal = if c.is_none() { e.credit_open_refusal(id, initial, day) } else { None };
    let r = &e.margin;
    CreditAccount {
        opened: c.is_some(),
        can_open: c.is_none() && refusal.is_none(),
        open_message: refusal,
        status: c.as_ref().map(|c| c.status.clone()),
        call_day: c.as_ref().and_then(|c| c.call_day.clone()).map(super::types::day),
        net_assets: m(n.net()),
        gross_assets: m(n.gross()),
        liabilities: m(n.liabilities),
        debt: m(n.debt_principal),
        interest: m(n.debt_interest),
        short_value: m(n.short_value),
        fees: m(n.short_fees),
        locked: m(n.locked),
        bad_debt: m(n.bad_debt),
        available_margin: m(if c.is_some() { n.available_margin.max(0) } else { 0 }),
        maintenance_ratio: n.ratio().map(r6),
        financing_rate: r6(e.financing_rate()),
        lending_rate: r6(e.lending_rate()),
        margin_ratio: r.margin_ratio,
        call_ratio: r.call_ratio,
        liquidate_ratio: r.liquidate_ratio,
        target_ratio: r.target_ratio,
        debts: e
            .debts_of(id)
            .map(|d| CreditDebt {
                stock: sref(e, d.company_id),
                principal: m(d.principal),
                interest: m(d.interest),
                opened_on: super::types::day(d.opened_on.clone()),
            })
            .collect(),
        shorts: e
            .shorts_of(id)
            .map(|s| {
                let p = e.company(s.company_id).map_or(0, |c| c.last_price);
                CreditShort {
                    stock: sref(e, s.company_id),
                    quantity: s.qty,
                    price: m(p),
                    value: m(s.qty * p),
                    proceeds: m(s.proceeds),
                    fee: m(s.fee),
                    opened_on: super::types::day(s.opened_on.clone()),
                }
            })
            .collect(),
    }
}

/// Liabilities (cents) and credit status of an account, from the database.
pub fn liabilities(c: &Connection, id: i64) -> R<(i64, i64, Option<String>)> {
    let (total, bad): (i64, i64) = c
        .query_row(
            "SELECT COALESCE((SELECT SUM(principal + interest) FROM margin_debts WHERE account_id=?1), 0)
               + COALESCE((SELECT SUM(s.qty * co.last_price + s.fee) FROM short_positions s JOIN companies co ON co.id = s.company_id
                           WHERE s.account_id=?1), 0)
               + COALESCE((SELECT bad_debt FROM credit_accounts WHERE account_id=?1), 0),
               COALESCE((SELECT bad_debt FROM credit_accounts WHERE account_id=?1), 0)",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(sql)?;
    let status: Option<String> =
        c.query_row("SELECT status FROM credit_accounts WHERE account_id=?", [id], |r| r.get(0)).optional().map_err(sql)?;
    Ok((total, bad, status))
}

/// SQL for the liabilities of the account whose id is the column `col` (the leaderboard).
pub fn liabilities_expr(col: &str) -> String {
    format!(
        "(COALESCE((SELECT SUM(principal + interest) FROM margin_debts WHERE account_id={col}), 0)
          + COALESCE((SELECT SUM(s.qty * co.last_price + s.fee) FROM short_positions s JOIN companies co ON co.id = s.company_id
                      WHERE s.account_id={col}), 0)
          + COALESCE((SELECT bad_debt FROM credit_accounts WHERE account_id={col}), 0))"
    )
}

/// Map the engine's credit rejections to API errors (None: not a credit message).
pub fn credit_error(msg: &str) -> Option<E> {
    let code = match msg {
        "尚未开通信用账户" => "credit_not_open",
        "信用账户已穿仓" => "credit_defaulted",
        "追保或强制平仓期间不能新增融资融券" => "margin_call_active",
        "这支股票不是融资融券标的" => "not_marginable",
        "可用保证金不足" => "insufficient_margin",
        "这支股票的融资额度已满" => "financing_cap",
        "单支股票的融资融券不能超过净资产" => "concentration",
        "持有这支股票时不能融券卖出" => "holds_long",
        "欠这支股票的券时，买入请用买券还券" => "has_short",
        "没有融资欠款" => "no_debt",
        "没有这支股票的融券负债" => "no_short",
        "强制平仓期间不能买入" => "liquidating",
        "信用委托只接受限价" => "credit_limit_only",
        "信用委托只在连续交易时段接受" => "credit_continuous_only",
        m if m.starts_with("可融券数量不足") => "lending_pool_empty",
        m if m.starts_with("融券卖出价格不能低于最新价") => "short_price_rule",
        m if m.starts_with("还券数量超过欠券") => "return_too_many",
        _ => return None,
    };
    Some(E::new(422, code, msg.to_string()))
}

/// 信用账户
#[utoipa::path(get, path = "/api/v1/me/credit", tag = "account",
    responses((status = 200, body = CreditEnvelope), (status = "default", body = ErrorBody)))]
pub async fn credit(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<CreditEnvelope>> {
    let id = caller.reader()?;
    let (initial, day) = (initial_cents(&st.cfg), st.clock.today());
    let credit = engine(&st, move |ctx| credit_of(&ctx.engine, id, initial, &day)).await?;
    Ok(Json(CreditEnvelope { credit }))
}

/// 开通信用账户
#[utoipa::path(post, path = "/api/v1/me/credit", tag = "account",
    responses((status = 200, body = CreditEnvelope), (status = "default", body = ErrorBody)))]
pub async fn open(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Json<CreditEnvelope>> {
    let id = caller.player()?;
    super::account::require_signed(&st, id).await?;
    // 风险测评, the knowledge test and the margin disclosure (docs/v2-account.md §6)
    let st2 = st.clone();
    let steps = super::read(&st, move |c| super::assess::credit_steps(&st2, c, id)).await?;
    if let Some(e) = super::assess::credit_refusal(&steps) {
        return Err(e);
    }
    let _ = json_body_opt(&headers, &body)?;
    let (initial, day, t) = (initial_cents(&st.cfg), st.clock.today(), st.clock.ts());
    let credit = engine(&st, move |ctx| -> R<CreditAccount> {
        let e = &mut ctx.engine;
        if e.credit(id).is_some_and(|c| c.status != crate::margin::DEFAULTED) {
            return Err(E::new(409, "credit_open", "信用账户已经开通"));
        }
        e.open_credit(id, initial, &day, &t).map_err(|err| E::new(422, "credit_not_eligible", err.0))?;
        e.flush().map_err(error::internal)?;
        Ok(credit_of(e, id, initial, &day))
    })
    .await??;
    Ok(Json(CreditEnvelope { credit }))
}

/// 直接还款
#[utoipa::path(post, path = "/api/v1/me/credit/repay", tag = "account",
    request_body = RepayRequest,
    responses((status = 200, body = RepayResponse), (status = "default", body = ErrorBody)))]
pub async fn repay(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Json<RepayResponse>> {
    let id = caller.trader(&st).await?.account;
    let b = json_body_opt(&headers, &body)?;
    let amount = body_int(&b, "amount")?;
    if amount.is_some_and(|a| a < 1) {
        return Err(error::invalid(vec![error::too_small("amount", 1)]));
    }
    if st.phase() == CPhase::Closed {
        return Err(E::new(422, "market_closed", "休市时无法还款"));
    }
    let (initial, day, t) = (initial_cents(&st.cfg), st.clock.today(), st.clock.ts());
    let (paid, credit) = engine(&st, move |ctx| -> R<(i64, CreditAccount)> {
        let e = &mut ctx.engine;
        let paid = e.repay_cash(id, amount, &day, &t).map_err(|err| match err.0.as_str() {
            "可用资金不足" => E::new(422, "insufficient_cash", "可用资金不足"),
            m => credit_error(m).unwrap_or_else(|| E::new(422, "repay_rejected", m.to_string())),
        })?;
        e.flush().map_err(error::internal)?;
        Ok((paid, credit_of(e, id, initial, &day)))
    })
    .await??;
    Ok(Json(RepayResponse { paid: m(paid), credit }))
}

/// 融资融券条件
#[utoipa::path(get, path = "/api/v1/stocks/{id}/margin", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = StockMargin), (status = "default", body = ErrorBody)))]
pub async fn stock_margin(State(st): State<Arc<AppState>>, Path(raw): Path<String>) -> R<Json<StockMargin>> {
    let id = super::path_id(&raw, "id")?;
    let day = st.clock.today();
    engine(&st, move |ctx| -> R<StockMargin> {
        let e = &ctx.engine;
        if e.company(id).is_none() {
            return Err(error::stock_not_found(id));
        }
        let s = e.stock_margin(id, &day);
        let mg = &e.st.margin;
        let financed: i64 = mg.debts.values().filter(|d| d.company_id == id).map(|d| d.principal).sum();
        let shorted: i64 = mg.shorts.values().filter(|x| x.company_id == id).map(|x| x.qty).sum();
        Ok(StockMargin {
            eligible: s.eligible,
            reason: s.reason.map(str::to_string),
            haircut: s.haircut,
            lendable: s.lendable,
            financing_room: m(s.financing_room),
            financed: m(financed),
            shorted,
            financing_rate: r6(e.financing_rate()),
            lending_rate: r6(e.lending_rate()),
        })
    })
    .await?
    .map(Json)
}
