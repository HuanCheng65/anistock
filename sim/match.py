# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Match the same anime across AniList, Annict and Bangumi. Writes sim/matches.json.

AniList <-> Annict: shared MyAnimeList id.
AniList <-> Bangumi: normalised Japanese titles (+ aliases), then first-air date as a tie-breaker.
The output only holds id pairs, so it can be committed and hand-corrected:
entries under "manual" override anything automatic.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import time
import unicodedata
import urllib.request
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "sim" / "matches.json"
EXTRA = ROOT / "tracked_extra.json"      # read by collect.py: extra Bangumi subjects to track daily
SEARCH_MIN_POPULARITY = 3000
SIM_FROM_YEAR = 2026

SEASON_PAT = [
    (r"第\s*([0-9０-９一二三四五六七八九十]+)\s*(期|季|シーズン|クール|部)", r"S\1"),
    (r"season\s*([0-9]+)", r"S\1"),
    (r"([0-9]+)(st|nd|rd|th)\s*season", r"S\1"),
]
KANJI_NUM = {"一": "1", "二": "2", "三": "3", "四": "4", "五": "5", "六": "6", "七": "7", "八": "8", "九": "9", "十": "10"}


def norm(t: str | None) -> str:
    if not t:
        return ""
    t = unicodedata.normalize("NFKC", t).lower()
    for pat, rep in SEASON_PAT:
        t = re.sub(pat, rep, t, flags=re.I)
    t = re.sub(r"S([一二三四五六七八九十])", lambda m: "S" + KANJI_NUM[m.group(1)], t)
    t = re.sub(r"[\s・:：\-－―~～!！?？、。,，.「」『』()（）\[\]【】'\"“”’]", "", t)
    return t


def stem(t: str) -> str:
    """Title without its season marker, for prefix matching."""
    return re.sub(r"S\d+$", "", t)


def load(db):
    al, an, bg = {}, {}, {}
    for i, d in db.execute("SELECT external_id, data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day"):
        al[int(i)] = json.loads(d)
    for i, d in db.execute("SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work' ORDER BY day"):
        an[int(i)] = json.loads(d)
    for i, d in db.execute("SELECT external_id, data FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day"):
        bg[int(i)] = json.loads(d)
    return al, an, bg


def anilist_titles(m):
    t = m["title"]
    return {norm(x) for x in [t.get("native"), t.get("romaji"), t.get("english"), *(m.get("synonyms") or [])] if x}


def bgm_titles(s):
    names = [s.get("name"), s.get("name_cn")]
    for item in s.get("infobox") or []:
        if item.get("key") in ("别名", "中文名"):
            v = item["value"]
            names += [x.get("v") for x in v] if isinstance(v, list) else [v]
    return {norm(x) for x in names if isinstance(x, str) and x}


def anilist_start(m):
    sd = m.get("startDate") or {}
    try:
        return date(sd["year"], sd["month"], sd["day"])
    except (KeyError, TypeError, ValueError):
        return None


def bgm_start(s):
    try:
        return date.fromisoformat(s.get("date") or "")
    except ValueError:
        return None


