//! The model's per-day inputs file (written by `engine/daily_inputs.py`; format in its docstring and
//! docs/v2-architecture.md §3), and running the Python side to produce it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::config::Config;
use crate::valuation::M3;

#[derive(Debug, Clone, Deserialize)]
pub struct Markets {
    pub us: f64,
    pub cn: f64,
    pub jp: f64,
}

impl Markets {
    pub fn m3(&self) -> M3 {
        [self.us, self.cn, self.jp]
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowInputs {
    pub id: i64,
    pub title_ja: String,
    pub title_en: Option<String>,
    pub studio: Option<String>,
    /// YYYY-MM-DD
    pub first_air: String,
    pub episodes: Option<i64>,
    pub episodes_known: bool,
    pub long_runner: bool,
    pub popularity_end: i64,
    pub aired: i64,
    pub has_real_data: bool,
    pub att: Markets,
    pub expected: Markets,
    /// the audience-based pre-air estimate per market (the valuation filter's prior; since v0.3.0)
    #[serde(default)]
    pub preair: Option<Markets>,
    /// the day each episode aired so far, one entry per episode (since v0.3.0)
    #[serde(default)]
    pub air_dates: Vec<String>,
    /// an adult title (AniList isAdult or the Hentai genre): never listed, bought out if it is (since v0.3.1)
    #[serde(default)]
    pub adult: bool,
    /// a PV / CM / teaser entry, not a show: treated like `adult` (since v0.3.2)
    #[serde(default)]
    pub promo: bool,
    /// point-in-time pre-air estimate, frozen at the premiere (since v0.3.2)
    #[serde(default)]
    pub prior: Option<Markets>,
    /// which markets reported on the day (since v0.3.2)
    #[serde(default)]
    pub observed: Option<Flags>,
    /// which markets' data sources the show is mapped to (since v0.3.2)
    #[serde(default)]
    pub sources: Option<Flags>,
    /// AniList averageScore on the day (None: not enough votes yet); the long tail's prior (phase 1)
    #[serde(default)]
    pub score: Option<f64>,
    /// AniList format (TV, ONA, MOVIE, ...) (phase 1)
    #[serde(default)]
    pub format: Option<String>,
    /// AniList source (MANGA, LIGHT_NOVEL, ORIGINAL, ...) (phase 1: the sequel option)
    #[serde(default)]
    pub source: Option<String>,
    /// itself a sequel: a PREQUEL relation to a TV/ONA/film (phase 1)
    #[serde(default)]
    pub prequel: bool,
    /// in the top decile of popularity among the shows premiering within half a year of it (phase 1)
    #[serde(default)]
    pub top_pop: bool,
    /// an announced sequel not yet aired (phase 1, docs/v2-series.md §5.2)
    #[serde(default)]
    pub sequel: Option<SequelInputs>,
    /// the series key (engine/series.py: the smallest AniList id of the series) (phase 1, docs/v2-series.md §3)
    #[serde(default)]
    pub series: Option<i64>,
    /// AniList list entries on the day (the tail prior's popularity at the finale) (phase 1)
    #[serde(default)]
    pub popularity: Option<f64>,
    /// the collector has the show's relations as of the day (else "no sequel announced" is not information)
    #[serde(default)]
    pub relations_known: bool,
    /// the public schedule: every main episode's air date on Bangumi (past and announced) (phase 1)
    #[serde(default)]
    pub schedule: Vec<String>,
    /// AniList status HIATUS on the day: production halted (docs/v2-corporate.md §2.3)
    #[serde(default)]
    pub hiatus: bool,
    /// AniList's next episode on the day: its number and air day (docs/v2-corporate.md §2.2)
    #[serde(default)]
    pub next_airing: Option<NextAiring>,
}

/// AniList `nextAiringEpisode`: the episode number and its air day (local date of `airingAt`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextAiring {
    pub episode: i64,
    pub date: String,
}

/// An announced sequel: its AniList id, the day the relation first appeared, and its premiere day(s): the
/// announced date, the middle of the announced month or year, or the announcement plus each of the fitted
/// announcement-to-premiere lags (equally likely).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequelInputs {
    pub id: i64,
    pub announced: String,
    pub premieres: Vec<String>,
    #[serde(default)]
    pub exact: bool,
}

/// One flag per market.
#[derive(Debug, Clone, Deserialize)]
pub struct Flags {
    #[serde(default = "yes")]
    pub us: bool,
    pub cn: bool,
    pub jp: bool,
}

