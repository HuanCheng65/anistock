//! External news from the news-feed stock extension (docs/v3-news.md): the QQ bot's token pushes one
//! judged news event per request. Every push is archived (`media_news`, on the side connection); what the
//! game does with it depends on `[news] mode`:
//! - `shadow`: archived only, for labelling and calibrating the extension's thresholds;
//! - `publish`: also a game news item (kind `media`, category 媒体) the first time, never edited after;
//! - `market` (not yet): also an event for the NPC market.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use rusqlite::{params, OptionalExtension};

use super::bind::with_side;
use super::error::{self, sql, ErrorBody, E, R};
use super::types::*;
use super::{body_int, body_str, engine, json_body, need, Caller};
use crate::api::AppState;

pub const EVENTS: [&str; 12] =
    ["sequel", "movie", "delay", "hiatus", "sales", "award", "collab", "controversy", "staff", "rumor", "denial", "other"];
const DIRECTIONS: [&str; 3] = ["positive", "negative", "neutral"];
const CREDIBILITY: [&str; 3] = ["official", "media", "rumor"];

fn credibility_label(c: &str) -> &'static str {
    match c {
        "official" => "官方发布",
        "media" => "媒体报道",
        _ => "传闻，未经证实",
    }
}

/// A media news item's headline: the extension's, with the work's title in front when it lacks it.
fn headline(title: &str, title_of: &str) -> String {
    if title.contains(title_of) {
        title.to_string()
    } else {
        format!("《{title_of}》{title}")
    }
}

/// A media news item's body: the source, its credibility, (an item published late: the day the source ran it), the
/// original title and the link.
fn news_body(source: &str, credibility: &str, reported: Option<&str>, original_title: &str, url: &str) -> String {
    let when = reported
        .and_then(|d| chrono::NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d").ok())
        .map(|d| format!("，{}月{}日报道", chrono::Datelike::month(&d), chrono::Datelike::day(&d)))
        .unwrap_or_default();
    format!("{source}，{}{when}。原文：{original_title}\n{url}", credibility_label(credibility))
}

/// Publish, once, the items archived while [news] mode was `shadow` (startup, mode `publish` or `market`): each
/// not merged, not published, about a stock still listed, oldest first, dated today with the day the source ran
/// it in the body. Later pushes publish as they come; an item published here is never published again.
pub fn publish_backlog(engine: &mut crate::engine::Engine, game_db: &std::path::Path, day: &str, ts: &str) -> Result<usize, String> {
    let c = rusqlite::Connection::open(game_db).map_err(|e| e.to_string())?;
    let has = c
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='media_news'", [], |_| Ok(()))
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();
    if !has {
        return Ok(0);
    }
    type Row = (i64, i64, String, String, String, String, Option<String>, String);
    let rows: Vec<Row> = {
        let mut s = c
            .prepare(
                "SELECT id, company_id, source, url, title, original_title, coalesce(published_at, fetched_at), credibility
                 FROM media_news WHERE news_id IS NULL AND status='stored' AND merged_into IS NULL ORDER BY received_at, id",
            )
            .map_err(|e| e.to_string())?;
        let it = s
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))
            .map_err(|e| e.to_string())?;
        it.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    let mut n = 0;
    for (id, company, source, url, title, original, reported, cred) in rows {
        let Some(title_of) = engine.company(company).filter(|co| co.status == "listed").map(|co| co.title.clone()) else { continue };
        engine.post_news(ts, day, "media", Some(company), headline(&title, &title_of), news_body(&source, &cred, reported.as_deref(), &original, &url));
        let nid = engine.st.last_news_id;
        // the news row first: the archive row points at it
        engine.flush()?;
        c.execute("UPDATE media_news SET news_id=?, status='published', updated_at=? WHERE id=?", params![nid, ts, id])
            .map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}

fn rfc3339(b: &serde_json::Map<String, serde_json::Value>, k: &str, required: bool) -> R<Option<String>> {
    let v = body_str(b, k)?;
    match v {
        None if required => Err(error::invalid(vec![error::missing(k)])),
        None => Ok(None),
        Some(s) => match chrono::DateTime::parse_from_rfc3339(&s) {
            Ok(_) => Ok(Some(s)),
            Err(_) => Err(error::invalid(vec![error::bad_format(k)])),
        },
    }
}

fn one_of(b: &serde_json::Map<String, serde_json::Value>, k: &str, allowed: &[&str]) -> R<String> {
    let v = need(body_str(b, k)?, k)?;
    if allowed.contains(&v.as_str()) { Ok(v) } else { Err(error::invalid(vec![error::not_allowed(k)])) }
}

fn text(b: &serde_json::Map<String, serde_json::Value>, k: &str, max: usize) -> R<String> {
    let v = need(body_str(b, k)?, k)?.trim().to_string();
    if v.is_empty() {
        return Err(error::invalid(vec![error::missing(k)]));
    }
    if v.chars().count() > max {
        return Err(error::invalid(vec![error::field(k, "too_long", format!("最多 {max} 字"))]));
    }
    Ok(v)
}

/// 推送一条外部新闻（机器人）
#[utoipa::path(post, path = "/api/v1/bot/news", tag = "bot",
    request_body = MediaNewsRequest,
    responses((status = 200, body = MediaNewsResponse), (status = "default", body = ErrorBody)),
    security(("bot" = [])))]
