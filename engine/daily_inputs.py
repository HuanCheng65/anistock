"""The economic model's output for one game day: the interface between the Python model and the game server.

    uv run engine/daily_inputs.py --day 2026-11-05 [--out PATH] [--window-start 2026-07-01]

Writes data/derived/inputs/<day>.json (or $ANISTOCK_INPUTS_DIR/<day>.json, or --out). The daily close
(server/daily.py now, the Rust server later) reads everything it needs from the model out of this file and
combines it with game state through the pure formulas in engine/valuation.py. The file is deterministic:
the same collector data, derived market series and parameters give the same bytes.

File format (JSON, one object):

    day           "YYYY-MM-DD"  the day being closed (D)
    window_start  "YYYY-MM-DD"  the game's data window start (game state `window_start`)
    cn_titles     {anilist_id: name}  Chinese title for every matched show (not only those in `shows`): a manual
                  `title_cn` in sim/matches.json wins, else Bangumi's name_cn. The close renames any company
                  whose id is here (refresh_titles); a new company is named cn_titles[id], else title_ja.
    shows         [ ... ]  every show of the model's universe that is airing or lists within
                  ipo_subscription_days + 1 days of D, IN MODEL ORDER (new IPOs are created, and their stock
                  codes numbered, in this order). Each entry:
        id              AniList id (= company id)
        title_ja        native title (else romaji, else the id)
        title_en        English title (else romaji); may be null
        studio          main studio; may be null
        first_air       "YYYY-MM-DD"  premiere date
        episodes        episode count used by the valuation; when unknown and not a long-runner, the default
                        count (valuation.default_episodes); null only for long-runners with no count
        episodes_known  false when `episodes` is the default (companies store null then)
        long_runner     premiered more than valuation.long_runner_years before D
        popularity_end  AniList list entries in the latest snapshot (gate: game.min_listing_popularity)
        aired           episodes aired as of D (0 before the premiere)
        has_real_data   D is at least valuation.min_days_after_premiere days after first_air
        air_dates       ["YYYY-MM-DD", ...] the day each episode aired so far (one entry per episode; since v0.3.0)
        adult           true for an adult title (AniList isAdult or the Hentai genre); absent otherwise.
                        Never listed; a listed one is bought out at the next close (since v0.3.1)
        preair          {"us","cn","jp"}  the audience-based pre-air estimate of daily attention per market: the
                        valuation filter's prior (since v0.3.0)
        att             {"us","cn","jp"}  realised attention on D per market (revenue shares, demand index)
        expected        {"us","cn","jp"}  expected attention on D: `att` once the show has real data, else the
                        per-market max of `att` and the audience-based pre-air estimate (expected-share
                        denominator, IPO pricing, fair values)

Not in the file, because they come from game state or config rather than the model: the demand index
(from Σ att.us over *listed* companies and the game's `a_ref`), market weights, rates, prices.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from dataclasses import dataclass, field
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:          # run as a script: engine/ is sys.path[0], the repo root is not
    sys.path.insert(0, str(ROOT))

from engine import economy as E  # noqa: E402


def inputs_dir() -> Path:
    return Path(os.environ.get("ANISTOCK_INPUTS_DIR", ROOT / "data" / "derived" / "inputs"))


def load_markets() -> dict[int, dict]:
    mpath = ROOT / "data" / "derived" / "markets.json"
    if not mpath.exists():
        return {}
    return {int(k): v for k, v in json.loads(mpath.read_text(encoding="utf-8"))["series"].items()}


def air_dates(s: E.Show, day: date) -> list[str]:
    """The day each episode aired so far (from the model's per-day episode count)."""
    out, prev = [], 0
    for d, a in zip(s.days, s.aired):
        if d > day:
            break
        if a > prev:
            out.extend([d.isoformat()] * (a - prev))
        prev = a
    return out


def build(shows: list[E.Show], markets: dict, aud: dict, cn_titles: dict[int, str], day: date, p: dict,
          window_start: date) -> dict:
    """The inputs dict for `day` from already-loaded model objects (no I/O)."""
    rv, val = p["revenue"], p["valuation"]
    out = []
    for s in shows:
        att = E.market_att(s, day, rv, markets)
        out.append({
            "id": s.id,
            "title_ja": s.title,
            "title_en": s.title_en,
            "studio": s.studio,
            "first_air": s.first_air.isoformat(),
            "episodes": s.episodes,
            "episodes_known": s.episodes_known,
            "long_runner": s.long_runner,
            "popularity_end": s.popularity_end,
            "aired": E.at(s, s.aired, day, 0),
            "has_real_data": E.has_real_data(s, day, val),
            "att": {m: att[m] for m in E.MARKETS},
            "preair": {m: v for m, v in E.preair_att(s, day, val, aud).items()},
            "air_dates": air_dates(s, day),
            "expected": {m: v for m, v in E.expected_att(s, day, rv, markets, val, aud).items()},
            **({"adult": True} if s.adult else {}),
        })
    return {
        "day": day.isoformat(),
        "window_start": window_start.isoformat(),
        "cn_titles": {str(k): v for k, v in cn_titles.items()},
        "shows": out,
    }


def window_start_of(cfg) -> date:
    """The game's window start (game state), else the parameters' window start."""
    if cfg.game_db.exists():
        db = sqlite3.connect(f"file:{cfg.game_db}?mode=ro", uri=True)
        try:
            row = db.execute("SELECT value FROM state WHERE key='window_start'").fetchone()
        except sqlite3.Error:
            row = None
        finally:
            db.close()
        if row:
            return date.fromisoformat(row[0])
    return date.fromisoformat(cfg.params["window"]["start"])


def daily_inputs(day: str, cfg=None, window_start: date | None = None) -> dict:
    """Load the model's data and build the inputs for `day` (what the CLI writes)."""
    if cfg is None:
        from server.config import load
        cfg = load()
    d = date.fromisoformat(day)
    start = window_start or window_start_of(cfg)
    shows, _ = E.load_shows(cfg.collector_db, cfg.params, start, d,
                            upcoming_days=cfg.game["ipo_subscription_days"] + 1)
    return build(shows, load_markets(), E.load_audiences(cfg.collector_db), E.load_cn_titles(cfg.collector_db),
                 d, cfg.params, start)


def dumps(inputs: dict) -> str:
    return json.dumps(inputs, ensure_ascii=False, separators=(",", ":"))


def write(inputs: dict, path: Path | None = None) -> Path:
    path = path or inputs_dir() / f"{inputs['day']}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(dumps(inputs), encoding="utf-8")
    tmp.replace(path)
    return path


# ---------------------------------------------------------------- reading (used by the daily close)

@dataclass(frozen=True)
class ShowInputs:
    id: int
    title_ja: str
    title_en: str | None
    studio: str | None
    first_air: date
    episodes: int | None
    episodes_known: bool
    long_runner: bool
    popularity_end: int
    aired: int
    has_real_data: bool
    att: dict
    expected: dict
    preair: dict | None = None             # since v0.3.0
    air_dates: list = field(default_factory=list)
    adult: bool = False                    # since v0.3.1


@dataclass(frozen=True)
class Inputs:
    day: str
    shows: dict[int, ShowInputs]     # model order
    cn_titles: dict[int, str]
    expected: dict[int, dict]        # id -> ShowInputs.expected, for the expected-share denominator


def parse(raw: dict) -> Inputs:
    shows = {}
    for r in raw["shows"]:
        shows[r["id"]] = ShowInputs(**{**r, "first_air": date.fromisoformat(r["first_air"])})
    return Inputs(day=raw["day"], shows=shows, cn_titles={int(k): v for k, v in raw["cn_titles"].items()},
                  expected={i: s.expected for i, s in shows.items()})


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--day", required=True, help="the day being closed, YYYY-MM-DD")
    ap.add_argument("--out", type=Path, help="output path (default: data/derived/inputs/<day>.json)")
    ap.add_argument("--window-start", help="override the game's window start (default: game DB state)")
    ap.add_argument("--from", dest="start",
                    help="also write every day from this one up to --day into the default directory, skipping "
                         "files that exist (history for the server's valuation bootstrap at go-live)")
    args = ap.parse_args()
    ws = date.fromisoformat(args.window_start) if args.window_start else None
    if args.start:
        d, end = date.fromisoformat(args.start), date.fromisoformat(args.day)
        while d <= end:
            if not (inputs_dir() / f"{d.isoformat()}.json").exists():
                inputs = daily_inputs(d.isoformat(), window_start=ws)
                print(f"{write(inputs, None)}: {len(inputs['shows'])} shows")
            d += timedelta(days=1)
        return
    inputs = daily_inputs(args.day, window_start=ws)
    path = write(inputs, args.out)
    print(f"{path}: {len(inputs['shows'])} shows")


if __name__ == "__main__":
    main()
