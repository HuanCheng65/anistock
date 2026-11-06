# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx[socks]>=0.27"]
# ///
"""Rebuild weekly history for the Japan (Annict) and China (Bangumi) markets from snapshots.

Idea: each episode's cumulative count belongs to the week that episode aired. Episode k's
Annict watch records ≈ that week's viewers; episode k's Bangumi comments ≈ that week's buzz.
Recent episodes are still accumulating, so counts are divided by a maturity curve F(age)
measured once on a small sample.

    uv run sim/reconstruct.py airtimes     # Annict first-broadcast time per episode (~10 requests)
    uv run sim/reconstruct.py calibrate    # maturity curves from ~15 Annict + ~10 Bangumi episodes
    uv run sim/reconstruct.py build        # weekly per-market series -> data/derived/markets.json

Everything written here is derived data under data/derived/ (git-ignored). Only aggregate
counts are stored; individual records/comments are counted and discarded (Annict API terms Art. 6).
"""

from __future__ import annotations

import json
import os
import random
import sqlite3
import sys
import time
from collections import defaultdict
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parent.parent
DB = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
# ANISTOCK_DERIVED_DIR: where markets.json is written (default data/derived; annict_airtimes.json is read from
# data/derived unless it exists there)
DERIVED_OUT = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived"))
DERIVED = DERIVED_OUT if (DERIVED_OUT / "annict_airtimes.json").exists() else ROOT / "data" / "derived"
CALIBRATION = ROOT / "sim" / "calibration"   # tracked in git: small aggregate curves, costly to re-measure
UA = "anistock-collector/0.1 (anime club market game)"
MAX_AGE = 28  # maturity curve horizon, days


def load_dotenv() -> None:
    f = ROOT / ".env"
    if f.is_file():
        for line in f.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, _, v = line.partition("=")
                os.environ.setdefault(k.strip(), v.split(" #", 1)[0].strip().strip('"').strip("'"))


def db():
    return sqlite3.connect(f"file:{DB}?mode=ro", uri=True)


def latest(source: str, kind: str) -> dict[int, dict]:
    out = {}
    for i, d in db().execute("SELECT external_id, data FROM snapshots WHERE source=? AND kind=? ORDER BY day",
                             (source, kind)):
        out[int(i)] = json.loads(d)
    return out


def matches() -> dict:
    # sim/matches.json plus the Annict works the collector found by MAL id, plus the manual corrections
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    from engine.economy import effective_matches
    return effective_matches(DB)


def annict_gql(client: httpx.Client, query: str, variables: dict, partial_ok: bool = False) -> dict:
    for attempt in range(5):
        r = client.post("https://api.annict.com/graphql", json={"query": query, "variables": variables})
        if r.status_code == 429:
            time.sleep(int(r.headers.get("retry-after", "30")) + 1)
            continue
        body = r.json()
        if r.status_code != 200 or (body.get("errors") and not (partial_ok and body.get("data"))):
            raise RuntimeError(f"Annict {r.status_code}: {str(body.get('errors'))[:300]}")
        if r.headers.get("ratelimit-remaining", "9").isdigit() and int(r.headers["ratelimit-remaining"]) < 5:
            time.sleep(int(r.headers.get("ratelimit-reset", "60")) + 1)
        time.sleep(0.3)
        return body["data"]
    raise RuntimeError("Annict: too many 429s")


def annict_client() -> httpx.Client:
    token = os.environ.get("ANNICT_TOKEN", "").strip()
    if not token:
        sys.exit("ANNICT_TOKEN not set (put it in .env)")
    return httpx.Client(headers={"Authorization": f"Bearer {token}", "User-Agent": UA}, timeout=60, trust_env=True)


# ---------------------------------------------------------------- airtimes

PROGRAMS_Q = """
query ($ids: [Int!]) {
  searchWorks(annictIds: $ids, first: 25) {
    nodes {
      annictId
      programs(orderBy: {field: STARTED_AT, direction: DESC}, first: 150) {
        nodes { startedAt rebroadcast episode { annictId sortNumber } }
      }
    }
  }
}
"""


