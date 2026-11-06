//! A stock's works (docs/v2-series.md §3): each work's broadcast schedule from the public sources, the
//! per-episode discussion on Bangumi, and the series' sequel status (docs/api-v1.md §4.2.10).
//!
//! Only dates a public source gives are shown: the episodes aired (the game's own record), Bangumi's dated
//! episodes and AniList's next episode. The valuation's weekly fill-in for undated episodes and a halted show's
//! assumed resume day stay internal (`date: null`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use chrono::NaiveDate;
use serde_json::Value;

use super::error::{self, sql, ErrorBody, R};
use super::market::{collector, snapshot};
use super::stocks::{id_str, matches};
use super::types::*;
use super::{engine, read};
use crate::api::AppState;
use crate::engine::Engine;
use crate::vclose::day_no;

/// Aired episodes listed for a long-runner (its most recent).
const LONG_AIRED: usize = 8;
/// Episodes with a discussion count, most recent.
const DISCUSSION_MAX: usize = 26;

/// What the engine knows about one work.
struct RawWork {
    id: i64,
    title: Option<String>,
    first_air: Option<i64>,
    episodes: Option<i64>,
    long: bool,
    /// (episode, air day), aired so far
    aired: Vec<(i64, i64)>,
    /// (episode, planned day, Bangumi's day for it, the day is from a public source)
    upcoming: Vec<(i64, i64, Option<i64>, bool)>,
    halted: bool,
}

struct Raw {
    works: Vec<RawWork>,
    /// an announced sequel not (yet) a work of the company: (AniList id, announcement day)
    sequel: Option<(i64, i64)>,
    /// the collector has the relations (else "no sequel" is not information)
    watched: bool,
}

