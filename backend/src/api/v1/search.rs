//! `GET /api/v1/stocks/search` (docs/api-v1.md §4.2.0): find a stock by any of its names or its code.
//!
//! The name table lives in memory, rebuilt at startup and after each daily close from the game database
//! (title, title_ja, title_en) and the collector's snapshots (AniList `title.romaji` and `synonyms`,
//! Bangumi infobox 别名). A few hundred works with a handful of names each: a linear scan is enough.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{RawQuery, State};
use axum::Json;
use serde_json::Value;

use super::error::{self, sql, ErrorBody, R};
use super::market::{collector, snapshot};
use super::types::*;
use super::{read, Q};
use crate::api::AppState;
use crate::config::Config;

#[derive(Clone, Debug)]
struct Entry {
    id: i64,
    field: &'static str,
    text: String,
    norm: String,
    grams: HashSet<(char, char)>,
}

#[derive(Default)]
pub struct Names {
    entries: Vec<Entry>,
    codes: HashMap<String, i64>,
}

/// Full-width to half-width, traditional to simplified, punctuation and spaces dropped, lower case,
/// katakana to hiragana.
pub fn normalize(s: &str) -> String {
    let half: String = s
        .chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect();
    let simp = zhconv::zhconv(&half, zhconv::Variant::ZhHans);
    simp.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

fn grams(s: &str) -> HashSet<(char, char)> {
    let v: Vec<char> = s.chars().collect();
    if v.len() == 1 {
        return [(v[0], '\0')].into_iter().collect();
    }
    v.windows(2).map(|w| (w[0], w[1])).collect()
}

fn dice(a: &HashSet<(char, char)>, b: &HashSet<(char, char)>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    2.0 * a.intersection(b).count() as f64 / (a.len() + b.len()) as f64
}

impl Names {
    fn push(&mut self, id: i64, field: &'static str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let norm = normalize(text);
        if norm.is_empty() || self.entries.iter().any(|e| e.id == id && e.norm == norm) {
            return;
        }
        let grams = grams(&norm);
        self.entries.push(Entry { id, field, text: text.to_string(), norm, grams });
    }

    /// Build from the game database and the collector (read only).
    pub fn build(cfg: &Config, c: &rusqlite::Connection) -> rusqlite::Result<Names> {
        let mut n = Names::default();
        let mut s = c.prepare("SELECT id, code, title, title_ja, title_en FROM companies ORDER BY id")?;
        let rows: Vec<(i64, String, String, Option<String>, Option<String>)> =
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
        let matches = super::stocks::matches(cfg);
        let cdb = collector(cfg);
        for (id, code, title, ja, en) in rows {
            n.codes.insert(code, id);
            n.push(id, "title", &title);
            if let Some(t) = ja {
                n.push(id, "title_ja", &t);
            }
            if let Some(t) = en {
                n.push(id, "title_en", &t);
            }
            let Some(cdb) = cdb.as_ref() else { continue };
            if let Some(mm) = snapshot(cdb, "anilist", "media", &id.to_string()) {
                if let Some(r) = mm.get("title").and_then(|t| t.get("romaji")).and_then(Value::as_str) {
                    n.push(id, "title_romaji", r);
                }
                for syn in mm.get("synonyms").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(s) = syn.as_str() {
                        n.push(id, "alias", s);
                    }
                }
            }
            let bgm = matches.get(&id.to_string()).and_then(|e| e.get("bgm")).and_then(super::stocks::id_str);
            if let Some(subject) = bgm.and_then(|b| snapshot(cdb, "bgm", "subject", &b)) {
                for item in subject.get("infobox").and_then(Value::as_array).into_iter().flatten() {
                    if item.get("key").and_then(Value::as_str) != Some("别名") {
                        continue;
                    }
                    match item.get("value") {
                        Some(Value::String(s)) => n.push(id, "alias", s),
                        Some(Value::Array(a)) => {
                            for v in a {
                                if let Some(s) = v.get("v").and_then(Value::as_str) {
                                    n.push(id, "alias", s);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(n)
    }

    /// Best (score, field, text) per stock id, highest first.
    pub fn find(&self, q: &str) -> Vec<(i64, f64, &'static str, String)> {
        let mut best: HashMap<i64, (f64, &'static str, String)> = HashMap::new();
        let mut bump = |id: i64, score: f64, field: &'static str, text: &str| {
            let e = best.entry(id).or_insert((0.0, field, text.to_string()));
            if score > e.0 {
                *e = (score, field, text.to_string());
            }
        };
        let qt = q.trim();
        if qt.len() == 6 && qt.chars().all(|c| c.is_ascii_digit()) {
            if let Some(id) = self.codes.get(qt) {
                bump(*id, 1.0, "code", qt);
            }
        }
        let nq = normalize(qt);
        if !nq.is_empty() {
            let qg = grams(&nq);
            let ql = nq.chars().count() as f64;
            for e in &self.entries {
                let nl = e.norm.chars().count() as f64;
                let score = if e.norm == nq {
                    0.95
                } else if e.norm.starts_with(&nq) {
                    0.85 + 0.09 * (ql / nl)
                } else if e.norm.contains(&nq) {
                    0.75 + 0.09 * (ql / nl)
                } else {
                    let d = dice(&qg, &e.grams);
                    if d >= 0.3 {
                        0.7 * d
                    } else {
                        0.0
                    }
                };
                if score > 0.0 {
                    bump(e.id, score, e.field, &e.text);
                }
            }
        }
        let mut out: Vec<(i64, f64, &'static str, String)> =
            best.into_iter().map(|(id, (s, f, t))| (id, (s * 1e4).round() / 1e4, f, t)).collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        out
    }
}

/// Rebuild the name table (startup, after each close).
pub fn refresh(st: &AppState) {
    let res = st.reads.run(|c| Names::build(&st.cfg, c));
    match res {
        Ok(Ok(n)) => {
            let count = n.entries.len();
            *st.v1.names.write().unwrap() = n;
            eprintln!("search: {count} names");
        }
        Ok(Err(e)) | Err(e) => eprintln!("search: rebuilding the names failed: {e}"),
    }
}

/// 按名字或代码找股票
#[utoipa::path(get, path = "/api/v1/stocks/search", tag = "stocks",
    params(("q" = String, Query, description = "1–50 个字符"),
        ("limit" = Option<i64>, Query, description = "默认 5，最大 20"),
        ("status" = Option<String>, Query, description = "默认 listed,subscribing；可加 delisted")),
    responses((status = 200, body = SearchResponse), (status = "default", body = ErrorBody)))]
pub async fn search(State(st): State<Arc<AppState>>, RawQuery(q): RawQuery) -> R<Json<SearchResponse>> {
    let q = Q::parse(q.as_deref());
    let query = q.get("q").map(str::trim).unwrap_or("").to_string();
    let n = query.chars().count();
    if n == 0 {
        return Err(error::invalid(vec![error::missing("q")]));
    }
    if n > 50 {
        return Err(error::invalid(vec![error::field("q", "too_long", "搜索内容最多 50 个字符")]));
    }
    let limit = q.int("limit", 1, 20)?.unwrap_or(5) as usize;
    let statuses = q.list("status").unwrap_or_else(|| vec!["listed".into(), "subscribing".into()]);
    for s in &statuses {
        if !["listed", "subscribing", "delisted"].contains(&s.as_str()) {
            return Err(error::invalid(vec![error::not_allowed("status")]));
        }
    }
    let hits = st.v1.names.read().unwrap().find(&query);
    read(&st, move |c| {
        let mut s = c.prepare_cached("SELECT code, title, status, last_price, prev_close FROM companies WHERE id=?").map_err(sql)?;
        let mut items = Vec::new();
        for (id, score, field, text) in hits {
            if items.len() >= limit {
                break;
            }
            let row: Option<(String, String, String, i64, i64)> = s
                .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                .map(Some)
                .or_else(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e) })
                .map_err(sql)?;
            let Some((code, title, status, price, prev)) = row else { continue };
            if !statuses.contains(&status) {
                continue;
            }
            items.push(SearchHit {
                stock: StockRef { id, code, title },
                status,
                score,
                matched: SearchMatch { field: field.into(), text },
                price: m(price),
                change: super::market::ratio(price, prev),
            });
        }
        Ok(SearchResponse { query, items })
    })
    .await
    .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization() {
        assert_eq!(normalize("海賊王"), "海贼王");
        assert_eq!(normalize("ＯＮＥ　ＰＩＥＣＥ!"), "onepiece");
        assert_eq!(normalize("ワンピース"), "わんぴーす");
        assert_eq!(normalize("哆啦A梦 水田版"), "哆啦a梦水田版");
    }

    #[test]
    fn ranking() {
        let mut n = Names::default();
        n.codes.insert("100123".into(), 21);
        n.push(21, "title", "航海王");
        n.push(21, "alias", "海贼王");
        n.push(22, "title", "海贼王的猫");
        let r = n.find("海贼王");
        assert_eq!(r[0].0, 21);
        assert_eq!(r[0].1, 0.95);
        assert_eq!(r[0].2, "alias");
        assert_eq!(r[1].0, 22);
        assert_eq!(n.find("100123")[0].1, 1.0);
        assert!(n.find("航海").iter().any(|h| h.0 == 21 && h.1 > 0.85));
    }
}