def cmd_airtimes() -> None:
    works = latest("annict", "work")
    ids = sorted(wid for wid, w in works.items() if not w.get("noEpisodes") and w.get("episodes"))
    client = annict_client()
    out = {}
    for i in range(0, len(ids), 25):
        # some programs point at no episode and Annict nulls that work's whole list; keep the rest
        nodes = annict_gql(client, PROGRAMS_Q, {"ids": ids[i:i + 25]}, partial_ok=True)["searchWorks"]["nodes"]
        for w in nodes:
            if not w:
                continue
            first: dict[int, str] = {}
            for p in ((w.get("programs") or {}).get("nodes") or []):
                if not p or p["rebroadcast"] or not p.get("episode"):
                    continue
                eid = p["episode"]["annictId"]
                if eid not in first or p["startedAt"] < first[eid]:
                    first[eid] = p["startedAt"]
            out[w["annictId"]] = first
        print(f"  programs {min(i + 25, len(ids))}/{len(ids)}")
    DERIVED.mkdir(parents=True, exist_ok=True)
    (DERIVED / "annict_airtimes.json").write_text(json.dumps(out), encoding="utf-8")
    n_ep = sum(len(v) for v in out.values())
    print(f"works={len(out)} with_airtimes={sum(1 for v in out.values() if v)} episodes={n_ep}")


# ---------------------------------------------------------------- calibrate

RECORDS_Q = """
query ($id: Int!, $after: String) {
  searchEpisodes(annictIds: [$id], first: 1) {
    nodes {
      recordsCount
      records(orderBy: {field: CREATED_AT, direction: ASC}, first: 50, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { createdAt }
      }
    }
  }
}
"""


def _curve(ages_list: list[list[float]]) -> list[float]:
    """F[d] = average fraction, of the count within MAX_AGE+1 days, logged within d+1 days of airing.

    Normalising by the (MAX_AGE+1)-day count rather than the total at sampling time keeps every
    sampled episode on the same horizon, whatever its age when it was sampled.
    """
    fr = []
    for d in range(MAX_AGE + 1):
        vals = []
        for ages in ages_list:
            base = sum(1 for a in ages if a <= MAX_AGE + 1)
            if base:
                vals.append(sum(1 for a in ages if a <= d + 1) / base)
        fr.append(round(sum(vals) / len(vals), 4) if vals else 1.0)
    return fr


def maturity(curve: list[float], age_days: float) -> float:
    """Fraction of the 29-day count expected by `age_days` after airing (linear between F points)."""
    x = age_days - 1                      # F[d] covers "within d+1 days"
    if x <= 0:
        return curve[0] * max(age_days, 0.0)
    if x >= len(curve) - 1:
        return 1.0
    i = int(x)
    return curve[i] + (curve[i + 1] - curve[i]) * (x - i)


