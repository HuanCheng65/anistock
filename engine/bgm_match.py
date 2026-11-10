"""AniList -> Bangumi matching, once per show, when the collector meets it (collect.py job `bgm_match`).

Bangumi carries no MyAnimeList id, so a show is matched by title: AniList's native / romaji / English titles and
synonyms against Bangumi's name, Chinese name and aliases, normalised (`norm`), with the first-air dates as the
check. Candidates are the Bangumi subjects the collector already snapshots (its calendar, the tracked extras);
a show none of them fits is looked up once with Bangumi's search (again a week later if nothing came up: a
show's subject often appears only shortly before it airs).

A match is kept in the collector database (`bgm_matches`) and never revisited; the matched subject is registered
for the daily Bangumi collection and snapshotted at once. `sim/matches.json` stays the curated layer above it:
its `matches` (the last run of sim/match.py) and its `manual` corrections win (engine/economy.py
`effective_matches`, backend/src/api/v1/stocks.rs `matches`).
"""

from __future__ import annotations

import json
import re
import sqlite3
import unicodedata
from datetime import date, datetime, timedelta, timezone

SCHEMA = """
CREATE TABLE IF NOT EXISTS bgm_matches (
    anilist_id  INTEGER PRIMARY KEY,
    bgm_id      INTEGER NOT NULL,
    confidence  REAL NOT NULL,
    via         TEXT NOT NULL,              -- calendar | search
    matched_on  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS bgm_match_tries (
    anilist_id  INTEGER PRIMARY KEY,
    searched_on TEXT NOT NULL               -- the last Bangumi search for it that found nothing
);
"""

# shows worth a match: what the game may list (docs: game.min_listing_popularity gates the listing itself)
MIN_POPULARITY = 1000
SEARCH_MIN_POPULARITY = MIN_POPULARITY  # a search a week at most per show: cheap
SEARCH_AGAIN_DAYS = 7
# search for an upcoming show this many days before its premiere at the earliest (its subject may not exist yet)
SEARCH_AHEAD_DAYS = 45
FORMATS = {"TV", "TV_SHORT", "ONA"}

SEASON_PAT = [
    (r"第\s*([0-9一二三四五六七八九十]+)\s*(期|季|シーズン|クール|部)", r"s\1"),
    (r"season\s*([0-9]+)", r"s\1"),
    (r"([0-9]+)(st|nd|rd|th)\s*season", r"s\1"),
    # a bare number closing the title after a space: ゲルぴよ 2 = ゲルぴよ 第2期
    (r"\s([0-9]{1,2})$", r" s\1"),
]
KANJI_NUM = {"一": "1", "二": "2", "三": "3", "四": "4", "五": "5", "六": "6", "七": "7", "八": "8", "九": "9", "十": "10"}
# tags in brackets that are not part of the title: (TV), (2025)
TAGS = r"[(\[]\s*(tv|tvアニメ|tv版|[0-9]{4})\s*[)\]]"


def norm(t: str | None) -> str:
    if not t:
        return ""
    t = unicodedata.normalize("NFKC", t).lower().strip()
    t = re.sub(TAGS, "", t).strip()
    for pat, rep in SEASON_PAT:
        t = re.sub(pat, rep, t, flags=re.I)
    t = re.sub(r"s([一二三四五六七八九十])", lambda m: "s" + KANJI_NUM[m.group(1)], t)
    t = re.sub(r"[\s・:：\-－―‐~～〜〰!！?？、。,，.「」『』()（）\[\]【】'\"“”’☆★♪]", "", t)
    return t


def stem(t: str) -> str:
    """Title without its season marker."""
    return re.sub(r"s\d+$", "", t)


def anilist_titles(m: dict) -> set[str]:
    t = m.get("title") or {}
    return {norm(x) for x in [t.get("native"), t.get("romaji"), t.get("english"), *(m.get("synonyms") or [])] if x} - {""}


def bgm_titles(s: dict) -> set[str]:
    names = [s.get("name"), s.get("name_cn")]
    for item in s.get("infobox") or []:
        if item.get("key") in ("别名", "中文名"):
            v = item["value"]
            names += [x.get("v") for x in v] if isinstance(v, list) else [v]
    return {norm(x) for x in names if isinstance(x, str) and x} - {""}


def anilist_start(m: dict) -> date | None:
    sd = m.get("startDate") or {}
    try:
        return date(sd["year"], sd["month"], sd["day"])
    except (KeyError, TypeError, ValueError):
        return None


def bgm_start(s: dict) -> date | None:
    try:
        return date.fromisoformat(s.get("date") or "")
    except ValueError:
        return None


def score(ts: set[str], start: date | None, bts: set[str], bstart: date | None, near: int = 3) -> float:
    """How well a Bangumi subject's titles fit a show's: 1.0 a shared title and the premieres within `near` days
    (or one unknown), 0.7 the same title without the season marker and the premieres within `near` days; 0 no
    match. Unattended, a shared title with the premieres apart is not a match (a remake, the OVA a TV cut is made
    from: Girls und Panzer もっとらぶらぶ作戦です！); sim/match.py takes it at 0.8, and a looser 0.6 tier, for review."""
    close = bool(start and bstart and abs((start - bstart).days) <= near)
    if ts & bts and (close or not (start and bstart)):
        return 1.0
    if close and any(stem(a) and stem(a) == stem(b) for a in ts for b in bts):
        return 0.7
    return 0.0