fn yes() -> bool {
    true
}

/// One show's AniEx heat index for the day (engine/heat_index.py; docs/v3-futures.md §3): the checked heat
/// per market and the index futures settle on. Published once and never revised.
#[derive(Debug, Clone, Deserialize)]
pub struct HeatIndex {
    pub idx: f64,
    #[serde(default)]
    pub us: Option<f64>,
    #[serde(default)]
    pub cn: Option<f64>,
    #[serde(default)]
    pub jp: Option<f64>,
    /// market -> missing | filled | negative | spike (the value was filled or replaced)
    #[serde(default)]
    pub flags: BTreeMap<String, String>,
    /// the method's version
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RawInputs {
    day: String,
    #[serde(default)]
    window_start: Option<String>,
    cn_titles: BTreeMap<String, String>,
    shows: Vec<ShowInputs>,
    /// phase 1: each market's platform size (docs/v2-series.md §6) and the number of days it averages
    #[serde(default)]
    platform: Option<Markets>,
    #[serde(default)]
    platform_days: Option<i64>,
    /// the heat index of the day per show (top level, so binaries from before it ignore it)
    #[serde(default)]
    heat: BTreeMap<String, HeatIndex>,
}

/// Parsed inputs for one day. `shows` keeps the file's (model) order.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub day: String,
    pub window_start: Option<String>,
    pub shows: Vec<ShowInputs>,
    pub index: HashMap<i64, usize>,
    pub cn_titles: BTreeMap<i64, String>,
    /// each market's platform size: the trailing-year mean of the day's total attention of every tracked work
    /// airing (listed or not); None in inputs written before phase 1
    pub platform: Option<Markets>,
    /// the number of days `platform` averages (up to 365; fewer while the collector's history is short)
    pub platform_days: i64,
    /// the day's published heat index per show (empty before it is computed)
    pub heat: BTreeMap<i64, HeatIndex>,
}

impl ShowInputs {
    /// Why the show is not traded here, if it isn't: the news copy for its buyout or cancelled offer.
    pub fn excluded(&self) -> Option<&'static str> {
        if self.adult {
            Some("成人向作品不在安易交易")
        } else if self.promo {
            Some("宣传片不在安易交易")
        } else {
            None
        }
    }
}

impl Inputs {
    pub fn parse(text: &str) -> Result<Inputs, String> {
        let raw: RawInputs = serde_json::from_str(text).map_err(|e| format!("inputs: {e}"))?;
        let mut shows: Vec<ShowInputs> = Vec::new();
        let mut index = HashMap::new();
        for s in raw.shows {
            // a Python dict keeps the first position of a repeated key and the last value
            match index.get(&s.id) {
                Some(&i) => shows[i] = s,
                None => {
                    index.insert(s.id, shows.len());
                    shows.push(s);
                }
            }
        }
        let mut cn_titles = BTreeMap::new();
        for (k, v) in raw.cn_titles {
            cn_titles.insert(k.parse::<i64>().map_err(|e| format!("cn_titles key {k}: {e}"))?, v);
        }
        let mut heat = BTreeMap::new();
        for (k, v) in raw.heat {
            heat.insert(k.parse::<i64>().map_err(|e| format!("heat key {k}: {e}"))?, v);
        }
        Ok(Inputs {
            day: raw.day,
            window_start: raw.window_start,
            shows,
            index,
            cn_titles,
            platform: raw.platform,
            platform_days: raw.platform_days.unwrap_or(0),
            heat,
        })
    }

    pub fn read(path: &Path) -> Result<Inputs, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Inputs::parse(&text)
    }

    pub fn show(&self, id: i64) -> Option<&ShowInputs> {
        self.index.get(&id).map(|i| &self.shows[*i])
    }
}