def cmd_calibrate() -> None:
    rng = random.Random(7)
    now = datetime.now(timezone.utc)
    air = json.loads((DERIVED / "annict_airtimes.json").read_text(encoding="utf-8"))
    works = latest("annict", "work")

    # --- Annict: episodes aired 28-60 days ago, spread across popularity
    cands = []
    for wid, w in works.items():
        times = air.get(str(wid), {})
        for e in w.get("episodes", []):
            t = times.get(str(e["annictId"]))
            if not t:
                continue
            age = (now - datetime.fromisoformat(t.replace("Z", "+00:00"))).days
            if 28 <= age <= 60 and e["recordsCount"] >= 30:
                cands.append((e["recordsCount"], e["annictId"], t))
    cands.sort()
    picks = [cands[int(q * (len(cands) - 1))] for q in [i / 14 for i in range(15)]] if len(cands) >= 15 else cands
    client = annict_client()
    an_ages = []
    for count, eid, t in picks:
        aired = datetime.fromisoformat(t.replace("Z", "+00:00"))
        ages, after = [], None
        while True:
            node = annict_gql(client, RECORDS_Q, {"id": eid, "after": after})["searchEpisodes"]["nodes"][0]
            conn = node["records"]
            ages += [(datetime.fromisoformat(r["createdAt"].replace("Z", "+00:00")) - aired).total_seconds() / 86400
                     for r in conn["nodes"]]
            if not conn["pageInfo"]["hasNextPage"]:
                break
            after = conn["pageInfo"]["endCursor"]
        ages = [max(0.0, a) for a in ages]
        an_ages.append(ages)
        late = sum(1 for a in ages if a > 14) / max(1, len(ages))
        print(f"  annict ep {eid}: {len(ages)} records, {late:.0%} after day 14")

    # --- Bangumi: episode comments via next.bgm.tv (unofficial endpoint: one request each, slow)
    eps = latest("bgm", "episodes")
    bcands = []
    for sid, blob in eps.items():
        for e in blob["data"]:
            try:
                ad = date.fromisoformat(e.get("airdate") or "")
            except ValueError:
                continue
            age = (now.date() - ad).days
            if 28 <= age <= 60 and (e.get("comment") or 0) >= 20:
                bcands.append((e["comment"], e["id"], ad))
    bcands.sort()
    bpicks = [bcands[int(q * (len(bcands) - 1))] for q in [i / 9 for i in range(10)]] if len(bcands) >= 10 else bcands
    bg_ages = []
    with httpx.Client(headers={"User-Agent": UA}, timeout=30, trust_env=True) as c:
        for count, eid, ad in bpicks:
            r = c.get(f"https://next.bgm.tv/p1/episodes/{eid}/comments")
            time.sleep(1.0)
            if r.status_code != 200:
                print(f"  bgm ep {eid}: HTTP {r.status_code}, skipped")
                continue
            # Bangumi airdate is a JST calendar day; count from that day's 00:00 JST
            start = datetime(ad.year, ad.month, ad.day, tzinfo=timezone(timedelta(hours=9))).timestamp()
            ts = [x["createdAt"] for x in r.json()] + [y["createdAt"] for x in r.json() for y in x.get("replies", [])]
            ages = [max(0.0, (t - start) / 86400) for t in ts]
            bg_ages.append(ages)
            print(f"  bgm ep {eid}: {len(ages)} comments, {sum(1 for a in ages if a > 14) / max(1, len(ages)):.0%} after day 14")

    out = {
        "horizon_days": MAX_AGE,
        "annict": {"episodes": len(an_ages), "fraction_by_day": _curve(an_ages)},
        "bgm": {"episodes": len(bg_ages), "fraction_by_day": _curve(bg_ages)},
        "note": "fraction of the day-28 count reached by day offset d (index). Aggregates only.",
    }
    CALIBRATION.mkdir(parents=True, exist_ok=True)
    (CALIBRATION / "maturity.json").write_text(json.dumps(out, indent=1), encoding="utf-8")
    for k in ("annict", "bgm"):
        f = out[k]["fraction_by_day"]
        print(f"{k}: n={out[k]['episodes']}  day0={f[0]:.0%} day1={f[1]:.0%} day3={f[3]:.0%} "
              f"day7={f[7]:.0%} day14={f[14]:.0%}")




# ---------------------------------------------------------------- per-episode series + cross-check

def _fetched_at(source: str, kind: str) -> dict[int, datetime]:
    out = {}
    for i, f in db().execute("SELECT external_id, fetched_at FROM snapshots WHERE source=? AND kind=? ORDER BY day",
                             (source, kind)):
        out[int(i)] = datetime.fromisoformat(f)
    return out


def _fallback_airtimes(m: dict, fetched: dict[int, datetime]) -> dict[int, dict[str, str]]:
    """Air times for Annict works whose program list Annict could not return.

    Annict numbers sequels continuously (第49話), so episodes are aligned by order from the newest
    one that has records, against Bangumi airdates (or AniList episode markers as a second choice).
    """
    jst = timezone(timedelta(hours=9))
    bg_eps = latest("bgm", "episodes")
    marks = defaultdict(list)
    for mid, d, data in db().execute("SELECT media_id, date, data FROM anilist_trends"):
        if json.loads(data).get("episode") is not None:
            marks[mid].append(date.fromisoformat(d))
    out = {}
    for aid, r in m.items():
        wid = r.get("annict")
        if wid is None or wid not in fetched:
            continue
        cutoff = fetched[wid].date()
        days = []
        if r.get("bgm") in bg_eps:
            for e in bg_eps[r["bgm"]]["data"]:
                try:
                    d = date.fromisoformat(e.get("airdate") or "")
                except ValueError:
                    continue
                if d <= cutoff:
                    days.append(d)
        if not days:
            days = [d for d in marks.get(int(aid), []) if d <= cutoff]
        if days:
            out[wid] = sorted(set(days))
    work_eps = latest("annict", "work")
    result = {}
    for wid, days in out.items():
        eps = sorted((e for e in work_eps[wid].get("episodes", []) if e["recordsCount"] > 0),
                     key=lambda e: e["sortNumber"])
        n = min(len(eps), len(days))
        if n:
            result[wid] = {str(e["annictId"]): datetime(d.year, d.month, d.day, tzinfo=jst).isoformat()
                           for e, d in zip(eps[-n:], days[-n:])}
    return result


