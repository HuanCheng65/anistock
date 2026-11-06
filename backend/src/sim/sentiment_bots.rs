//! Exploit tests of the sentiment layer's second batch (docs/v2-sentiment.md §7): a submodule of `bots` so the
//! strategies share the bot's state. They see only public data: each session's opening price (the last close),
//! the stocks' heat (public popularity) and the order book.

use super::{Bot, BotKind};
use crate::engine::Engine;
use crate::market::{local, ts_of};

/// Run length and threshold of a bubble trigger as a player would reconstruct it from the rules in
/// server/game.toml: the return over the last 5 closes reaches 25% in a stock at least as hot as average.
const RUN_DAYS: usize = 5;
const RUN: f64 = 0.25;

pub(super) fn on_minute(b: &mut Bot, e: &mut Engine, t: i64, heat: &dyn Fn(i64) -> f64) {
    let (_, s) = local(t);
    if !b.exits.is_empty() {
        b.run_exits(e, t);
    }
    if s != 10 * 3600 {
        return;
    }
    let acct = b.account;
    match b.kind {
        BotKind::BubbleRider | BotKind::BubbleRider1 => {
            // buy each hot stock on the morning its run starts (a fifth of wealth each), sell k days later
            let k = if b.kind == BotKind::BubbleRider { 3 } else { 1 };
            let wealth = b.wealth(e);
            let starts: Vec<i64> = e
                .companies()
                .filter(|c| c.status == "listed" && c.last_price > 0 && heat(c.id) >= 1.0)
                .filter_map(|c| {
                    let o = b.opens.get(&c.id)?;
                    let n = o.len();
                    if n < RUN_DAYS + 2 {
                        return None;
                    }
                    let run = |end: usize| o[end] as f64 / o[end - RUN_DAYS].max(1) as f64 - 1.0;
                    (run(n - 1) >= RUN && run(n - 2) < RUN).then_some(c.id)
                })
                .collect();
            for c in starts {
                if b.exits.iter().any(|x| x.0 == c) {
                    continue;
                }
                b.buy(e, c, wealth / 5, t);
                b.exits.push((c, t + k * 86_400));
            }
        }
        BotKind::DispositionFade => {
            // Mondays: hold the five stocks of the hotter half (where retail agents trade) furthest under their
            // average close of the last 20 sessions (the capital-loss overhang, Grinblatt–Han's reference
            // price): if retail holders refuse to sell at a loss, these should be supported
            let (d, _) = local(t);
            let monday = (d + 4).rem_euclid(7) == 1;
            if !monday && b.since != 0 {
                return;
            }
            b.since = 1;
            let mut hot: Vec<(f64, i64)> = e.companies().filter(|c| c.status == "listed" && c.last_price >= 100).map(|c| (heat(c.id), c.id)).collect();
            hot.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
            let half = hot.len() / 2;
            let mut over: Vec<(f64, i64)> = hot
                .iter()
                .take(half)
                .filter_map(|&(_, c)| {
                    let o = b.opens.get(&c)?;
                    if o.len() < 20 {
                        return None;
                    }
                    let avg = o[o.len() - 20..].iter().sum::<i64>() as f64 / 20.0;
                    let p = e.company(c)?.last_price as f64;
                    Some(((p / avg).ln(), c))
                })
                .collect();
            if over.len() < 10 {
                return;
            }
            over.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
            let pick: Vec<i64> = over.iter().take(5).filter(|x| x.0 < 0.0).map(|x| x.1).collect();
            let held: Vec<i64> = e.holdings_of(acct).filter(|h| h.qty > 0).map(|h| h.company_id).collect();
            for c in held {
                if !pick.contains(&c) {
                    b.sell_all(e, c, t);
                }
            }
            let per = b.wealth(e) / 5;
            let ts = ts_of(t);
            for c in pick {
                let p = e.company(c).map_or(0, |x| x.last_price);
                let have = e.holding(acct, c).map_or(0, |h| h.qty) * p;
                if have < per * 8 / 10 {
                    e.cancel_all(acct, c, &ts);
                    b.buy(e, c, per - have, t);
                }
            }
        }
        _ => {}
    }
}
