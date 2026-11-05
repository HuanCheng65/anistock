//! IPO subscriptions (公开认购): v1 `app.subscribe`, `app._withdraw`, `app.unsubscribe`, as engine
//! operations because they freeze cash. Allotment is part of the daily close.

use crate::engine::Engine;
use crate::exchange::OrderError;
use crate::model::IpoApplication;
use crate::pyfmt::{fixed_grouped, int_grouped};
use crate::store::persist::Change;

impl Engine {
    /// An account's application for a company, if any.
    pub fn ipo_application(&self, company_id: i64, account_id: i64) -> Option<&IpoApplication> {
        self.st.ipo_apps.get(&(company_id, account_id))
    }

    /// All applications for a company, by account id.
    pub fn ipo_applications(&self, company_id: i64) -> Vec<&IpoApplication> {
        self.st.ipo_apps.range((company_id, i64::MIN)..=(company_id, i64::MAX)).map(|(_, a)| a).collect()
    }

    fn do_withdraw(&mut self, company_id: i64, account_id: i64) {
        if let Some(old) = self.st.ipo_apps.remove(&(company_id, account_id)) {
            self.add_cash(account_id, 0, -old.frozen);
            self.out.push(Change::IpoAppDelete { company_id, account_id });
        }
    }

    /// Apply for `qty` shares (replacing any earlier application; `qty == 0` just withdraws). `today` is the
    /// game day. Returns the cash frozen. The API must still reject qty < 0 or > 10,000,000 (v1's pydantic
    /// model) before calling.
    pub fn subscribe(
        &mut self,
        account_id: i64,
        company_id: i64,
        qty: i64,
        today: &str,
        ts: &str,
    ) -> Result<i64, OrderError> {
        let c = match self.company(company_id) {
            Some(c) if c.status == "subscribing" => c,
            _ => return Err(OrderError::new("这支新股不在认购期")),
        };
        let (start, end) = (c.sub_start.clone().unwrap_or_default(), c.sub_end.clone().unwrap_or_default());
        if !(start.as_str() <= today && today <= end.as_str()) {
            return Err(OrderError(format!("认购时间是 {start} 至 {end}")));
        }
        let price = c.ipo_price;
        let mut frozen = 0;
        if qty > 0 {
            frozen = self.fees.buy_reserve(price, qty);
            // v1 withdraws the old application first, inside the same transaction
            let released = self.ipo_application(company_id, account_id).map_or(0, |a| a.frozen);
            if self.available_cash(account_id) + released < frozen {
                return Err(OrderError(format!(
                    "可用资金不足：认购 {} 股需要冻结 {} 币",
                    int_grouped(qty),
                    fixed_grouped(frozen as f64 / 100.0, 2)
                )));
            }
        }
        self.do_withdraw(company_id, account_id);
        if qty > 0 {
            self.add_cash(account_id, 0, frozen);
            let app =
                IpoApplication { company_id, account_id, qty, frozen, allotted: None, created_at: ts.to_string() };
            self.out.push(Change::IpoAppInsert(app.clone()));
            self.st.ipo_apps.insert((company_id, account_id), app);
        }
        self.commit();
        Ok(frozen)
    }

    /// Withdraw an application while the subscription is open.
    pub fn unsubscribe(&mut self, account_id: i64, company_id: i64) -> Result<(), OrderError> {
        match self.company(company_id) {
            Some(c) if c.status == "subscribing" => {}
            _ => return Err(OrderError::new("认购已经截止，不能撤回")),
        }
        self.do_withdraw(company_id, account_id);
        self.commit();
        Ok(())
    }
}