def bgm_search(keyword: str) -> list[dict]:
    """One Bangumi search request (anime only). Honours HTTPS_PROXY via urllib's env handling."""
    req = urllib.request.Request(
        "https://api.bgm.tv/v0/search/subjects?limit=5",
        data=json.dumps({"keyword": keyword, "filter": {"type": [2]}}).encode(),
        headers={"Content-Type": "application/json", "User-Agent": "anistock-collector/0.1 (anime club market game)"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r).get("data", [])


def search_missing(al: dict, matches: dict, claimed: set) -> dict[str, dict]:
    """For airing AniList shows with no Bangumi match, try Bangumi's search once each."""
    found = {}
    todo = [(aid, m) for aid, m in al.items()
            if "bgm" not in matches[str(aid)]
            and (m.get("status") == "RELEASING"
                 # finished shows from seasons we simulate, so the China market has a full universe
                 or (m.get("status") == "FINISHED" and (m.get("seasonYear") or 0) >= SIM_FROM_YEAR))
            and (m.get("popularity") or 0) >= SEARCH_MIN_POPULARITY]
    for aid, m in todo:
        ts, start = anilist_titles(m), anilist_start(m)
        try:
            results = bgm_search(m["title"].get("native") or m["title"].get("romaji"))
        except Exception as e:  # network hiccup: leave it unmatched
            print(f"  search failed for {aid}: {e}")
            continue
        time.sleep(1.0)
        for r in results:
            if r["id"] in claimed:
                continue
            bts = {norm(r.get("name")), norm(r.get("name_cn"))} - {""}
            rstart = bgm_start(r)
            close = start and rstart and abs((start - rstart).days) <= 7
            if (ts & bts) and (close or not (start and rstart)):
                found[str(aid)] = {"bgm": r["id"], "bgm_confidence": 0.9, "via": "search"}
            elif close and any(len(a) >= 4 and (stem(a) in b or b in a) for a in ts for b in bts):
                found[str(aid)] = {"bgm": r["id"], "bgm_confidence": 0.7, "via": "search"}
            if str(aid) in found:
                claimed.add(r["id"])
                print(f"  search: {m['title'].get('native')}  ->  {r['id']} {r.get('name')}")
                break
    return found


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--search", action="store_true",
                    help="query Bangumi search for airing shows missing from its calendar (one request each)")
    args = ap.parse_args()
    db = sqlite3.connect(f"file:{ROOT / 'data' / 'anistock.db'}?mode=ro", uri=True)
    al, an, bg = load(db)
    manual = {}
    if OUT.exists():
        manual = json.loads(OUT.read_text(encoding="utf-8")).get("manual", {})

    by_mal = {}
    for aid, m in al.items():
        if m.get("idMal"):
            by_mal[int(m["idMal"])] = aid

    bg_titles = {sid: bgm_titles(s) for sid, s in bg.items()}
    matches = {}
    for aid, m in al.items():
        row = {"anilist": aid, "title": m["title"].get("native") or m["title"].get("romaji")}
        # Annict via MAL id
        for wid, w in an.items():
            if w.get("malAnimeId") and m.get("idMal") and int(w["malAnimeId"]) == int(m["idMal"]):
                row["annict"] = wid
                break
        # Bangumi via titles
        ts, start = anilist_titles(m), anilist_start(m)
        best, conf = None, 0.0
        for sid, bts in bg_titles.items():
            bstart = bgm_start(bg[sid])
            close = start and bstart and abs((start - bstart).days) <= 3
            if ts & bts:
                c = 1.0 if close or not (start and bstart) else 0.8
            elif close and any(stem(a) and stem(a) == stem(b) for a in ts for b in bts):
                c = 0.7
            elif close and any(len(a) >= 6 and (a in b or b in a) for a in ts for b in bts):
                c = 0.6
            else:
                continue
            if c > conf:
                best, conf = sid, c
        if best is not None:
            row["bgm"], row["bgm_confidence"] = best, conf
        matches[str(aid)] = row

    # one Bangumi subject must not be claimed twice: keep the highest confidence
    claimed = {}
    for k, row in matches.items():
        if "bgm" in row:
            prev = claimed.get(row["bgm"])
            if prev is None or row["bgm_confidence"] > matches[prev]["bgm_confidence"]:
                if prev is not None:
                    matches[prev].pop("bgm"); matches[prev].pop("bgm_confidence")
                claimed[row["bgm"]] = k
            else:
                row.pop("bgm"); row.pop("bgm_confidence")

    # keep earlier search results; optionally search for the rest
    prev = json.loads(OUT.read_text(encoding="utf-8")).get("matches", {}) if OUT.exists() else {}
    for k, row in prev.items():
        if row.get("via") == "search" and k in matches and "bgm" not in matches[k] and row["bgm"] not in claimed:
            matches[k].update({x: row[x] for x in ("bgm", "bgm_confidence", "via")})
            claimed[row["bgm"]] = k
    if args.search:
        for k, hit in search_missing(al, matches, set(claimed)).items():
            matches[k].update(hit)
            claimed[hit["bgm"]] = k

    for k, override in manual.items():
        matches.setdefault(k, {"anilist": int(k)}).update(override)

    unmatched_bgm = sorted(
        ({"bgm": sid, "name": bg[sid].get("name"), "name_cn": bg[sid].get("name_cn"), "date": bg[sid].get("date")}
         for sid in bg if sid not in claimed and sid not in {r.get("bgm") for r in manual.values()}),
        key=lambda x: x["date"] or "")
    OUT.write_text(json.dumps({
        "note": "id pairs only. Put corrections under 'manual' keyed by AniList id, e.g. {\"12345\": {\"bgm\": 678}}",
        "manual": manual,
        "matches": matches,
        "unmatched_bgm": unmatched_bgm,
    }, ensure_ascii=False, indent=1), encoding="utf-8")

    # every matched subject, not just the ones missing locally: the server's DB is separate, and
    # re-registering a subject the calendar already lists is harmless
    extra = sorted({r["bgm"] for r in matches.values() if "bgm" in r})
    EXTRA.write_text(json.dumps({"note": "written by sim/match.py; Bangumi subjects missing from its calendar",
                                 "bgm": extra}, indent=1) + "\n", encoding="utf-8")

    n = len(matches)
    print(f"anilist={n}  with annict={sum('annict' in r for r in matches.values())}  "
          f"with bgm={sum('bgm' in r for r in matches.values())}  all three="
          f"{sum('annict' in r and 'bgm' in r for r in matches.values())}")
    conf = {}
    for r in matches.values():
        if "bgm_confidence" in r:
            conf[r["bgm_confidence"]] = conf.get(r["bgm_confidence"], 0) + 1
    print("bgm confidence:", dict(sorted(conf.items(), reverse=True)), f" unmatched bgm subjects: {len(unmatched_bgm)}")


if __name__ == "__main__":
    main()
