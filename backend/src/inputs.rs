//! The model's per-day inputs file (written by `engine/daily_inputs.py`; format in its docstring and
//! docs/v2-architecture.md §3), and running the Python side to produce it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

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
}

#[derive(Debug, Clone, Deserialize)]
struct RawInputs {
    day: String,
    #[serde(default)]
    window_start: Option<String>,
    cn_titles: BTreeMap<String, String>,
    shows: Vec<ShowInputs>,
}

/// Parsed inputs for one day. `shows` keeps the file's (model) order.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub day: String,
    pub window_start: Option<String>,
    pub shows: Vec<ShowInputs>,
    pub index: HashMap<i64, usize>,
    pub cn_titles: BTreeMap<i64, String>,
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
        Ok(Inputs { day: raw.day, window_start: raw.window_start, shows, index, cn_titles })
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

/// Run `uv run engine/daily_inputs.py --day D --window-start W --out PATH` in the repository root and
/// read the file it writes. (v1's close builds the same inputs in-process from the game's `window_start`.)
pub fn fetch(cfg: &Config, day: &str, window_start: Option<&str>) -> Result<Inputs, String> {
    let out = default_path(cfg, day);
    let mut cmd = std::process::Command::new("uv");
    cmd.current_dir(&cfg.root).args(["run", "engine/daily_inputs.py", "--day", day, "--out"]).arg(&out);
    if let Some(w) = window_start {
        cmd.args(["--window-start", w]);
    }
    let r = cmd.output().map_err(|e| format!("daily_inputs.py: {e}"))?;
    if !r.status.success() {
        let err = String::from_utf8_lossy(&r.stderr);
        let tail: String = err.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(format!("daily_inputs.py --day {day} failed ({}): {tail}", r.status));
    }
    let inp = Inputs::read(&out)?;
    if inp.day != day {
        return Err(format!("inputs are for {}, not {day}", inp.day));
    }
    Ok(inp)
}

/// Rebuild the derived market series before a close (v1 `app.refresh_inputs`): Annict air times, then the
/// market build. Failures are logged and ignored, like v1.
pub fn refresh_market_data(cfg: &Config) {
    for args in [["sim/reconstruct.py", "airtimes"], ["sim/reconstruct.py", "build"]] {
        let r = std::process::Command::new("uv").current_dir(&cfg.root).arg("run").args(args).output();
        match r {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let tail: String = err.chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect();
                eprintln!("{} failed: {tail}", args.join(" "));
            }
            Err(e) => eprintln!("{} failed: {e}", args.join(" ")),
        }
    }
}