pub async fn push(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<axum::Json<MediaNewsResponse>> {
    if !matches!(caller, Caller::Bot { .. }) {
        return Err(error::forbidden("bot"));
    }
    let b = json_body(&headers, &body)?;
    let event_id = text(&b, "event_id", 64)?;
    let now = st.clock.ts();
    let mode = st.cfg.news_mode.clone();

    // merged into another event: keep the row, mark it, nothing else
    if let Some(into) = body_str(&b, "merged_into")? {
        let (eid, into2, now2) = (event_id.clone(), into.clone(), now.clone());
        let row = with_side(&st, move |c| {
            c.execute(
                "UPDATE media_news SET merged_into=?, status='merged', updated_at=? WHERE event_id=?",
                params![into2, now2, eid],
            )
            .map_err(sql)?;
            c.query_row("SELECT id, news_id FROM media_news WHERE event_id=?", [eid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))
                .optional()
                .map_err(sql)
        })?;
        let (id, news_id) = row.ok_or_else(|| E::new(404, "media_news_not_found", "没有这条新闻"))?;
        return Ok(axum::Json(MediaNewsResponse { id, status: "merged".into(), news_id, mode }));
    }

    let source = text(&b, "source", 40)?;
    let url = text(&b, "url", 500)?;
    let title = text(&b, "title", 60)?;
    let original_title = text(&b, "original_title", 300)?;
    let published_at = rfc3339(&b, "published_at", false)?;
    let fetched_at = rfc3339(&b, "fetched_at", true)?.unwrap_or_default();
    let stock_id = need(body_int(&b, "stock_id")?, "stock_id")?;
    let event = one_of(&b, "event", &EVENTS)?;
    let direction = one_of(&b, "direction", &DIRECTIONS)?;
    let credibility = one_of(&b, "credibility", &CREDIBILITY)?;
    let magnitude = match b.get("magnitude") {
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(-1.0),
        None => return Err(error::invalid(vec![error::missing("magnitude")])),
        _ => return Err(error::invalid(vec![error::wrong_type("magnitude")])),
    };
    if !(0.0..=10.0).contains(&magnitude) {
        return Err(error::invalid(vec![error::field("magnitude", "out_of_range", "强度在 0 到 10 之间")]));
    }
    let model = text(&b, "model", 40)?;
    let probabilities = match b.get("probabilities") {
        Some(v @ serde_json::Value::Object(_)) => v.to_string(),
        None => return Err(error::invalid(vec![error::missing("probabilities")])),
        _ => return Err(error::invalid(vec![error::wrong_type("probabilities")])),
    };

    // the stock must exist and be listed
    let title_of = engine(&st, move |ctx| -> R<String> {
        let co = ctx.engine.company(stock_id).ok_or_else(|| error::stock_not_found(stock_id))?;
        if co.status != "listed" {
            return Err(E::new(422, "stock_not_tradable", "这支股票不在交易").details(serde_json::json!({"status": co.status})));
        }
        Ok(co.title.clone())
    })
    .await??;

    // archive: a new event, or an update of the same one (the judgement may be refined)
    let (eid, now2) = (event_id.clone(), now.clone());
    let (src2, url2, t2, ot2, pub2, f2, ev2, dir2, cr2, m2, p2) = (
        source.clone(),
        url.clone(),
        title.clone(),
        original_title.clone(),
        published_at.clone(),
        fetched_at.clone(),
        event.clone(),
        direction.clone(),
        credibility.clone(),
        model.clone(),
        probabilities.clone(),
    );
    let (id, news_id, status) = with_side(&st, move |c| {
        c.execute(
            "INSERT INTO media_news(event_id, company_id, source, url, title, original_title, published_at, fetched_at,
                                    received_at, updated_at, event, direction, magnitude, credibility, model, probabilities, status)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,'stored')
             ON CONFLICT(event_id) DO UPDATE SET company_id=excluded.company_id, source=excluded.source, url=excluded.url,
               title=excluded.title, original_title=excluded.original_title, published_at=excluded.published_at,
               fetched_at=excluded.fetched_at, updated_at=excluded.updated_at, event=excluded.event,
               direction=excluded.direction, magnitude=excluded.magnitude, credibility=excluded.credibility,
               model=excluded.model, probabilities=excluded.probabilities",
            params![eid, stock_id, src2, url2, t2, ot2, pub2, f2, now2, now2, ev2, dir2, magnitude, cr2, m2, p2],
        )
        .map_err(sql)?;
        c.query_row("SELECT id, news_id, status FROM media_news WHERE event_id=?", [eid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(sql)
    })?;
    let (id, mut news_id, mut status): (i64, Option<i64>, String) = (id, news_id, status);

    // publish once; a published item is never edited (players may have traded on it)
    if (mode == "publish" || mode == "market") && news_id.is_none() && status != "merged" {
        let body_text = news_body(&source, &credibility, None, &original_title, &url);
        let (d, t) = (st.clock.today(), now.clone());
        let headline = headline(&title, &title_of);
        let nid = engine(&st, move |ctx| -> R<i64> {
            ctx.engine.post_news(&t, &d, "media", Some(stock_id), headline, body_text);
            let id = ctx.engine.st.last_news_id;
            ctx.engine.flush().map_err(error::internal)?;
            Ok(id)
        })
        .await??;
        let now3 = now.clone();
        with_side(&st, move |c| {
            c.execute("UPDATE media_news SET news_id=?, status='published', updated_at=? WHERE id=?", params![nid, now3, id]).map_err(sql)
        })?;
        news_id = Some(nid);
        status = "published".into();
    }
    Ok(axum::Json(MediaNewsResponse { id, status, news_id, mode }))
}
