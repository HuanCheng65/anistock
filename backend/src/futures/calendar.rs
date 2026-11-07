//! Contract periods, anchored on each work's premiere (docs/v3-futures.md §4.1). Week 0 is the premiere day
//! and the six after it (a whole season released at once falls in it too); period k covers weeks
//! `weeks*(k-1)+1 ..= weeks*k`. Week 0 has no contract. Period k is listed on the first day of period k-1
//! (period 1 on the premiere), so the current and the next period trade side by side.

use chrono::{Duration, NaiveDate};

/// First and last day of period `k` (k >= 1).
pub fn period_days(premiere: NaiveDate, k: i64, weeks: i64) -> (NaiveDate, NaiveDate) {
    let first = premiere + Duration::days(7 * (weeks * (k - 1) + 1));
    (first, first + Duration::days(7 * weeks - 1))
}

/// The day period `k` is listed: the first day of period k-1, the premiere for period 1.
pub fn listing_day(premiere: NaiveDate, k: i64, weeks: i64) -> NaiveDate {
    if k <= 1 {
        premiere
    } else {
        period_days(premiere, k - 1, weeks).0
    }
}

/// The period listed on `day`, if one is.
pub fn listed_on(premiere: NaiveDate, day: NaiveDate, weeks: i64) -> Option<i64> {
    if day == premiere {
        return Some(1);
    }
    let since = (day - premiere).num_days() - 7; // days since period 1 began
    (since >= 0 && since % (7 * weeks) == 0).then(|| since / (7 * weeks) + 2)
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
        assert_eq!(listing_day(p, 1, 4), p);
        assert_eq!(listing_day(p, 2, 4), d("2026-10-10"));
        assert_eq!(listing_day(p, 3, 4), d("2026-11-07"));
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
        assert_eq!(listed, [1, 2, 3, 4, 5, 6]); // on days 0, 7, 35, 63, 91, 119
        assert_eq!(listed_on(p, d("2026-10-04"), 4), None, "nothing during week 0 after the premiere");
    }
}