def best(m: dict, subjects: dict[int, dict], claimed: set[int], near: int = 3) -> tuple[int, float] | None:
    """The best subject for AniList media `m` among `subjects` not claimed by another show: highest score, then the
    closest premiere."""
    ts, start = anilist_titles(m), anilist_start(m)
    top: tuple[float, int, int] | None = None
    for sid, s in subjects.items():
        if sid in claimed:
            continue
        bstart = bgm_start(s)
        c = score(ts, start, bgm_titles(s), bstart, near)
        if c <= 0:
            continue
        gap = abs((start - bstart).days) if start and bstart else 9999
        if top is None or (c, -gap) > (top[0], -top[2]):
            top = (c, sid, gap)
    return (top[1], top[0]) if top else None


MEMBER_FORMATS = FORMATS | {"MOVIE", "OVA", "SPECIAL"}


def wanted(m: dict, member: bool = False) -> bool:
    """A show worth a match: airing or upcoming; or an earlier work of a series (collect.py `collect_series_members`:
    finished, and possibly a film, an OVA or a special), whose long tail joins a listed company."""
    if m.get("isAdult") or "Hentai" in (m.get("genres") or []) or (m.get("popularity") or 0) < MIN_POPULARITY:
        return False
    if member:
        return m.get("format") in MEMBER_FORMATS and m.get("status") != "NOT_YET_RELEASED"
    return m.get("format") in FORMATS and m.get("status") in ("RELEASING", "NOT_YET_RELEASED")


def latest(db: sqlite3.Connection, source: str, kind: str) -> dict[int, dict]:
    out: dict[int, dict] = {}
    for i, d in db.execute("SELECT external_id, data FROM snapshots WHERE source=? AND kind=? ORDER BY day", (source, kind)):
        out[int(i)] = json.loads(d)
    return out


def curated(path) -> tuple[set[int], set[int]]:
    """AniList ids sim/matches.json already gives a Bangumi subject (its matches and manual), and those subjects."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return set(), set()
    shows, subjects = set(), set()
    for part in ("matches", "manual"):
        for k, v in (data.get(part) or {}).items():
            if v.get("bgm"):
                shows.add(int(k))
                subjects.add(int(v["bgm"]))
    return shows, subjects


def match_new(db: sqlite3.Connection, day: str, matches_file, search=None, fetch=None) -> dict:
    """Match the shows the collector tracks that have no Bangumi subject yet. `search(keyword) -> [subject]`: Bangumi's
    search (None: local candidates only); `fetch(subject_id) -> subject | None`: one subject, to snapshot a new
    match at once. Returns counts, and writes the matches (the caller commits)."""
    db.executescript(SCHEMA)
    shows = latest(db, "anilist", "media")
    subjects = latest(db, "bgm", "subject")
    have, cur_subjects = curated(matches_file)
    done = {r[0]: r[1] for r in db.execute("SELECT anilist_id, bgm_id FROM bgm_matches")}
    claimed = cur_subjects | set(done.values())
    tries = {r[0]: r[1] for r in db.execute("SELECT anilist_id, searched_on FROM bgm_match_tries")}
    today = date.fromisoformat(day)
    out = {"todo": 0, "calendar": 0, "search": 0, "searched": 0}
    tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    members = {r[0] for r in db.execute("SELECT anilist_id FROM series_members")} if "series_members" in tables else set()
    todo = [(aid, m) for aid, m in shows.items() if aid not in have and aid not in done and wanted(m, aid in members)]
    out["todo"] = len(todo)
    for aid, m in sorted(todo, key=lambda x: -(x[1].get("popularity") or 0)):
        hit = best(m, subjects, claimed)
        via = "calendar"
        if hit is None and search is not None and (m.get("popularity") or 0) >= SEARCH_MIN_POPULARITY:
            start = anilist_start(m)
            last = tries.get(aid)
            due = last is None or date.fromisoformat(last) <= today - timedelta(days=SEARCH_AGAIN_DAYS)
            soon = start is None or start <= today + timedelta(days=SEARCH_AHEAD_DAYS)
            if due and soon:
                t = m.get("title") or {}
                found: dict[int, dict] = {}
                for kw in dict.fromkeys(x for x in (t.get("native"), t.get("romaji")) if x):
                    for s in search(kw) or []:
                        found.setdefault(int(s["id"]), s)
                    if found:
                        break
                out["searched"] += 1
                # (a search hit's date may be the web premiere a few days off the broadcast)
                hit = best(m, found, claimed, near=7)
                via = "search"
                if hit is None:
                    db.execute("INSERT OR REPLACE INTO bgm_match_tries VALUES (?,?)", (aid, day))
                else:
                    subjects.setdefault(hit[0], found[hit[0]])
        if hit is None:
            continue
        sid, conf = hit
        db.execute("INSERT OR REPLACE INTO bgm_matches VALUES (?,?,?,?,?)", (aid, sid, conf, via, day))
        db.execute("DELETE FROM bgm_match_tries WHERE anilist_id=?", (aid,))
        claimed.add(sid)
        out[via] += 1
        if fetch is not None and sid not in latest_ids(db):
            s = fetch(sid)
            if s:
                db.execute("INSERT OR REPLACE INTO snapshots VALUES (?,?,?,?,?,?)",
                           ("bgm", "subject", str(sid), day,
                            datetime.now(timezone(timedelta(hours=8))).isoformat(timespec="seconds"),
                            json.dumps(s, ensure_ascii=False, separators=(",", ":"))))
    return out


def latest_ids(db: sqlite3.Connection) -> set[int]:
    return {int(r[0]) for r in db.execute("SELECT DISTINCT external_id FROM snapshots WHERE source='bgm' AND kind='subject'")}


def table(db: sqlite3.Connection) -> dict[str, int]:
    """AniList id (str) -> Bangumi subject from `bgm_matches` (empty when the table does not exist yet)."""
    try:
        return {str(a): int(b) for a, b in db.execute("SELECT anilist_id, bgm_id FROM bgm_matches")}
    except sqlite3.OperationalError:
        return {}
