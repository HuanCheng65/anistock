"""Game configuration: simulation parameters + game overrides + game rules, and the game clock."""

from __future__ import annotations

import copy
import os
import tomllib
from dataclasses import dataclass
from datetime import date, datetime, time, timedelta
from functools import lru_cache
from pathlib import Path
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parent.parent


@dataclass
class Config:
    params: dict          # economic parameters (same shape as sim/params.toml)
    game: dict
    npc: dict
    tz: ZoneInfo
    collector_db: Path
    game_db: Path
    secret: bytes
    invite_code: str


def _set(d: dict, path: str, value) -> None:
    *secs, key = path.split(".")
    for s in secs:
        d = d[s]
    d[key] = value


@lru_cache
def load() -> Config:
    raw = tomllib.loads((ROOT / "server" / "game.toml").read_text(encoding="utf-8"))
    params = tomllib.loads((ROOT / raw["base_params"]).read_text(encoding="utf-8"))
    params = copy.deepcopy(params)
    for k, v in raw.get("overrides", {}).items():
        _set(params, k, v)
    # ANISTOCK_PARAMS_FILE: further dotted overrides, applied last (the same hook as backend/src/config.rs)
    extra = os.environ.get("ANISTOCK_PARAMS_FILE", "").strip()
    if extra:
        for k, v in tomllib.loads(Path(extra).read_text(encoding="utf-8")).items():
            _set(params, k, v)
    game = raw["game"]
    secret = os.environ.get("ANISTOCK_SECRET", "").strip()
    invite = os.environ.get("ANISTOCK_INVITE", "").strip()
    for name, v in (("ANISTOCK_SECRET", secret), ("ANISTOCK_INVITE", invite)):
        if v.startswith("#"):
            raise SystemExit(f"{name} looks like a comment ({v[:20]}...): put comments on their own line in .env")
    return Config(
        params=params, game=game, npc=raw["npc"], tz=ZoneInfo(game["timezone"]),
        collector_db=Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db")),
        game_db=Path(os.environ.get("ANISTOCK_GAME_DB", ROOT / "data" / "game.db")),
        secret=(secret or "dev-only-secret-change-me").encode(),
        invite_code=invite or "anistock",
    )


class Clock:
    """Wall clock in game time. Tests and replays can pin it with `freeze`."""

    def __init__(self, tz: ZoneInfo, rollover: str = "00:00"):
        self.tz = tz
        self.rollover = _hm(rollover)  # the trading day changes here (game.day_rollover)
        self._frozen: datetime | None = None

    def now(self) -> datetime:
        return self._frozen or datetime.now(self.tz)

    def freeze(self, when: datetime | None) -> None:
        self._frozen = when

    def today(self) -> str:
        r = self.rollover
        return (self.now() - timedelta(hours=r.hour, minutes=r.minute)).date().isoformat()

    def ts(self) -> str:
        return self.now().isoformat(timespec="seconds")

    def trading(self, game: dict) -> bool:
        return within(self.now().time(), game["trading_open"], game["trading_close"])


def within(t: time, start: str, end: str) -> bool:
    """start <= t < end by time of day; an end before the start wraps past midnight (night session)."""
    a, b = _hm(start), _hm(end)
    return a <= t < b if a <= b else (t >= a or t < b)


def _hm(s: str) -> time:
    h, m = s.split(":")
    return time(int(h), int(m))


def next_daily_step(now: datetime, at: str) -> datetime:
    t = _hm(at)
    candidate = now.replace(hour=t.hour, minute=t.minute, second=0, microsecond=0)
    return candidate if candidate > now else candidate + timedelta(days=1)


def prev_day(day: str) -> str:
    return (date.fromisoformat(day) - timedelta(days=1)).isoformat()
