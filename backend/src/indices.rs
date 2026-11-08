//! The index rules (docs/api-v1.md §4.1.4, docs/v3-funds.md §3), shared by the published indices
//! (`api::v1::market::compute_indices`, from the candles) and the funds that track them (`crate::funds`, from the
//! engine's state), so a fund follows exactly the index it is named after.
//!
//! Every index is weighted by free-float market value (shares not held by the production committee or the company)
//! as of the day before. Members are the stocks with a close that day and the day before (a new listing joins from
//! its second close; a delisted one keeps its history up to its delisting), selected by the index's rule:
//! - `All`: every stock; `Season`: not a long-runner, first aired on or after the season's start less 14 days;
//!   `Long`: long-runners; `Source`: AniList source; `Genre`: AniList genre or theme tag (company labels, V21);
//! - `Top(n)`: the n largest by free-float market value, chosen on the first market day of each month from the
//!   values of the day before, kept until the next month (a delisting leaves the index at once).
//!
//! An index with a cap holds no stock above that share of the index: the excess is shared out over the others in
//! proportion to their values.

/// What an index selects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rule {
    All,
    Season,
    Long,
    Top(usize),
    Source(&'static str),
    Genre(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct IndexDef {
    pub key: &'static str,
    pub name: &'static str,
    pub rule: Rule,
    /// the largest share one stock may have (None: no cap)
    pub cap: Option<f64>,
    /// on the market page (the others: the funds' indices)
    pub market: bool,
}

pub const DEFS: [IndexDef; 9] = [
    IndexDef { key: "all", name: "番剧综合指数", rule: Rule::All, cap: None, market: true },
    IndexDef { key: "season", name: "本季新番", rule: Rule::Season, cap: None, market: true },
    IndexDef { key: "long", name: "长篇连载", rule: Rule::Long, cap: Some(0.25), market: true },
    IndexDef { key: "top20", name: "安易 20", rule: Rule::Top(20), cap: Some(0.25), market: false },
    IndexDef { key: "manga", name: "漫改", rule: Rule::Source("MANGA"), cap: Some(0.25), market: false },
    IndexDef { key: "light_novel", name: "轻改", rule: Rule::Source("LIGHT_NOVEL"), cap: Some(0.25), market: false },
    IndexDef { key: "isekai", name: "异世界", rule: Rule::Genre("Isekai"), cap: Some(0.25), market: false },
    IndexDef { key: "romance", name: "恋爱", rule: Rule::Genre("Romance"), cap: Some(0.25), market: false },
    IndexDef { key: "action", name: "热血", rule: Rule::Genre("Action"), cap: Some(0.25), market: false },
];

pub fn def(key: &str) -> Option<&'static IndexDef> {
    DEFS.iter().find(|d| d.key == key)
}

/// What a rule looks at in a company.
pub struct Labels<'a> {
    pub long: bool,
    pub first_air: &'a str,
    pub source: Option<&'a str>,
    /// comma-separated (companies.genres)
    pub genres: &'a str,
}

/// Whether a stock is in a rule's universe (`Top` selects later, by value: every stock is a candidate).
/// `season_from`: the earliest premiere of the season on the day (`api::v1::market::season_from`).
pub fn eligible(rule: Rule, l: &Labels, season_from: &str) -> bool {
    match rule {
        Rule::All | Rule::Top(_) => true,
        Rule::Season => !l.long && l.first_air >= season_from,
        Rule::Long => l.long,
        Rule::Source(s) => l.source == Some(s),
        Rule::Genre(g) => l.genres.split(',').any(|x| x == g),
    }
}

/// Whether `day` starts a new month after `prev` (the market day before; None: the first day): a `Top` review day.
pub fn is_review(day: &str, prev: Option<&str>) -> bool {
    prev.is_none_or(|p| p.get(..7) != day.get(..7))
}

/// The `n` largest of (id, value), ties to the lower id.
pub fn top(mut values: Vec<(i64, f64)>, n: usize) -> Vec<i64> {
    values.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    values.into_iter().take(n).map(|x| x.0).collect()
}

/// Weights in proportion to `values`, none above `cap`: the excess of the capped is shared out over the rest in
/// proportion, repeated until none exceeds it (with fewer than 1 / cap stocks: equal weights).
pub fn weights(values: &[(i64, f64)], cap: Option<f64>) -> Vec<(i64, f64)> {
    let total: f64 = values.iter().map(|x| x.1.max(0.0)).sum();
    if total <= 0.0 {
        return values.iter().map(|x| (x.0, 0.0)).collect();
    }
    let mut w: Vec<(i64, f64)> = values.iter().map(|x| (x.0, x.1.max(0.0) / total)).collect();
    let Some(cap) = cap else { return w };
    if (w.len() as f64) * cap <= 1.0 {
        let e = 1.0 / w.len() as f64;
        return w.into_iter().map(|x| (x.0, e)).collect();
    }
    let mut fixed = vec![false; w.len()];
    loop {
        let over: Vec<usize> = (0..w.len()).filter(|&i| !fixed[i] && w[i].1 > cap + 1e-12).collect();
        if over.is_empty() {
            break;
        }
        for &i in &over {
            fixed[i] = true;
            w[i].1 = cap;
        }
        let left = 1.0 - fixed.iter().filter(|f| **f).count() as f64 * cap;
        let free: f64 = (0..w.len()).filter(|&i| !fixed[i]).map(|i| w[i].1).sum();
        if free <= 0.0 {
            break;
        }
        for i in 0..w.len() {
            if !fixed[i] {
                w[i].1 *= left / free;
            }
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cap_shares_the_excess_out() {
        // one stock at 79% of a long-runner index of five, capped at 25%
        let w = weights(&[(1, 79.0), (2, 7.0), (3, 7.0), (4, 5.0), (5, 2.0)], Some(0.25));
        let get = |id: i64| w.iter().find(|x| x.0 == id).unwrap().1;
        assert!((get(1) - 0.25).abs() < 1e-9);
        assert!((w.iter().map(|x| x.1).sum::<f64>() - 1.0).abs() < 1e-9);
        // 2 and 3 hit the cap in the second round; 4 and 5 share the rest 5:2
        assert!((get(2) - 0.25).abs() < 1e-9 && (get(3) - 0.25).abs() < 1e-9);
        assert!((get(4) / get(5) - 2.5).abs() < 1e-9);
        // no cap: proportional
        let w = weights(&[(1, 3.0), (2, 1.0)], None);
        assert_eq!(w, vec![(1, 0.75), (2, 0.25)]);
    }

    #[test]
    fn rules_and_reviews() {
        let l = Labels { long: false, first_air: "2026-10-03", source: Some("MANGA"), genres: "Action,Fantasy,Isekai" };
        assert!(eligible(Rule::Genre("Isekai"), &l, "2026-09-17"));
        assert!(!eligible(Rule::Genre("Romance"), &l, "2026-09-17"));
        assert!(eligible(Rule::Source("MANGA"), &l, "2026-09-17"));
        assert!(eligible(Rule::Season, &l, "2026-09-17"));
        assert!(is_review("2026-11-02", Some("2026-10-31")) && !is_review("2026-11-03", Some("2026-11-02")) && is_review("2026-07-01", None));
        assert_eq!(top(vec![(1, 5.0), (2, 9.0), (3, 9.0), (4, 1.0)], 2), vec![2, 3]);
    }
}
