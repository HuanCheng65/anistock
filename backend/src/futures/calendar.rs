//! Contract periods, anchored on each work's premiere (docs/v3-futures.md §4.1). Week 0 is the premiere day
//! and the six after it (a whole season released at once falls in it too); period k covers weeks
//! `weeks*(k-1)+1 ..= weeks*k`. Week 0 has no contract. One contract per work at a time, so the players of a
//! work trade the same one: period 1 is listed on its first day, a week after the premiere (with a week of
//! data: at the premiere the forecast is mostly the prior), and each later period in the last week of the one
//! before it.

use chrono::{Duration, NaiveDate};

/// First and last day of period `k` (k >= 1).
pub fn period_days(premiere: NaiveDate, k: i64, weeks: i64) -> (NaiveDate, NaiveDate) {
    let first = premiere + Duration::days(7 * (weeks * (k - 1) + 1));
    (first, first + Duration::days(7 * weeks - 1))
}

/// The day period `k` is listed: its own first day for period 1, a week before its first day after that.
pub fn listing_day(premiere: NaiveDate, k: i64, weeks: i64) -> NaiveDate {
    let first = period_days(premiere, k, weeks).0;
    if k <= 1 {
        first
    } else {
        first - Duration::days(7)
    }
}

/// The period listed on `day`, if any.
pub fn listed_on(premiere: NaiveDate, day: NaiveDate, weeks: i64) -> Option<i64> {
    let since = (day - premiere).num_days();
    if since == 7 {
        return Some(1);
    }
    (since > 7 && since % (7 * weeks) == 0).then(|| since / (7 * weeks) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn periods_follow_the_premiere() {
        let p = d("2026-10-03");
        assert_eq!(period_days(p, 1, 4), (d("2026-10-10"), d("2026-11-06")));
        assert_eq!(period_days(p, 2, 4), (d("2026-11-07"), d("2026-12-04")));
        assert_eq!(listing_day(p, 1, 4), d("2026-10-10"));
        assert_eq!(listing_day(p, 2, 4), d("2026-10-31"), "the last week of period 1");
        assert_eq!(listing_day(p, 3, 4), d("2026-11-28"));
    }

    #[test]
    fn one_listing_day_per_period() {
        let p = d("2026-10-03");
        let mut listed = vec![];
        for n in -10..120 {
            let day = p + Duration::days(n);
            if let Some(k) = listed_on(p, day, 4) {
                assert_eq!(listing_day(p, k, 4), day);
                listed.push(k);
            }
        }
        assert_eq!(listed, [1, 2, 3, 4, 5]); // on days 7, 28, 56, 84, 112
        assert_eq!(listed_on(p, p, 4), None, "nothing in week 0");
    }
}
