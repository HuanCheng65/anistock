//! Listing contracts at the daily close (docs/v3-futures.md §4.1): each period once, on its listing day, if
//! the model's forecast of its average index clears the floor and the work's company is listed and not on
//! its way out. A forecast cut later does not delist a contract.

use chrono::NaiveDate;

use super::calendar::{listed_on, period_days};
use super::{Contract, ContractStatus, FuturesChange, FuturesRules};
use crate::engine::Engine;
use crate::store::persist::Change;

/// A work futures may be listed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Underlying {
    pub company_id: i64,
    pub work_id: i64,
    pub premiere: NaiveDate,
}

impl Engine {
    pub(crate) fn fut_out(&mut self, c: FuturesChange) {
        self.out.push(Change::Futures(c));
    }

    /// List the periods whose listing day is `on` (the next trading day). `forecast(work, period, first, last)` is the
    /// model's forecast of the average index over those days, in index points. Returns the new contracts' ids.
    pub(crate) fn futures_list(
        &mut self,
        rules: &FuturesRules,
        today: &str,
        on: NaiveDate,
        works: &[Underlying],
        forecast: impl Fn(&Underlying, i64, NaiveDate, NaiveDate) -> Option<f64>,
    ) -> Vec<i64> {
        let mut new = vec![];
        for u in works {
            let Some(k) = listed_on(u.premiere, on, rules.weeks) else { continue };
            {
                if !self.may_underlie(u.company_id) || self.st.futures.on_work(u.work_id).any(|c| c.period == k) {
                    continue;
                }
                let (first, last) = period_days(u.premiere, k, rules.weeks);
                let Some(mean) = forecast(u, k, first, last).filter(|m| *m >= rules.listing_floor) else { continue };
                let ticks = (mean * 100.0).round() as i64;
                let f = &mut self.st.futures;
                f.last_contract_id += 1;
                let c = Contract {
                    id: f.last_contract_id,
                    code: format!("HF{}", f.last_contract_id),
                    company_id: u.company_id,
                    work_id: u.work_id,
                    period: k,
                    first_day: first.to_string(),
                    last_day: last.to_string(),
                    listed_on: today.to_string(),
                    status: ContractStatus::Listed,
                    tick_value: rules.tick_value,
                    forecast: ticks,
                    prev_settle: ticks,
                    open_ref: None,
                    vcm_until: None,
                    vcm_low: None,
                    vcm_high: None,
                    vcm_count: 0,
                    last_price: ticks,
                    final_price: None,
                    settled_on: None,
                };
                f.contracts.insert(c.id, c.clone());
                new.push(c.id);
                self.fut_out(FuturesChange::Contract(c.clone()));
                self.fmm_fund(c.id, today, &format!("{today}T06:30:00"));
            }
        }
        new
    }

    /// A listed company not counting down to delisting (weak revenue, or under 1 币).
    fn may_underlie(&self, company_id: i64) -> bool {
        let Some(c) = self.st.companies.get(&company_id) else { return false };
        c.status == "listed" && c.low_weeks == 0 && self.st.penny_days.get(&company_id).copied().unwrap_or(0) == 0
    }
}