/// Where `engine/daily_inputs.py` writes day `day` by default (`$ANISTOCK_INPUTS_DIR` or data/derived/inputs).
pub fn default_path(cfg: &Config, day: &str) -> PathBuf {
    let dir = std::env::var("ANISTOCK_INPUTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| cfg.root.join("data").join("derived").join("inputs"));
    dir.join(format!("{day}.json"))
}

/// `<root>/.venv/bin/python SCRIPT` (like v1, which runs its own interpreter; no dependency resolution
/// or network at runtime), or `uv run SCRIPT` when the project venv is missing.
pub fn python(cfg: &Config, script: &str) -> std::process::Command {
    let venv = cfg.root.join(".venv").join("bin").join("python");
    let mut cmd = if venv.exists() {
        let mut c = std::process::Command::new(venv);
        c.arg(script);
        c
    } else {
        let mut c = std::process::Command::new("uv");
        c.args(["run", script]);
        c
    };
    cmd.current_dir(&cfg.root);
    cmd
}

/// Run a command to completion, killing it after `timeout` (v1 uses `subprocess.run(timeout=...)`).
fn run_with_timeout(mut cmd: std::process::Command, timeout: Duration) -> Result<std::process::Output, String> {
    use std::process::Stdio;
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("spawn: {e}"))?;
    // drain the pipes on threads so a chatty child cannot block on a full pipe
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut v);
        v
    });
    let t_err = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut err, &mut v);
        v
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {} s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ok(std::process::Output {
        status,
        stdout: t_out.join().unwrap_or_default(),
        stderr: t_err.join().unwrap_or_default(),
    })
}

fn tail(bytes: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = s.chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

/// Run `engine/daily_inputs.py --day D --window-start W --out PATH` in the repository root (with the
/// project venv's python, see `python`) and read the file it writes. v1's close builds the same inputs
/// in-process from the game's `window_start`.
pub fn fetch(cfg: &Config, day: &str, window_start: Option<&str>) -> Result<Inputs, String> {
    let out = default_path(cfg, day);
    let mut cmd = python(cfg, "engine/daily_inputs.py");
    cmd.args(["--day", day, "--out"]).arg(&out);
    if let Some(w) = window_start {
        cmd.args(["--window-start", w]);
    }
    let r = run_with_timeout(cmd, Duration::from_secs(900)).map_err(|e| format!("daily_inputs.py: {e}"))?;
    if !r.status.success() {
        return Err(format!("daily_inputs.py --day {day} failed ({}): {}", r.status, tail(&r.stderr, 800)));
    }
    let inp = Inputs::read(&out)?;
    if inp.day != day {
        return Err(format!("inputs are for {}, not {day}", inp.day));
    }
    Ok(inp)
}

/// Rebuild the derived market series before a close (v1 `app.refresh_inputs`): Annict air times, then the
/// market build, 900 s each at most. Failures are logged and ignored, like v1.
pub fn refresh_market_data(cfg: &Config) {
    for (script, arg) in [("sim/reconstruct.py", "airtimes"), ("sim/reconstruct.py", "build")] {
        let mut cmd = python(cfg, script);
        cmd.arg(arg);
        match run_with_timeout(cmd, Duration::from_secs(900)) {
            Ok(o) if o.status.success() => {}
            Ok(o) => eprintln!("{script} {arg} failed: {}", tail(&o.stderr, 500)),
            Err(e) => eprintln!("{script} {arg} failed: {e}"),
        }
    }
}

/// Run `engine/flash_fetch.py` once (热度快讯: today's episodes' Bangumi comment counts → JSON files in the
/// events inbox, which the market ingests). Failures are logged and ignored.
pub fn fetch_flashes(cfg: &Config) {
    let inbox = crate::market::runtime::MarketRuntime::events_dir(cfg).join("inbox");
    let mut cmd = python(cfg, "engine/flash_fetch.py");
    cmd.arg("--inbox").arg(&inbox).arg("--game-db").arg(&cfg.game_db);
    match run_with_timeout(cmd, Duration::from_secs(600)) {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!("flash_fetch.py failed: {}", tail(&o.stderr, 500)),
        Err(e) => eprintln!("flash_fetch.py failed: {e}"),
    }
}

#[cfg(test)]
mod heat_tests {
    use super::*;

    #[test]
    fn heat_is_optional_and_keyed_by_show() {
        let old = Inputs::parse(r#"{"day":"2026-11-05","cn_titles":{},"shows":[]}"#).unwrap();
        assert!(old.heat.is_empty());
        let new = Inputs::parse(
            r#"{"day":"2026-11-05","cn_titles":{},"shows":[],
                "heat":{"103303":{"idx":47.2,"us":260.6,"cn":8.8,"jp":null,"flags":{"jp":"missing"},"version":"shadow-2"}}}"#,
        )
        .unwrap();
        let h = &new.heat[&103303];
        assert_eq!((h.idx, h.jp, h.flags["jp"].as_str()), (47.2, None, "missing"));
    }
}