def episode_series(min_age: float = 3.0) -> dict[str, dict[int, list[tuple[datetime, float, float]]]]:
    """Per market: AniList id -> [(air time, matured count, age days)] for episodes at least min_age old."""
    cur = json.loads((CALIBRATION / "maturity.json").read_text(encoding="utf-8"))
    m = matches()
    an_by_work = {r["annict"]: int(k) for k, r in m.items() if "annict" in r}
    bg_by_subject = {r["bgm"]: int(k) for k, r in m.items() if "bgm" in r}
    out = {"jp": defaultdict(list), "cn": defaultdict(list)}

    air = json.loads((DERIVED / "annict_airtimes.json").read_text(encoding="utf-8"))
    fetched = _fetched_at("annict", "work")
    fallback = _fallback_airtimes(m, fetched)
    for wid, w in latest("annict", "work").items():
        if wid not in an_by_work:
            continue
        # Annict's program list can miss a work's early episodes (it lists recent programs): those episodes'
        # records exist but had no air time and were dropped, so the jp series started weeks late. Fill the
        # gaps with the Bangumi-aligned fallback; Annict's own times win where they exist.
        times = {**fallback.get(wid, {}), **(air.get(str(wid)) or {})}
        for e in w.get("episodes", []):
            t = times.get(str(e["annictId"]))
            if not t:
                continue
            at_ = datetime.fromisoformat(t.replace("Z", "+00:00"))
            age = (fetched[wid] - at_).total_seconds() / 86400
            if age < min_age:
                continue
            out["jp"][an_by_work[wid]].append((at_, e["recordsCount"] / maturity(cur["annict"]["fraction_by_day"], age), age))

    fetched = _fetched_at("bgm", "episodes")
    jst = timezone(timedelta(hours=9))
    for sid, blob in latest("bgm", "episodes").items():
        if sid not in bg_by_subject:
            continue
        for e in blob["data"]:
            try:
                ad = date.fromisoformat(e.get("airdate") or "")
            except ValueError:
                continue
            at_ = datetime(ad.year, ad.month, ad.day, tzinfo=jst)
            age = (fetched[sid] - at_).total_seconds() / 86400
            if age < min_age or not e.get("comment"):
                continue
            out["cn"][bg_by_subject[sid]].append((at_, e["comment"] / maturity(cur["bgm"]["fraction_by_day"], age), age))
    for mk in out:
        for k in out[mk]:
            out[mk][k].sort()
    return out


def _rank(xs):
    order = sorted(range(len(xs)), key=lambda i: xs[i])
    r = [0.0] * len(xs)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and xs[order[j + 1]] == xs[order[i]]:
            j += 1
        for k in range(i, j + 1):
            r[order[k]] = (i + j) / 2
        i = j + 1
    return r


def spearman(a, b):
    ra, rb = _rank(a), _rank(b)
    n = len(a)
    ma, mb = sum(ra) / n, sum(rb) / n
    cov = sum((x - ma) * (y - mb) for x, y in zip(ra, rb))
    va = sum((x - ma) ** 2 for x in ra) ** 0.5
    vb = sum((y - mb) ** 2 for y in rb) ** 0.5
    return cov / (va * vb) if va and vb else None


def anilist_week_trending() -> dict[int, dict[date, float]]:
    out = defaultdict(dict)
    for mid, d, data in db().execute("SELECT media_id, date, data FROM anilist_trends"):
        out[mid][date.fromisoformat(d)] = json.loads(data).get("trending") or 0
    return out