fn fmt_day(d: i64) -> String {
    NaiveDate::from_num_days_from_ce_opt(d as i32).map(|x| x.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

fn raw(e: &Engine, id: i64, today: i64) -> R<Raw> {
    let c = e.company(id).ok_or_else(|| error::stock_not_found(id))?.clone();
    let mut out = Raw { works: Vec::new(), sequel: None, watched: false };
    let works = e.works_of(id);
    for w in &works {
        let js = e.valuation_state(*w);
        let meta = js.and_then(|j| j.meta.clone());
        if let Some(j) = js {
            out.watched |= j.watched;
            if let Some(q) = j.sequel.as_ref().filter(|q| !works.contains(&q.0)) {
                out.sequel.get_or_insert((q.0, q.1));
            }
        }
        let (title, first_air, episodes, long, air) = if *w == id {
            (Some(c.title.clone()), day_no(&c.first_air), c.episodes, c.long_runner != 0, e.air_days_of(id))
        } else {
            match &meta {
                Some(m) => (None, Some(m.first_air), m.episodes, m.long, m.air.clone()),
                None => continue,
            }
        };
        let mut aired: Vec<(i64, i64)> = air.iter().enumerate().map(|(i, d)| (i as i64 + 1, *d)).collect();
        if long {
            // a long-runner's back catalogue enters on its first day all at once: keep the recent episodes
            let first = aired.first().map(|a| a.1);
            let backfill = aired.iter().filter(|a| Some(a.1) == first).count() > 2;
            if backfill {
                aired.retain(|a| Some(a.1) != first || a.0 == air.len() as i64);
            }
            let skip = aired.len().saturating_sub(LONG_AIRED);
            aired.drain(..skip);
        }
        let halted = meta.as_ref().is_some_and(|m| m.hiatus.is_some());
        let mut upcoming = Vec::new();
        if let Some(m) = &meta {
            let n_aired = air.len() as i64;
            if long {
                if let Some((ep, d)) = m.next_airing.filter(|(ep, d)| *ep > n_aired && *d >= today) {
                    upcoming.push((ep, d, None, true));
                }
            } else {
                let n = Engine::planned_episodes(episodes, &air, today, &m.schedule_hold());
                let mut base = air.clone();
                base.sort_unstable();
                if base.is_empty() {
                    if let Some(f) = first_air {
                        upcoming.push((1, f, m.schedule.first().copied(), true));
                        base.push(f);
                    }
                }
                let start = base.len() as i64;
                for (k, d) in m.upcoming(&base, n).into_iter().enumerate() {
                    let ep = start + 1 + k as i64;
                    let bgm = m.schedule.get(ep as usize - 1).copied();
                    let next = m.next_airing.is_some_and(|(e, _)| e == ep);
                    // a public date: Bangumi has one for the episode (moved with AniList's next episode when they
                    // differ), and the show is not halted (its resume day is the valuation's assumption)
                    let public = !halted && (bgm.is_some() || next);
                    upcoming.push((ep, d, bgm, public));
                }
            }
        }
        out.works.push(RawWork { id: *w, title, first_air, episodes, long, aired, upcoming, halted });
    }
    Ok(out)
}

/// A work's Chinese title: Bangumi's `name_cn`, else AniList's native title.
fn work_title(cdb: Option<&rusqlite::Connection>, ids: &std::collections::HashMap<String, Value>, w: i64) -> Option<String> {
    let cdb = cdb?;
    let bgm = ids.get(&w.to_string()).and_then(|e| e.get("bgm")).and_then(id_str);
    if let Some(s) = bgm.and_then(|b| snapshot(cdb, "bgm", "subject", &b)) {
        let cn = s.get("name_cn").and_then(Value::as_str).unwrap_or("");
        let name = if cn.is_empty() { s.get("name").and_then(Value::as_str).unwrap_or("") } else { cn };
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    let m = snapshot(cdb, "anilist", "media", &w.to_string())?;
    let t = m.get("title")?;
    ["native", "romaji"].iter().find_map(|k| t.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
}

/// Bangumi's per-episode comment counts of a work (main episodes), by episode number.
fn discussion(cdb: Option<&rusqlite::Connection>, ids: &std::collections::HashMap<String, Value>, w: i64, aired: i64, long: bool, today: &str) -> Vec<EpisodeComments> {
    let Some(cdb) = cdb else { return Vec::new() };
    let Some(bgm) = ids.get(&w.to_string()).and_then(|e| e.get("bgm")).and_then(id_str) else { return Vec::new() };
    let Some(s) = snapshot(cdb, "bgm", "episodes", &bgm) else { return Vec::new() };
    let mut out: Vec<EpisodeComments> = s
        .get("data")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|x| x.get("type").and_then(Value::as_i64).unwrap_or(0) == 0)
                .filter_map(|x| {
                    // a season counts from 1 (`ep`); a long-runner's arc subjects restart `ep`, `sort` is the overall
                    // number the game uses
                    let (a, b) = if long { ("sort", "ep") } else { ("ep", "sort") };
                    let ep = x.get(a).and_then(Value::as_f64).or_else(|| x.get(b).and_then(Value::as_f64))?;
                    let air = x.get("airdate").and_then(Value::as_str).unwrap_or("");
                    let ep = ep as i64;
                    // aired: by count for a season, by date for a long-runner (numbering differs between sources)
                    let shown = if long { !air.is_empty() && air <= today } else { ep >= 1 && ep <= aired };
                    shown.then(|| EpisodeComments { episode: ep, comments: x.get("comment").and_then(Value::as_i64).unwrap_or(0) })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|x| x.episode);
    out.dedup_by_key(|x| x.episode);
    let skip = out.len().saturating_sub(DISCUSSION_MAX);
    out.drain(..skip);
    out
}

/// An announced sequel's premiere as AniList gives it: a day, a month or a year (as precise as announced).
fn premiere_of(m: &Value) -> Option<String> {
    let d = m.get("startDate")?;
    let n = |k: &str| d.get(k).and_then(Value::as_i64);
    match (n("year"), n("month"), n("day")) {
        (Some(y), Some(mo), Some(dd)) => Some(format!("{y:04}-{mo:02}-{dd:02}")),
        (Some(y), Some(mo), None) => Some(format!("{y:04}-{mo:02}")),
        (Some(y), None, _) => Some(format!("{y:04}")),
        _ => None,
    }
}

/// 一支股票名下的作品：播出排期、单集讨论、续作
#[utoipa::path(get, path = "/api/v1/stocks/{id}/works", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = WorksResponse), (status = "default", body = ErrorBody)))]
pub async fn works(State(st): State<Arc<AppState>>, Path(raw_id): Path<String>) -> R<Json<WorksResponse>> {
    let id = super::path_id(&raw_id, "id")?;
    let today_s = st.clock.today();
    let today = day_no(&today_s).ok_or_else(|| error::internal("today"))?;
    let r = engine(&st, move |ctx| raw(&ctx.engine, id, today)).await??;
    let st2 = st.clone();
    let out = read(&st, move |c| {
        let cfg = &st2.cfg;
        let cdb = collector(cfg);
        let ids = matches(cfg);
        // the delay notices name the episode (corporate.rs `schedule_news`): 《title》第 N 集延期至 …
        let mut notices: Vec<String> = Vec::new();
        {
            let mut s = c.prepare_cached("SELECT title FROM news WHERE company_id=? AND kind='delay'").map_err(sql)?;
            let rows = s.query_map([id], |r| r.get::<_, String>(0)).map_err(sql)?;
            for t in rows {
                notices.push(t.map_err(sql)?);
            }
        }
        let mut works = Vec::new();
        let mut dated: Option<(String, i64)> = None;
        for w in r.works {
            let title = w.title.clone().or_else(|| work_title(cdb.as_ref(), &ids, w.id)).unwrap_or_default();
            let noticed = |ep: i64| notices.iter().any(|t| t.starts_with(&format!("《{title}》")) && t.contains(&format!("第 {ep} 集延期")));
            let n_aired = w.aired.last().map_or(0, |a| a.0);
            let mut schedule: Vec<ScheduledEpisode> = w
                .aired
                .iter()
                .map(|(ep, d)| ScheduledEpisode { episode: *ep, date: Some(day(fmt_day(*d))), aired: true, delayed: noticed(*ep) })
                .collect();
            for (ep, d, bgm, public) in &w.upcoming {
                // later than Bangumi's date (beyond the late-night day of tolerance), or a delay notice for it
                let late = bgm.is_some_and(|b| *d - b > 1);
                schedule.push(ScheduledEpisode {
                    episode: *ep,
                    date: public.then(|| day(fmt_day(*d))),
                    aired: false,
                    delayed: (*public && late) || noticed(*ep),
                });
            }
            let not_started = w.aired.is_empty() && w.first_air.is_some_and(|f| f > today);
            let finished = !w.long && w.upcoming.is_empty() && !w.aired.is_empty();
            let status = if w.halted {
                "halted"
            } else if not_started {
                "upcoming"
            } else if finished {
                // (a long-runner is never finished: a break between arcs is not an ending)
                "finished"
            } else {
                "airing"
            };
            if w.id != id && not_started {
                if let Some(f) = w.first_air {
                    if dated.as_ref().is_none_or(|x| f < x.1) {
                        dated = Some((title.clone(), f));
                    }
                }
            }
            let total_aired = if w.long { n_aired } else { w.aired.len() as i64 };
            works.push(Work {
                id: w.id,
                title,
                first_air: w.first_air.map(|f| day(fmt_day(f))),
                episodes: if w.long { None } else { w.episodes },
                episodes_aired: total_aired,
                is_long_runner: w.long,
                status: status.into(),
                schedule,
                discussion: discussion(cdb.as_ref(), &ids, w.id, total_aired, w.long, &today_s),
            });
        }
        let sequel = if let Some((title, f)) = dated {
            Sequel { status: "dated".into(), title: Some(title), premiere: Some(fmt_day(f)), announced: None }
        } else if let Some((sid, announced)) = r.sequel {
            let m = cdb.as_ref().and_then(|cdb| snapshot(cdb, "anilist", "media", &sid.to_string()));
            let premiere = m.as_ref().and_then(premiere_of);
            let exact = premiere.as_ref().is_some_and(|p| p.len() == 10);
            Sequel {
                status: if exact { "dated" } else { "announced" }.into(),
                title: work_title(cdb.as_ref(), &ids, sid),
                premiere,
                announced: Some(day(fmt_day(announced))),
            }
        } else {
            Sequel { status: if r.watched { "none" } else { "unknown" }.into(), title: None, premiere: None, announced: None }
        };
        Ok(WorksResponse { works, sequel })
    })
    .await?;
    Ok(Json(out))
}
