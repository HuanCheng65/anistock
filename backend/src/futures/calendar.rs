//! Contract periods, anchored on each work's premiere (docs/design/futures.md §4.1). Week 0 is the premiere day
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

/// The period that lists on `day`, if any: period 1 on its first day; a later one on its listing day or on any day
/// of the week after, before it starts, so a close that did not run (or a deploy mid-week) only delays it. The
/// caller skips a period already listed.
pub fn listed_on(premiere: NaiveDate, day: NaiveDate, weeks: i64) -> Option<i64> {
    let since = (day - premiere).num_days();
    if since == 7 {
        return Some(1);
    }
    (since > 7 && since % (7 * weeks) < 7).then(|| since / (7 * weeks) + 1)
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
    fn each_period_lists_before_it_starts() {
        let p = d("2026-10-03");
        let mut first_seen = std::collections::BTreeMap::new();
        for n in -10..120 {
            let day = p + Duration::days(n);
            if let Some(k) = listed_on(p, day, 4) {
                assert!(day >= listing_day(p, k, 4), "not before its listing day");
                assert!(k == 1 || day < period_days(p, k, 4).0, "a later period only before it starts");
                first_seen.entry(k).or_insert(day);
            }
        }
        // each first on its listing day (days 7, 28, 56, 84, 112)
        assert_eq!(first_seen.keys().copied().collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
        assert!(first_seen.iter().all(|(k, day)| *day == listing_day(p, *k, 4)));
        assert_eq!(listed_on(p, p, 4), None, "nothing in week 0");
        assert_eq!(listed_on(p, p + Duration::days(8), 4), None, "period 1 only on its first day");
        // a missed listing day: still listed later that week, not once period 2 has begun (day 35)
        assert_eq!(listed_on(p, p + Duration::days(31), 4), Some(2));
        assert_eq!(listed_on(p, p + Duration::days(35), 4), None);
    }
}