def cmd_check() -> None:
    series = episode_series()
    tr = anilist_week_trending()
    titles = {int(k): r["title"] for k, r in matches().items()}

    def week_sum(mid, start: date):
        return sum(tr[mid].get(start + timedelta(days=k), 0) for k in range(7))

    for mk, label in (("jp", "Annict 看过"), ("cn", "Bangumi 吐槽")):
        rhos = []
        for mid, eps in series[mk].items():
            pts = [(c, week_sum(mid, t.date())) for t, c, age in eps if age >= 7]
            if len(pts) >= 4 and any(p[1] for p in pts):
                rho = spearman([p[0] for p in pts], [p[1] for p in pts])
                if rho is not None:
                    rhos.append(rho)
        rhos.sort()
        med = rhos[len(rhos) // 2] if rhos else None
        print(f"{label}: within-show rank correlation vs AniList weekly trending, shows={len(rhos)} "
              f"median={med:.2f} p25={rhos[len(rhos) // 4]:.2f} p75={rhos[3 * len(rhos) // 4]:.2f}" if rhos else f"{label}: no data")

        # cross-section: episodes aired in the last complete week (7-14 days old)
        last = {}
        for mid, eps in series[mk].items():
            recent = [(c, t) for t, c, age in eps if 7 <= age < 14]
            if recent:
                last[mid] = recent[-1]
        if last:
            wk = {mid: week_sum(mid, t.date()) for mid, (c, t) in last.items()}
            top_m = sorted(last, key=lambda k: -last[k][0])[:10]
            top_a = set(sorted(wk, key=lambda k: -wk[k])[:10])
            print(f"  top10 by {label} (★ = also AniList top10 among the same shows):")
            for mid in top_m:
                print(f"    {'★' if mid in top_a else ' '} {titles[mid][:24]:26} {last[mid][0]:8.0f}")


# ---------------------------------------------------------------- build

def snapshot_history(source: str, kind: str) -> dict[int, dict[date, dict]]:
    """Every day's snapshot of every work: {external id: {day: data}} (the collector keeps one row per day)."""
    out: dict[int, dict[date, dict]] = defaultdict(dict)
    for i, d, data in db().execute("SELECT external_id, day, data FROM snapshots WHERE source=? AND kind=?", (source, kind)):
        out[int(i)][date.fromisoformat(d)] = json.loads(data)
    return out


def collection_total(subject: dict) -> float:
    """Bangumi collections: 想看 + 在看 + 看过 (docs/v2-series.md §4.2)."""
    c = subject.get("collection") or {}
    return float((c.get("wish") or 0) + (c.get("doing") or 0) + (c.get("collect") or 0))


def record_total(work: dict) -> float:
    """Annict: watch records over all episodes."""
    return float(sum(e.get("recordsCount") or 0 for e in work.get("episodes", [])))


def daily_increments(snaps: dict[date, dict], total) -> dict[date, float]:
    """Per day, the increase of a cumulative count since the previous snapshot, spread over the days between."""
    days = sorted(snaps)
    out = {}
    for a, b in zip(days, days[1:]):
        inc = max(0.0, total(snaps[b]) - total(snaps[a])) / (b - a).days
        for k in range((b - a).days):
            out[b - timedelta(days=k)] = inc
    return out


def tail_link(season: dict[date, float], inc: dict[date, float], min_days: int = 14) -> float | None:
    """The in-season ratio of the game's series to the daily increments, over the in-season days with both:
    the scale that puts post-finale increments in the series' units (docs/v2-series.md §4.2)."""
    both = [d for d in season if d in inc]
    if len(both) < min_days:
        return None
    a, b = sum(season[d] for d in both) / len(both), sum(inc[d] for d in both) / len(both)
    return a / b if b > 0 else None


def tail_ingest(series, smooth: int = 7) -> dict[str, dict[int, dict[date, float]]]:
    """The real post-finale cn/jp tail where the collector has daily snapshots: cn = Bangumi collection-count
    increments, jp = Annict record increments, each × its in-season link (`tail_link`), 7-day trailing mean.
    {market: {AniList id: {day: value}}} for the days after each show's last episode week."""
    m = matches()
    maps = {"cn": ("bgm", "subject", collection_total, {r["bgm"]: int(k) for k, r in m.items() if "bgm" in r}),
            "jp": ("annict", "work", record_total, {r["annict"]: int(k) for k, r in m.items() if "annict" in r})}
    out: dict[str, dict[int, dict[date, float]]] = {"cn": {}, "jp": {}}
    for mk, (src, kind, total, idmap) in maps.items():
        for ext, snaps in snapshot_history(src, kind).items():
            aid = idmap.get(ext)
            eps = series[mk].get(aid) if aid else None
            if not eps or len(snaps) < 2:
                continue
            inc = daily_increments(snaps, total)
            season: dict[date, float] = {}
            for (t, c, _), nxt in zip(eps, eps[1:] + [None]):
                end = min(t.date() + timedelta(days=7), nxt[0].date()) if nxt else t.date() + timedelta(days=7)
                for k in range((end - t.date()).days):
                    season[t.date() + timedelta(days=k)] = c / 7
            link = tail_link(season, inc)
            if link is None:
                continue
            after = max(season) + timedelta(days=1)
            vals = {}
            for d in sorted(inc):
                if d < after:
                    continue
                w = [inc[d - timedelta(days=k)] for k in range(smooth) if d - timedelta(days=k) in inc]
                vals[d] = link * sum(w) / len(w)
            out[mk][aid] = vals
    return out


def cmd_build() -> None:
    """Daily attention per show for the China and Japan markets -> data/derived/markets.json.

    Each episode's matured count is spread evenly over its air week [air, air+6]. Episodes younger
    than 3 days are left out and the previous week is carried forward (so these markets lag by up to
    a week). Once a show has finished, the days after its last week are missing (no observation), or with
    ANISTOCK_TAIL_INGEST=1 the real tail from the collector's daily snapshots (`tail_ingest`).
    """
    import tomllib
    p = tomllib.loads((ROOT / "sim" / "params.toml").read_text(encoding="utf-8"))["valuation"]
    profile, tail = p["post_finale_profile"], p["post_finale_tail_decay"]
    sys.path.insert(0, str(ROOT))
    from engine import valuation as V
    ff = Path(os.environ.get("ANISTOCK_FILTER_FILE", CALIBRATION / "valuation_filter.json"))
    fp = json.loads(ff.read_text(encoding="utf-8")) if ff.exists() else None
    series = episode_series(min_age=3.0)
    ingest = tail_ingest(series) if os.environ.get("ANISTOCK_TAIL_INGEST") == "1" else None
    fetched = {**{k: v for k, v in _fetched_at("bgm", "episodes").items()},
               **{k: v for k, v in _fetched_at("annict", "work").items()}}
    snap_day = max(fetched.values()).date()
    out: dict[str, dict[str, dict[str, float]]] = defaultdict(dict)
    stats = {"jp": {"shows": 0, "observed_days": 0, "extended_days": 0},
             "cn": {"shows": 0, "observed_days": 0, "extended_days": 0}}
    for mk, by_show in series.items():
        for aid, eps in by_show.items():
            if not eps:
                continue
            stats[mk]["shows"] += 1
            daily: dict[date, float] = {}
            for (t, c, age), nxt in zip(eps, eps[1:] + [None]):
                start = t.date()
                # an episode covers its own week, or up to the next episode if that comes sooner
                end = min(start + timedelta(days=7), nxt[0].date()) if nxt else start + timedelta(days=7)
                for k in range((end - start).days):
                    daily[start + timedelta(days=k)] = c / 7
            last_t, last_c, last_age = eps[-1]
            weekly = last_c / 7
            d = max(daily) + timedelta(days=1)
            if last_age < 10:
                # still airing: newest episode too young to count, hold the last full week flat
                while d <= snap_day:
                    daily[d] = weekly
                    d += timedelta(days=1)
            else:
                # finished. The rebuilt series has no post-finale data (an episode's count belongs to its air week).
                # With the fitted long tail (docs/v2-series.md §4) those days are left MISSING: the filter infers the
                # level from the other markets and the close books the filter's expectation as revenue (a stopgap,
                # vclose.rs `revenue_att`). With ANISTOCK_TAIL_INGEST=1 the real tail is used where the collector
                # has daily snapshots (`tail_ingest`: Bangumi collection-count increments, Annict record increments,
                # linked to the in-season series); off by default until the link is calibrated (sim/fit_tail_link.py).
                # Without the tail fit (older parameters): the v1 profile extension, as before.
                if fp and V.tail_of(fp, mk) is not None:
                    real = ingest.get(mk, {}).get(aid, {}) if ingest else {}
                    for day, v in sorted(real.items()):
                        if day >= d and day <= snap_day:
                            daily[day] = v
                            stats[mk]["extended_days"] += 1
                else:
                    k = 0
                    while d <= snap_day:
                        w = k // 7
                        f = profile[w] if w < len(profile) else profile[-1] * tail ** (w - len(profile) + 1)
                        daily[d] = weekly * f
                        stats[mk]["extended_days"] += 1
                        d += timedelta(days=1)
                        k += 1
            stats[mk]["observed_days"] += len(daily)
            for day, v in daily.items():
                out[str(aid)].setdefault(day.isoformat(), {})[mk] = round(v, 3)
    DERIVED_OUT.mkdir(parents=True, exist_ok=True)
    (DERIVED_OUT / "markets.json").write_text(json.dumps({"snapshot_day": snap_day.isoformat(), "stats": stats,
                                                      "series": out}), encoding="utf-8")
    print(f"snapshot {snap_day}: " + "  ".join(f"{mk} shows={v['shows']} days={v['observed_days']} "
                                                f"(extended {v['extended_days']})" for mk, v in stats.items()))


if __name__ == "__main__":
    load_dotenv()
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    {"airtimes": cmd_airtimes, "calibrate": cmd_calibrate, "check": cmd_check, "build": cmd_build}.get(cmd, lambda: sys.exit(__doc__))()
