//! Season data for the simulation from the repository's existing data: episode air times (Annict, via
//! sim/matches.json) and 热度快讯 synthesized from Bangumi episode comment counts in the collector DB.
//!
//! The live 热度快讯 compares an episode's comment count a few hours after it airs with earlier episodes
//! at the same age; the collector only has each episode's final count, so the simulation uses the final
//! counts as the proxy (same rule: this episode vs the average of the previous three).

use std::collections::HashMap;
use std::path::Path;

use crate::config::Config;
use crate::engine::Engine;
use crate::market::at_local;

pub struct SeasonData {
    pub air: HashMap<i64, Vec<i64>>,
    pub flashes: Vec<FlashObs>,
}

/// A 热度快讯 observation (what engine/flash_fetch.py would have seen).
#[derive(Debug, Clone)]
pub struct FlashObs {
    pub company_id: i64,
    pub at: i64,
    pub episode: f64,
    pub comments: i64,
    pub prev: Vec<i64>,
    pub hours: f64,
}

fn parse_utc(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

impl SeasonData {
    pub fn load(
        cfg: &Config,
        mc: &crate::market::MarketConfig,
        collector: Option<&Path>,
        e: &Engine,
        days: &[String],
    ) -> SeasonData {
        let mut out = SeasonData { air: HashMap::new(), flashes: Vec::new() };
        let fp = crate::vfilter::FilterParams::load(cfg).ok();
        let read = |p: std::path::PathBuf| -> Option<serde_json::Value> {
            serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
        };
        let Some(matches) = read(cfg.root.join("sim").join("matches.json")) else { return out };
        let airtimes = read(cfg.root.join("data").join("derived").join("annict_airtimes.json")).unwrap_or_default();
        let (first, last) = (at_local(&days[0], 0, 0), at_local(days.last().unwrap(), 24, 0));
        // anilist id -> (annict id, bgm id)
        let mut ids: HashMap<i64, (Option<i64>, Option<i64>)> = HashMap::new();
        if let Some(m) = matches.get("matches").and_then(|x| x.as_object()) {
            for (k, v) in m {
                let Ok(id) = k.parse::<i64>() else { continue };
                ids.insert(id, (v.get("annict").and_then(|x| x.as_i64()), v.get("bgm").and_then(|x| x.as_i64())));
            }
        }
        if let Some(m) = matches.get("manual").and_then(|x| x.as_object()) {
            for (k, v) in m {
                let Ok(id) = k.parse::<i64>() else { continue };
                if let Some(b) = v.get("bgm").and_then(|x| x.as_i64()) {
                    ids.entry(id).or_insert((None, None)).1 = Some(b);
                }
            }
        }
        let mut annict_times: HashMap<i64, Vec<i64>> = HashMap::new();
        for (&id, &(annict, _)) in &ids {
            let Some(a) = annict else { continue };
            if let Some(eps) = airtimes.get(a.to_string()).and_then(|x| x.as_object()) {
                let mut ts: Vec<i64> = eps.values().filter_map(|v| v.as_str().and_then(parse_utc)).collect();
                ts.sort();
                annict_times.insert(id, ts);
            }
        }
        // Bangumi episodes (latest snapshot per subject)
        let mut bgm_eps: HashMap<i64, Vec<(i64, String, i64)>> = HashMap::new();
        if let Some(path) = collector {
            if let Ok(db) = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
                if let Ok(mut st) = db.prepare("SELECT CAST(external_id AS TEXT), data FROM snapshots WHERE source='bgm' AND kind='episodes' ORDER BY day") {
                    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
                    if let Ok(rows) = rows {
                        for (sid, data) in rows.flatten() {
                            let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else { continue };
                            let Some(list) = v.get("data").and_then(|x| x.as_array()) else { continue };
                            let mut eps: Vec<(i64, String, i64)> = list
                                .iter()
                                .filter(|x| x.get("type").and_then(|t| t.as_i64()).unwrap_or(0) == 0)
                                .filter_map(|x| {
                                    Some((
                                        x.get("sort").and_then(|s| s.as_f64())? as i64,
                                        x.get("airdate").and_then(|s| s.as_str())?.to_string(),
                                        x.get("comment").and_then(|s| s.as_i64()).unwrap_or(0),
                                    ))
                                })
                                .collect();
                            eps.sort();
                            if let Ok(sid) = sid.parse::<i64>() {
                                bgm_eps.insert(sid, eps);
                            }
                        }
                    }
                }
            }
        }
        for (&id, &(_, bgm)) in &ids {
            let annict = annict_times.get(&id);
            let mut times: Vec<i64> = annict.cloned().unwrap_or_default();
            let eps = bgm.and_then(|b| bgm_eps.get(&b));
            if times.is_empty() {
                if let Some(eps) = eps {
                    times = eps.iter().filter(|e| e.1.len() == 10).map(|e| at_local(&e.1, 23, 0)).collect();
                }
            }
            times.retain(|&t| t >= first - 86400 && t <= last);
            if !times.is_empty() {
                out.air.insert(id, times);
            }
            let Some(eps) = eps else { continue };
            let title = e.company(id).map(|c| c.title.clone()).unwrap_or_else(|| id.to_string());
            for (k, ep) in eps.iter().enumerate() {
                if ep.1.len() != 10 || k == 0 {
                    continue;
                }
                let air = annict.and_then(|a| a.get((ep.0 - 1).max(0) as usize).copied()).filter(|t| {
                    // the Annict time must be the same calendar day (±1) as Bangumi's air date
                    (t - at_local(&ep.1, 12, 0)).abs() < 2 * 86400
                });
                let air = air.unwrap_or_else(|| at_local(&ep.1, 23, 0));
                if air < first || air > last {
                    continue;
                }
                let prev: Vec<i64> = eps[k.saturating_sub(3)..k].iter().map(|x| x.2).collect();
                let mean = prev.iter().sum::<i64>() as f64 / prev.len() as f64;
                if mean < 5.0 {
                    continue;
                }
                // the hourly fetch a few hours after airing; the count then is the final count scaled by the
                // hourly curve (the collector has final counts only; the live fetcher sees the real count)
                let hours = mc.flash_delay_h;
                let frac = fp.as_ref().and_then(|fp| fp.flash_curve.get((hours as usize).saturating_sub(1)).copied()).unwrap_or(0.2);
                out.flashes.push(FlashObs {
                    company_id: id,
                    at: air + (hours * 3600.0) as i64,
                    episode: ep.0 as f64,
                    comments: (ep.2 as f64 * frac).round() as i64,
                    prev: prev.clone(),
                    hours,
                });
                let _ = &title;
            }
        }
        out.flashes.sort_by_key(|f| (f.at, f.company_id));
        if std::env::var("SIM_DEBUG").is_ok() {
            eprintln!(
                "season data: {} matched ids, {} with Annict times, {} Bangumi subjects, {} air schedules, {} flashes",
                ids.len(),
                annict_times.len(),
                bgm_eps.len(),
                out.air.len(),
                out.flashes.len()
            );
        }
        out
    }
}
