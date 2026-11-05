"""体感 replay report: one season of the v0.3.0 market, show by show, for an anime fan to judge.

    .venv/bin/python sim/replay_report.py RUN_DIR INPUTS_DIR COLLECTOR_DB OUT.html [--start-day 2026-07-15]

RUN_DIR is a marketsim output directory (series.json, events.json, metrics.json; run it with `--bots none`).
INPUTS_DIR: the daily inputs the run used (attention per market, episodes, pre-air estimates). COLLECTOR_DB: a
collector database (read-only) for the Bangumi score / rank / collections / episode comments and the AniList
popularity of each show (its latest snapshot). Writes one self-contained HTML file (inline SVG, no scripts,
light/dark). Every sentence in it is computed from these files; nothing is typed in by hand.
"""

from __future__ import annotations

import argparse
import html
import json
import math
import sqlite3
import statistics as st
from collections import defaultdict
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MARKETS = ("us", "cn", "jp")
MARKET_NAME = {"us": "欧美 AniList", "cn": "中国 Bangumi", "jp": "日本 Annict"}


def esc(s) -> str:
    return html.escape(str(s), quote=True)


def pct(x: float, d: int = 0, sign: bool = True) -> str:
    if x is None or (isinstance(x, float) and math.isnan(x)):
        return "–"
    return f"{x * 100:+.{d}f}%" if sign else f"{x * 100:.{d}f}%"


def yuan(cents: float) -> str:
    return f"{cents / 100:.2f}"


def md(day: str) -> str:
    return f"{int(day[5:7])}/{int(day[8:10])}"


# ---------------------------------------------------------------- data


def load(run: Path, inputs: Path, collector: Path):
    series = json.loads((run / "series.json").read_text(encoding="utf-8"))
    events = json.loads((run / "events.json").read_text(encoding="utf-8"))
    paths = {c: {"group": g, "rows": [{"day": r[0], "close": r[1], "fair": r[2], "div": r[3]} for r in rows]}
             for c, g, rows in series["paths"]}
    comp = {c["id"]: c for c in events["companies"]}
    news = defaultdict(list)
    for n in events["news"]:
        if n.get("company_id") is not None:
            news[n["company_id"]].append(n)
    # inputs: attention per market, aired count, pre-air estimate per show and day
    att = defaultdict(dict)
    for f in sorted(inputs.glob("*.json")):
        d = json.loads(f.read_text(encoding="utf-8"))
        for s in d["shows"]:
            att[s["id"]][d["day"]] = {"att": s["att"], "aired": s["aired"], "preair": s.get("preair") or s["expected"],
                                      "long": s["long_runner"], "first_air": s["first_air"],
                                      "episodes": s.get("episodes")}
    # collector: Bangumi and AniList
    m = json.loads((ROOT / "sim" / "matches.json").read_text(encoding="utf-8"))
    matches = dict(m.get("matches", {}))
    for k, v in m.get("manual", {}).items():
        if "bgm" in v:
            matches.setdefault(k, {})["bgm"] = v["bgm"]
    db = sqlite3.connect(f"file:{collector}?mode=ro", uri=True)
    subj, eps, media = {}, {}, {}
    for sid, data in db.execute("SELECT CAST(external_id AS INTEGER), data FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day"):
        subj[sid] = json.loads(data)
    for sid, data in db.execute("SELECT CAST(external_id AS INTEGER), data FROM snapshots WHERE source='bgm' AND kind='episodes' ORDER BY day"):
        eps[sid] = json.loads(data)
    for aid, data in db.execute("SELECT CAST(external_id AS INTEGER), data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day"):
        media[aid] = json.loads(data)
    db.close()
    info = {}
    for cid in comp:
        bgm = (matches.get(str(cid)) or {}).get("bgm")
        s = subj.get(bgm) or {}
        rating = s.get("rating") or {}
        coll = s.get("collection") or {}
        e = eps.get(bgm) or {}
        comments = sum((x.get("comment") or 0) for x in e.get("data", []) if x.get("type", 0) == 0
                       and "2026-07-01" <= (x.get("airdate") or "") <= "2026-11-03")
        info[cid] = {
            "bgm": bgm, "name_cn": s.get("name_cn") or s.get("name"),
            "score": rating.get("score") or None, "rank": rating.get("rank") or None,
            "collect_total": sum(coll.get(k, 0) for k in ("wish", "doing", "collect", "on_hold", "dropped")) or None,
            "doing": coll.get("doing"), "comments": comments or None,
            "popularity": (media.get(cid) or {}).get("popularity"),
        }
    return paths, comp, news, att, info


# ---------------------------------------------------------------- per show


def show_facts(cid, paths, comp, news, att, start_day, first_day=None):
    rows = paths[cid]["rows"]
    c = comp[cid]
    a = att.get(cid, {})
    days = [r["day"] for r in rows]
    # season start: the later of start_day and the first row (listing)
    i0 = next((i for i, r in enumerate(rows) if r["day"] >= start_day), None)
    if i0 is None:
        return None
    listed_late = c["listed_on"] > (first_day or days[0])
    p0 = rows[i0]["close"]
    p1 = rows[-1]["close"] if c["status"] == "listed" else 0
    divs = sum(r["div"] for r in rows[i0:])
    paid_out = 0.0
    for n in news.get(cid, []):
        if n["kind"] == "delisting" and n.get("body"):
            # 按每股净资产 X 币向股东返还现金
            import re
            mm = re.search(r"([0-9.]+) 币", n["body"])
            if mm:
                paid_out = float(mm.group(1)) * 100
    total = (p1 + divs + paid_out) / p0 - 1 if p0 > 0 else float("nan")
    # air days from the aired counts
    air = []
    prev = None
    for d in sorted(a):
        x = a[d]["aired"]
        if prev is not None and x > prev:
            air += [(d, k) for k in range(prev + 1, x + 1)]
        prev = x
    prem = next((d for d, k in air if k == 1), None)
    if prem is None and a:
        first = sorted(a)[0]
        if a[first]["aired"] == 0:
            prem = None
    fin = None
    eps_n = c.get("episodes")
    if eps_n and not c["long_runner"]:
        fin = next((d for d, k in air if k == eps_n), None)
    return {"rows": rows, "i0": i0, "p0": p0, "p1": p1, "divs": divs, "paid_out": paid_out, "total": total,
            "air": air, "premiere": prem, "finale": fin, "listed_late": listed_late}


def mean_att(a, m, d0, d1):
    v = [a[d]["att"][m] for d in a if d0 <= d <= d1 and a[d]["att"][m] > 0]
    return sum(v) / len(v) if v else 0.0


def preair_of(a, day):
    if not a:
        return {}
    ds = sorted(d for d in a if d <= day) or sorted(a)
    return a[ds[-1]]["preair"]


def day_add(d: str, n: int) -> str:
    return date.fromordinal(date.fromisoformat(d).toordinal() + n).isoformat()


def close_at(rows, day):
    """Close on `day` or the last trading day before it."""
    best = None
    for r in rows:
        if r["day"] <= day:
            best = r
    return best


def narrate(cid, f, comp, news, att, info) -> list[str]:
    """2-4 plain sentences, every number from the data."""
    c = comp[cid]
    rows = f["rows"]
    a = att.get(cid, {})
    out = []
    flashes = [n for n in news.get(cid, []) if n["kind"] == "flash"]
    # IPO
    if f["listed_late"]:
        lc = close_at(rows, c["listed_on"])
        if lc:
            out.append(f"{md(c['listed_on'])} 以发行价 {yuan(c['ipo_price'])} 币上市，首日收于 {yuan(lc['close'])} 币"
                       f"（{pct(lc['close'] / c['ipo_price'] - 1)}）。")
    # premiere reaction
    prem = f["premiere"]
    if prem and rows[0]["day"] < prem:
        before = close_at(rows, day_add(prem, -1))
        after = close_at(rows, day_add(prem, 3))
        pre = preair_of(a, day_add(prem, -1))
        cn_actual = mean_att(a, "cn", prem, day_add(prem, 13))
        s = f"{md(prem)} 首播"
        first_flash = next((n for n in flashes if prem <= n["day"] <= day_add(prem, 7)), None)
        if first_flash:
            s += "，" + first_flash["title"].replace("热度快讯：", "").replace(f"《{c['title']}》", "")
        if pre.get("cn") and cn_actual:
            s += f"；开播两周 Bangumi 热度是开播前预估的 {cn_actual / pre['cn'] * 100:.0f}%"
        if before and after and before["close"] > 0:
            s += f"，首播后三天股价 {pct(after['close'] / before['close'] - 1)}"
        out.append(s + "。")
    # biggest flash after the first week
    later = [n for n in flashes if not prem or n["day"] > day_add(prem, 7)]
    if later:
        import re

        def mag(n):
            mm = re.search(r"([高低])于预期 (\d+)%", n["title"])
            return int(mm.group(2)) if mm else 0
        big = max(later, key=mag)
        b = close_at(rows, day_add(big["day"], -1))
        e3 = close_at(rows, day_add(big["day"], 3))
        s = f"{md(big['day'])} " + big["title"].replace("热度快讯：", "").replace(f"《{c['title']}》", "")
        if b and e3 and b["close"] > 0:
            s += f"，三天内股价 {pct(e3['close'] / b['close'] - 1)}"
        out.append(s + "。")
    # peak and the path since
    seg = rows[f["i0"]:]
    if len(seg) > 10:
        pk = max(seg, key=lambda r: r["close"])
        ep_at = max([k for d, k in f["air"] if d <= pk["day"]], default=0)
        tail = seg[-1]
        if pk is not seg[0] and pk is not tail and tail["close"] < pk["close"] * 0.85:
            out.append(f"股价在 {md(pk['day'])}（第 {ep_at} 集后）见顶 {yuan(pk['close'])} 币，"
                       f"此后回落到 {yuan(tail['close'])} 币（{pct(tail['close'] / pk['close'] - 1)}）。")
    # dividends, finale, delisting
    if f["divs"] > 0 and f["p0"] > 0:
        share = f["divs"] / f["p0"]
        out.append(f"期间每股分红合计 {yuan(f['divs'])} 币（相当于起始价的 {share * 100:.0f}%），"
                   f"含分红的总收益 {pct(f['total'])}。")
    warn = [n for n in news.get(cid, []) if n["kind"] in ("warning", "delisting")]
    if warn:
        out.append("；".join(f"{md(n['day'])} {n['title']}" for n in warn[-2:]) + "。")
    return out


# ---------------------------------------------------------------- charts


def chart(cid, f, comp, news, att, width=760) -> str:
    rows = f["rows"]
    a = att.get(cid, {})
    days = [r["day"] for r in rows]
    n = len(days)
    if n < 2:
        return ""
    L, R, T = 48, 12, 22
    H1, gap, H2 = 200, 26, 70
    W = width - L - R
    x = lambda i: L + W * i / (n - 1)
    idx = {d: i for i, d in enumerate(days)}

    def xi(day):
        if day in idx:
            return x(idx[day])
        if day < days[0] or day > days[-1]:
            return None
        j = sum(1 for d in days if d <= day) - 1
        return x(max(0, j))
    vals = [r["close"] for r in rows if r["close"] > 0] + [r["fair"] for r in rows if r["fair"] > 0]
    c = comp[cid]
    if f["listed_late"]:
        vals.append(c["ipo_price"])
    lo, hi = min(vals), max(vals)
    lo, hi = lo * 0.92, hi * 1.06
    y = lambda v: T + H1 - H1 * (v - lo) / (hi - lo)
    out = [f'<svg viewBox="0 0 {width} {T + H1 + gap + H2 + 22}" role="img" aria-label="{esc(c["title"])} 价格与估值">']
    # grid and axis labels
    for k in range(5):
        v = lo + (hi - lo) * k / 4
        out.append(f'<line class="grid" x1="{L}" x2="{L + W}" y1="{y(v):.1f}" y2="{y(v):.1f}"/>'
                   f'<text class="ax" x="{L - 4}" y="{y(v) + 3:.1f}" text-anchor="end">{v / 100:.2f}</text>')
    months = sorted({d[:7] for d in days})
    for mth in months:
        d0 = next(d for d in days if d.startswith(mth))
        out.append(f'<text class="ax" x="{xi(d0):.1f}" y="{T + H1 + gap + H2 + 16}">{int(mth[5:])}月</text>')
    # premiere / finale lines
    for key, label in (("premiere", "首播"), ("finale", "完结")):
        d = f[key]
        if d and xi(d) is not None:
            out.append(f'<line class="mark" x1="{xi(d):.1f}" x2="{xi(d):.1f}" y1="{T}" y2="{T + H1 + gap + H2}"/>'
                       f'<text class="lab" x="{xi(d) + 3:.1f}" y="{T + 10}">{label}</text>')
    # listing / IPO price
    if f["listed_late"] and xi(c["listed_on"]) is not None:
        xl = xi(c["listed_on"])
        out.append(f'<line class="ipo" x1="{xl - 10:.1f}" x2="{xl + 30:.1f}" y1="{y(c["ipo_price"]):.1f}" y2="{y(c["ipo_price"]):.1f}"/>'
                   f'<text class="lab" x="{xl + 32:.1f}" y="{y(c["ipo_price"]) + 3:.1f}">发行价 {yuan(c["ipo_price"])}</text>')
    # value (dashed) and price
    fp = " ".join(f"{x(i):.1f},{y(r['fair']):.1f}" for i, r in enumerate(rows) if r["fair"] > 0)
    pp = " ".join(f"{x(i):.1f},{y(r['close']):.1f}" for i, r in enumerate(rows) if r["close"] > 0)
    out.append(f'<polyline class="fair" points="{fp}"><title>隐藏估值</title></polyline>')
    out.append(f'<polyline class="price" points="{pp}"><title>收盘价</title></polyline>')
    # episodes: ticks under the price panel
    for d, k in f["air"]:
        xx = xi(d)
        if xx is not None:
            out.append(f'<line class="ep" x1="{xx:.1f}" x2="{xx:.1f}" y1="{T + H1}" y2="{T + H1 + 6}"><title>第 {k} 集 {d}</title></line>')
    # dividends: small diamonds at the bottom of the price panel
    for i, r in enumerate(rows):
        if r["div"] > 0:
            xx, yy = x(i), T + H1 - 4
            out.append(f'<path class="div" d="M{xx:.1f},{yy - 3:.1f} l3,3 l-3,3 l-3,-3z"><title>{r["day"]} 除息 {yuan(r["div"])} 币/股</title></path>')
    # news markers on the price line
    by_day = {r["day"]: r for r in rows}
    for nw in news.get(cid, []):
        xx = xi(nw["day"])
        if xx is None:
            continue
        r = by_day.get(nw["day"]) or close_at(rows, nw["day"])
        yy = y(r["close"]) if r and r["close"] > 0 else T + H1 / 2
        t = esc(f"{nw['day']} {nw['title']}")
        if nw["kind"] == "flash":
            up = "高于" in nw["title"]
            pth = f"M{xx:.1f},{T + 2:.1f} l5,9 l-10,0z" if up else f"M{xx:.1f},{T + 11:.1f} l5,-9 l-10,0z"
            out.append(f'<path class="{"fup" if up else "fdn"}" d="{pth}"><title>{t}</title></path>')
        elif nw["kind"] == "vcm":
            out.append(f'<path class="vcm" d="M{xx - 3:.1f},{yy - 3:.1f} l6,6 m0,-6 l-6,6"><title>{t}</title></path>')
        elif nw["kind"] in ("warning", "delisting"):
            out.append(f'<circle class="warn" cx="{xx:.1f}" cy="{yy:.1f}" r="4"><title>{t}</title></circle>')
    # attention panel
    top2 = T + H1 + gap
    out.append(f'<text class="ax" x="{L}" y="{top2 - 6}">三个市场的热度（各自除以本图期间的最大值）</text>')
    for m, cls in zip(MARKETS, ("a1", "a2", "a3")):
        vs = [(a.get(d) or {}).get("att", {}).get(m, 0.0) for d in days]
        mx = max(vs) if vs else 0
        if mx <= 0:
            continue
        pts = " ".join(f"{x(i):.1f},{top2 + H2 - H2 * v / mx:.1f}" for i, v in enumerate(vs))
        out.append(f'<polyline class="{cls}" points="{pts}"><title>{MARKET_NAME[m]}</title></polyline>')
    out.append(f'<line class="axis" x1="{L}" x2="{L + W}" y1="{top2 + H2}" y2="{top2 + H2}"/>')
    out.append("</svg>")
    return "".join(out)


def legend() -> str:
    return ('<p class="legend"><span class="k price"></span>收盘价 <span class="k fair"></span>隐藏估值（虚线） '
            '<span class="sym fup">▲</span><span class="sym fdn">▼</span>热度快讯（高于/低于预期） '
            '<span class="sym div">◆</span>除息 <span class="sym vcm">×</span>冷静期 <span class="sym warn">●</span>退市警示/退市 '
            '<span class="sym ep">|</span>每集播出 · 下方：<span class="k a1"></span>欧美 <span class="k a2"></span>中国 '
            '<span class="k a3"></span>日本。鼠标悬停看详情。</p>')


def scatter(points, xlabel, ylabel, width=560, height=300, logx=False) -> str:
    L, R, T, B = 52, 14, 14, 36
    W, H = width - L - R, height - T - B
    xs = [math.log10(p[0]) if logx else p[0] for p in points]
    ys = [p[1] for p in points]
    x0, x1 = min(xs), max(xs)
    y0, y1 = min(ys + [0]), max(ys + [0])
    px = lambda v: L + W * (v - x0) / ((x1 - x0) or 1)
    py = lambda v: T + H - H * (v - y0) / ((y1 - y0) or 1)
    out = [f'<svg viewBox="0 0 {width} {height}" role="img" aria-label="{esc(ylabel)} 对 {esc(xlabel)}">']
    out.append(f'<line class="grid" x1="{L}" x2="{L + W}" y1="{py(0):.1f}" y2="{py(0):.1f}"/>')
    for k in range(5):
        v = y0 + (y1 - y0) * k / 4
        out.append(f'<text class="ax" x="{L - 4}" y="{py(v) + 3:.1f}" text-anchor="end">{v * 100:.0f}%</text>')
        u = x0 + (x1 - x0) * k / 4
        lab = f"{10 ** u:.0f}" if logx else f"{u:.1f}"
        out.append(f'<text class="ax" x="{px(u):.1f}" y="{T + H + 14}" text-anchor="middle">{lab}</text>')
    out.append(f'<text class="ax" x="{L + W / 2:.1f}" y="{height - 4}" text-anchor="middle">{esc(xlabel)}</text>')
    for (xv, yv, title), u in zip(points, xs):
        out.append(f'<circle class="pt" cx="{px(u):.1f}" cy="{py(yv):.1f}" r="3.5"><title>{esc(title)}：{yv * 100:+.0f}%</title></circle>')
    out.append("</svg>")
    return "".join(out)


def pearson(a, b):
    if len(a) < 3:
        return float("nan")
    ma, mb = st.mean(a), st.mean(b)
    va = sum((x - ma) ** 2 for x in a)
    vb = sum((y - mb) ** 2 for y in b)
    return sum((x - ma) * (y - mb) for x, y in zip(a, b)) / math.sqrt(va * vb) if va and vb else float("nan")


def ranks(v):
    o = sorted(range(len(v)), key=lambda i: v[i])
    r = [0.0] * len(v)
    for k, i in enumerate(o):
        r[i] = k
    return r


def spearman(a, b):
    return pearson(ranks(a), ranks(b))


# ---------------------------------------------------------------- main


CSS = """
:root{--bg:#fbfaf7;--fg:#1d1d1f;--mut:#6b6b70;--line:#e3e1db;--card:#fff;--price:#1f5fbf;--fair:#8a8a90;--up:#c2410c;--dn:#2563eb;
--a1:#7c3aed;--a2:#dc2626;--a3:#059669;--div:#b45309;--vcm:#d97706;--warn:#b91c1c;--ok:#2d7d46;--bad:#b3261e}
@media (prefers-color-scheme: dark){:root:not([data-theme="light"]){--bg:#151517;--fg:#ececee;--mut:#9a9aa1;--line:#2c2c30;--card:#1d1d20;
--price:#6ea8ff;--fair:#9a9aa1;--up:#fb923c;--dn:#60a5fa;--a1:#a78bfa;--a2:#f87171;--a3:#34d399;--div:#f59e0b;--vcm:#fbbf24;--warn:#f87171;--ok:#6fcf97;--bad:#ff6b6b}}
:root[data-theme="dark"]{--bg:#151517;--fg:#ececee;--mut:#9a9aa1;--line:#2c2c30;--card:#1d1d20;--price:#6ea8ff;--fair:#9a9aa1;--up:#fb923c;--dn:#60a5fa;
--a1:#a78bfa;--a2:#f87171;--a3:#34d399;--div:#f59e0b;--vcm:#fbbf24;--warn:#f87171;--ok:#6fcf97;--bad:#ff6b6b}
body{background:var(--bg);color:var(--fg);font:15px/1.7 system-ui,-apple-system,"PingFang SC","Noto Sans CJK SC","Microsoft YaHei",sans-serif;margin:0;padding:24px 16px}
main{max-width:860px;margin:0 auto} h1{font-size:24px;margin:0 0 6px} h2{font-size:19px;margin:36px 0 10px;border-bottom:1px solid var(--line);padding-bottom:4px}
h3{font-size:16px;margin:0 0 4px} .mut{color:var(--mut)} .card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:14px 14px 8px;margin:16px 0}
svg{width:100%;height:auto;display:block} .grid{stroke:var(--line)} .axis{stroke:var(--mut)} .ax{fill:var(--mut);font-size:10.5px} .lab{fill:var(--mut);font-size:10.5px}
.mark{stroke:var(--mut);stroke-dasharray:3 3} .ipo{stroke:var(--up);stroke-width:2} .price{fill:none;stroke:var(--price);stroke-width:1.8}
.fair{fill:none;stroke:var(--fair);stroke-width:1.4;stroke-dasharray:5 3} .ep{stroke:var(--mut)} .div{fill:var(--div)} .fup{fill:var(--up)} .fdn{fill:var(--dn)}
.vcm{stroke:var(--vcm);stroke-width:1.6} .warn{fill:var(--warn)} .a1{fill:none;stroke:var(--a1);stroke-width:1.3} .a2{fill:none;stroke:var(--a2);stroke-width:1.3}
.a3{fill:none;stroke:var(--a3);stroke-width:1.3} .pt{fill:var(--price);opacity:.75}
table{border-collapse:collapse;width:100%;font-size:13px;font-variant-numeric:tabular-nums} th,td{text-align:left;padding:5px 7px;border-bottom:1px solid var(--line)} th{color:var(--mut);font-weight:600}
.wrap{overflow-x:auto} .legend{font-size:12.5px;color:var(--mut)} .k{display:inline-block;width:16px;height:0;border-top:2px solid;vertical-align:middle;margin:0 3px 0 8px}
.k.price{border-color:var(--price)} .k.fair{border-color:var(--fair);border-top-style:dashed} .k.a1{border-color:var(--a1)} .k.a2{border-color:var(--a2)} .k.a3{border-color:var(--a3)}
.sym{margin:0 2px 0 8px} .sym.fup{color:var(--up)} .sym.fdn{color:var(--dn)} .sym.div{color:var(--div)} .sym.vcm{color:var(--vcm)} .sym.warn{color:var(--warn)} .sym.ep{color:var(--mut)}
.facts{font-size:12.5px;color:var(--mut);margin:2px 0 6px} .ok{color:var(--ok);font-weight:600} .bad{color:var(--bad);font-weight:600}
.grid2{display:grid;grid-template-columns:1fr;gap:12px} @media(min-width:760px){.grid2{grid-template-columns:1fr 1fr}}
"""


def unexplained_jumps(cid, f, news, att, thresh=0.35):
    """Single-day moves of the hidden value above `thresh` with no episode, no flash and no ex-dividend that day,
    with what changed in the data: [(day, change, markets that first appeared / updated / none)]."""
    rows = f["rows"][f["i0"]:]
    a_ = att.get(cid, {})
    ad = sorted(a_)
    flash_days = {n["day"] for n in news.get(cid, []) if n["kind"] == "flash"}
    air_days = {d for d, _ in f["air"]}
    out = []
    for r0, r1 in zip(rows, rows[1:]):
        if r0["fair"] <= 0 or r1["fair"] <= 0:
            continue
        ch = (r1["fair"] + r0["div"]) / r0["fair"] - 1
        d = r0["day"]
        if abs(ch) <= thresh or d in air_days or d in flash_days or r0["div"] > 0:
            continue
        cause = "none"
        ms = []
        if d in a_:
            k = ad.index(d)
            prev = a_[ad[k - 1]]["att"] if k else {m: 0.0 for m in MARKETS}
            now = a_[d]["att"]
            first = [m for m in ("cn", "jp") if prev[m] <= 0 < now[m]]
            weekly = [m for m in ("cn", "jp") if prev[m] > 0 and now[m] > 0 and now[m] != prev[m]]
            lost = [m for m in ("cn", "jp") if prev[m] > 0 >= now[m]]
            if first:
                late = f["premiere"] and (date.fromisoformat(d) - date.fromisoformat(f["premiere"])).days > 7
                cause, ms = ("first_late" if late else "first"), first
            elif weekly:
                cause, ms = "weekly", weekly
            elif lost:
                cause, ms = "lost", lost
        out.append((d, ch, cause, ms))
    return out


def summary(run: Path, inputs: Path, collector: Path, start_day: str) -> dict:
    """The before/after numbers of one run: the September IPO cohort, unexplained value jumps, correlations."""
    paths, comp, news, att, info = load(run, inputs, collector)
    first_day = min(r["day"] for p in paths.values() for r in p["rows"])
    facts = {cid: show_facts(cid, paths, comp, news, att, start_day, first_day) for cid in paths if cid in comp}
    facts = {k: v for k, v in facts.items() if v}
    cohort = []
    for cid, f in facts.items():
        c = comp[cid]
        if not ("2026-09-01" <= c["listed_on"] <= "2026-09-30") or not f["listed_late"]:
            continue
        lc = close_at(f["rows"], c["listed_on"])
        if not lc or lc["close"] <= 0:
            continue
        end = f["rows"][-1]
        p1 = end["close"] if c["status"] == "listed" else 0
        divs = sum(r["div"] for r in f["rows"] if r["day"] >= c["listed_on"])
        fair_l = lc["fair"]
        cohort.append({"id": cid, "from_listing": (p1 + divs) / lc["close"] - 1, "from_ipo": (p1 + divs) / c["ipo_price"] - 1,
                       "first_day": lc["close"] / c["ipo_price"] - 1,
                       "discount": 1 - c["ipo_price"] / fair_l if fair_l > 0 else None})
    # the cohort's log move from listing to premiere+14 (hidden value, dividends added back), and the
    # hidden value's mean daily log revision by days to the premiere
    cohort_log = []
    buckets = {"21-15": [], "14-8": [], "7-1": []}
    for x in cohort:
        c = comp[x["id"]]
        f = facts[x["id"]]
        rows = f["rows"]
        a0 = close_at(rows, c["listed_on"])
        p14 = close_at(rows, day_add(c["first_air"], 14))
        if a0 and p14 and a0["fair"] > 0 and p14["fair"] > 0:
            dv = sum(r["div"] for r in rows if a0["day"] <= r["day"] < p14["day"])
            cohort_log.append(math.log((p14["fair"] + dv) / a0["fair"]))
        for r0, r1 in zip(rows, rows[1:]):
            k = (date.fromisoformat(c["first_air"]) - date.fromisoformat(r0["day"])).days
            if r0["fair"] > 0 and r1["fair"] > 0 and 1 <= k <= 21:
                b = "21-15" if k >= 15 else ("14-8" if k >= 8 else "7-1")
                buckets[b].append(math.log((r1["fair"] + r0["div"]) / r0["fair"]))
    jumps = []
    for cid, f in facts.items():
        for d, ch, cause, ms in unexplained_jumps(cid, f, news, att):
            jumps.append({"id": cid, "title": comp[cid]["title"], "day": d, "change": ch, "cause": cause, "markets": ms})
    med = lambda v: st.median(v) if v else float("nan")
    return {
        "cohort_n": len(cohort),
        "cohort_from_listing": med([x["from_listing"] for x in cohort]),
        "cohort_from_ipo": med([x["from_ipo"] for x in cohort]),
        "cohort_first_day": med([x["first_day"] for x in cohort]),
        "cohort_discount": med([x["discount"] for x in cohort if x["discount"] is not None]),
        "jumps": sorted(jumps, key=lambda j: -abs(j["change"])),
        "cohort_log_to_p14": med(cohort_log),
        "cohort_log_to_p14_mean": (sum(cohort_log) / len(cohort_log)) if cohort_log else float("nan"),
        "prepremiere_buckets": {k: ((sum(v) / len(v)) if v else float("nan"), len(v), med(v)) for k, v in buckets.items()},
        "metrics": json.loads((run / "metrics.json").read_text(encoding="utf-8")),
    }


def before_after(before: dict, after: dict) -> str:
    def row(name, a, b, fmt):
        return f"<tr><td>{esc(name)}</td><td>{fmt(a)}</td><td>{fmt(b)}</td></tr>"
    p_ = lambda x: pct(x)
    h = ["<h2>v0.3.0 → v0.3.2（修正前后）</h2>",
         "<p class=mut>同一份数据、同一个随机种子、同一组作品。v0.3.2 改了估值模型：开播前的预估只用当时已有的数据（中国、日本两个市场按欧美的预估和拟合出的比例推算），"
         "三个市场合成一个联合滤波（一个市场超预期，会按相关性带动另外两个市场的预期；某个市场暂时没有数据时，用其他市场推断，不再当成零）；"
         "开播前的预估计入它到开播前一天的预期增长；开播前不再按“热度衰减”折算；市场总热度按每部番自己的预期走势相加，再加上还没上市的番的预期。</p>",
         "<div class=wrap><table><tr><th>指标</th><th>v0.3.0</th><th>v0.3.2</th></tr>"]
    h.append(row(f"9 月上市新股：上市首日到 {md('2026-11-03')} 的收益中位数（含分红）", before["cohort_from_listing"], after["cohort_from_listing"], p_))
    h.append(row("　从发行价算", before["cohort_from_ipo"], after["cohort_from_ipo"], p_))
    h.append(row("　上市首日相对发行价", before["cohort_first_day"], after["cohort_first_day"], p_))
    h.append(row("　发行折价（发行价相对上市时估值）中位数", before["cohort_discount"], after["cohort_discount"], p_))
    lg = lambda x: f"{x:+.3f}" if x == x else "–"
    h.append(row("　上市到首播后两周：估值对数变动的中位数（分红加回）", before["cohort_log_to_p14"], after["cohort_log_to_p14"], lg))
    h.append(row("　　平均", before["cohort_log_to_p14_mean"], after["cohort_log_to_p14_mean"], lg))
    for k, lab in (("21-15", "开播前 21–15 天：估值日均对数修正"), ("14-8", "开播前 14–8 天"), ("7-1", "开播前 7–1 天")):
        bb, aa = before["prepremiere_buckets"][k], after["prepremiere_buckets"][k]
        h.append(f"<tr><td>{esc(lab)}</td><td>{bb[0] * 100:+.2f}%（中位数 {bb[2] * 100:+.2f}%，{bb[1]}）</td>"
                 f"<td>{aa[0] * 100:+.2f}%（中位数 {aa[2] * 100:+.2f}%，{aa[1]}）</td></tr>")
    nb = len(before["jumps"])
    na = len(after["jumps"])
    h.append(row("没有新集、快讯、除息却单日估值变动超过 35% 的次数", nb, na, str))
    for k, lab in (("first", "　其中：首播后 7 天内某市场数据第一次出现（信息事件）"), ("first_late", "　其中：首播 7 天以后数据才第一次出现（缺陷）"),
                   ("weekly", "　其中：中国/日本周数据更新"), ("lost", "　其中：某市场数据中断"), ("none", "　其中：数据没有变化")):
        h.append(row(lab, sum(1 for j in before["jumps"] if j["cause"] == k), sum(1 for j in after["jumps"] if j["cause"] == k), str))
    gb, ga = before["metrics"]["groups"]["all"], after["metrics"]["groups"]["all"]
    h.append(row("日波动中位数（价格）", gb["daily_sigma_median"], ga["daily_sigma_median"], lambda x: pct(x, 1, False)))
    rb, ra = before["metrics"]["revisions"], after["metrics"]["revisions"]
    h.append(row("估值修正：滞后 1 自相关", rb["lag1_corr"], ra["lag1_corr"], lambda x: f"{x:+.3f}"))
    h.append(row("估值修正：首播后两周的播出日平均", rb.get("air_first2w_simple_mean"), ra.get("air_first2w_simple_mean"), lambda x: pct(x, 1) if x is not None else "–"))
    h.append("</table></div>")
    if after["jumps"]:
        names = {"first": "首播后 7 天内数据第一次出现", "first_late": "首播 7 天后数据才第一次出现", "weekly": "周数据更新",
                 "lost": "数据中断", "none": "数据没有变化"}
        h.append("<p class=mut>v0.3.2 仍然存在的单日大幅变动（首播后 7 天内数据第一次出现属于信息事件，不算在内）：" + esc("；".join(
            f"《{j['title']}》{md(j['day'])} {pct(j['change'])}（{names[j['cause']]}{'：' + '、'.join(MARKET_NAME[m] for m in j['markets']) if j['markets'] else ''}）"
            for j in [j for j in after["jumps"] if j["cause"] != "first"][:12])) + "</p>")
    return "".join(h)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run", type=Path)
    ap.add_argument("inputs", type=Path)
    ap.add_argument("collector", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--selection", type=Path, help="JSON file with the shows to chart: read when it exists "
                                                   "(the same shows as an earlier report), else written")
    ap.add_argument("--before", nargs=2, type=Path, metavar=("RUN_DIR", "INPUTS_DIR"),
                    help="an earlier model's run of the same data: adds a before/after section at the top")
    ap.add_argument("--start-day", default="2026-07-15",
                    help="season returns start here (the first week is the market repricing from v1 prices)")
    a = ap.parse_args()
    paths, comp, news, att, info = load(a.run, a.inputs, a.collector)
    metrics = json.loads((a.run / "metrics.json").read_text(encoding="utf-8"))
    days_all = sorted({r["day"] for p in paths.values() for r in p["rows"]})
    facts = {cid: show_facts(cid, paths, comp, news, att, a.start_day, days_all[0]) for cid in paths if cid in comp}
    facts = {k: v for k, v in facts.items() if v}

    # ---- selection
    def summer(cid):
        f = facts[cid]
        return not comp[cid]["long_runner"] and f["premiere"] and "2026-06-25" <= f["premiere"] <= "2026-08-15"
    summer_ids = [c for c in facts if summer(c)]
    by_coll = sorted((c for c in summer_ids if info[c]["collect_total"]), key=lambda c: -info[c]["collect_total"])
    picked = by_coll[:6]
    longs = [c for c in (21, 235) if c in facts]
    # expectation vs reality in the Chinese market: first four weeks' Bangumi attention / pre-air estimate
    ratio = {}
    for c in summer_ids:
        f = facts[c]
        pre = preair_of(att.get(c, {}), day_add(f["premiere"], -1)).get("cn") or 0
        act = mean_att(att.get(c, {}), "cn", f["premiere"], day_add(f["premiere"], 27))
        if pre > 0 and act > 0:
            ratio[c] = (act / pre, pre)
    pres = sorted(ratio, key=lambda c: -ratio[c][1])
    top_pre = pres[:30]
    flops = [c for c in sorted(top_pre, key=lambda c: ratio[c][0]) if c not in picked][:3]
    # (excluding the shows with almost no pre-air signal: ranks 31-90 by the estimate, 1,500+ collections)
    rest = [c for c in pres[30:90] if c not in picked and (info[c]["collect_total"] or 0) >= 1500]
    hits = [c for c in sorted(rest, key=lambda c: -ratio[c][0]) if c not in flops][:3]
    groups = [("本季最受关注（Bangumi 收藏数前 6）", picked), ("长篇", longs), ("落差：开播前期待高、实际热度低", flops),
              ("黑马：开播前期待一般、实际热度高", hits)]
    if a.selection and a.selection.exists():
        groups = [(g, [c for c in ids if c in facts]) for g, ids in json.loads(a.selection.read_text(encoding="utf-8"))]
    elif a.selection:
        a.selection.write_text(json.dumps(groups, ensure_ascii=False), encoding="utf-8")

    h = [f'<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
         f'<title>上季回放</title><style>{CSS}</style></head><body><main>']
    h.append("<h1>上季回放：2026 年夏季番的股价体感</h1>")
    h.append(f'<p class="mut">用 v0.3.0 的模型（估值、NPC 市场、热度快讯、规则）把 {md(days_all[0])}–{md(days_all[-1])} 的真实数据完整回放一遍（一个随机种子）。'
             f'玩家不参与交易，价格完全由 NPC 和做市商形成。下文所有数字和叙述都由回放数据自动生成。季度收益从 {md(a.start_day)} 算起'
             '（第一周是开服时从旧价格调整到新估值的过程，不算在内），之后上市的从上市首日收盘价算起，都包含分红。</p>')
    h.append('<p class="mut">选取规则：7 月前后首播的新番里按 Bangumi 收藏人数取前 6 部；两部长篇（航海王、名侦探柯南）；开播前预估热度前 30 部里，'
             '开播后四周 Bangumi 实际热度与预估之比最低的 3 部（落差）；预估排第 31–90、收藏 1,500 以上的作品里这一比值最高的 3 部（黑马）。</p>')
    if a.before:
        h.append(before_after(summary(a.before[0], a.before[1], a.collector, a.start_day),
                              summary(a.run, a.inputs, a.collector, a.start_day)))
    h.append(legend())
    for gname, ids in groups:
        h.append(f"<h2>{esc(gname)}</h2>")
        for cid in ids:
            f = facts[cid]
            c = comp[cid]
            inf = info[cid]
            meta = []
            if inf["score"]:
                meta.append(f"Bangumi {inf['score']} 分" + (f"（第 {inf['rank']} 名）" if inf["rank"] else ""))
            if inf["collect_total"]:
                meta.append(f"收藏 {inf['collect_total']:,}")
            if inf["comments"]:
                meta.append(f"本季单集吐槽 {inf['comments']:,}")
            if inf["popularity"]:
                meta.append(f"AniList 人气 {inf['popularity']:,}")
            if cid in ratio:
                meta.append(f"开播四周中国热度 / 预估 = {ratio[cid][0] * 100:.0f}%")
            meta.append(f"季度总收益 {pct(f['total'])}")
            h.append(f'<div class="card"><h3>{esc(c["title"])}</h3><p class="facts">{esc(" · ".join(meta))}</p>')
            h.append(chart(cid, f, comp, news, att))
            h.append("<p>" + esc("".join(narrate(cid, f, comp, news, att, info))) + "</p></div>")

    # ---- season tables: this season (premiered before mid-September, and the long-runners); the autumn
    # shows listed in September are a cohort of their own (their "season" has only just begun)
    def autumn(cid):
        f = facts[cid]
        pa = f["premiere"] or comp[cid]["first_air"]
        return not comp[cid]["long_runner"] and pa >= "2026-09-15"
    table, fall = [], []
    for cid, f in facts.items():
        rows = f["rows"]
        if len(rows) - f["i0"] < 30 or math.isnan(f["total"]):
            continue
        (fall if autumn(cid) else table).append((cid, f["total"]))
    table.sort(key=lambda x: -x[1])
    fall.sort(key=lambda x: -x[1])

    def num(v):
        return f"{v:,}" if isinstance(v, (int, float)) and v else "–"

    def trow(cid, ret):
        inf = info[cid]
        return (f"<tr><td>{esc(comp[cid]['title'])}</td><td>{pct(ret)}</td><td>{inf['score'] or '–'}</td><td>{inf['rank'] or '–'}</td>"
                f"<td>{num(inf['collect_total'])}</td><td>{num(inf['comments'])}</td><td>{num(inf['popularity'])}</td></tr>")
    head = "<tr><th>作品</th><th>季度总收益</th><th>Bangumi 评分</th><th>排名</th><th>收藏</th><th>本季吐槽</th><th>AniList 人气</th></tr>"
    h.append(f"<h2>季度收益排行（夏季及之前开播、交易满 30 天的 {len(table)} 支股票）</h2>")
    h.append("<h3>前 10</h3><div class=wrap><table>" + head + "".join(trow(c, r) for c, r in table[:10]) + "</table></div>")
    h.append("<h3 style='margin-top:14px'>后 10</h3><div class=wrap><table>" + head + "".join(trow(c, r) for c, r in table[-10:][::-1]) + "</table></div>")

    # ---- scatter
    sc = [(info[c]["score"], r, comp[c]["title"]) for c, r in table if info[c]["score"]]
    dc = [(info[c]["comments"], r, comp[c]["title"]) for c, r in table if info[c]["comments"]]
    h.append("<h2>收益和口碑、讨论量的关系（同上 " + str(len(table)) + " 支）</h2><div class=grid2>")
    if len(sc) >= 3:
        h.append(f"<div><p class=facts>季度总收益 vs Bangumi 评分（{len(sc)} 部）：Pearson {pearson([p[0] for p in sc], [p[1] for p in sc]):+.2f}，"
                 f"Spearman {spearman([p[0] for p in sc], [p[1] for p in sc]):+.2f}</p>" + scatter(sc, "Bangumi 评分", "季度总收益") + "</div>")
    if len(dc) >= 3:
        h.append(f"<div><p class=facts>季度总收益 vs 本季单集吐槽数（对数，{len(dc)} 部）：Pearson {pearson([math.log10(p[0]) for p in dc], [p[1] for p in dc]):+.2f}，"
                 f"Spearman {spearman([p[0] for p in dc], [p[1] for p in dc]):+.2f}</p>" + scatter(dc, "本季单集吐槽数（对数刻度）", "季度总收益", logx=True) + "</div>")
    h.append("</div><p class=mut>估值看的是一部番在三个市场的热度份额，不是评分：评分高但观众少的作品分红也少。收益衡量的是“比开季时市场的预期更好还是更差”，"
             "所以和评分、讨论量的绝对水平未必相关。</p>")

    # ---- surprises
    h.append("<h2>可能违背体感的地方</h2>")
    items = []
    colls = sorted([info[c]["collect_total"] for c, _ in table if info[c]["collect_total"]])
    med_coll = colls[len(colls) // 2] if colls else 0
    top_coll = colls[int(len(colls) * 0.8)] if colls else 1e18

    def cn_ratio(cid, weeks):
        f = facts[cid]
        if not f["premiere"]:
            return None
        pre = preair_of(att.get(cid, {}), day_add(f["premiere"], -1)).get("cn") or 0
        act = mean_att(att.get(cid, {}), "cn", f["premiere"], day_add(f["premiere"], 7 * weeks - 1))
        return act / pre if pre > 0 and act > 0 else None

    # (a) the autumn cohort
    if fall:
        rets = [r for _, r in fall]
        fr = [x for x in (cn_ratio(c, 2) for c, _ in fall) if x]
        sr = [x for x in (cn_ratio(c, 2) for c, _ in table) if x]
        ipo_pop = [close_at(facts[c]["rows"], comp[c]["listed_on"])["close"] / comp[c]["ipo_price"] - 1 for c, _ in fall
                   if close_at(facts[c]["rows"], comp[c]["listed_on"])]
        ex = "、".join(f"《{comp[c]['title']}》{pct(r)}" for c, r in fall[:3])
        if fr and sr:
            items.append(("model", f"9 月上市的 {len(fall)} 部秋季新番，从上市首日到 {md(days_all[-1])} 的收益中位数 {pct(st.median(rets))}（{ex}）",
                          [f"开播前两周的中国热度是开播前预估的 {st.median(fr) * 100:.0f}%（中位数），而夏季新番是 {st.median(sr) * 100:.0f}%："
                           f"秋季新番的开播前预估系统性偏低，第一批中国/日本数据一到估值就成倍上调（例如首播周的跳升）",
                           f"上市首日相对发行价的涨幅中位数 {pct(st.median(ipo_pop))}" if ipo_pop else "",
                           "开播前预估的偏差（prior bias）只用夏季一季的数据校准过"]))
    # (b) well-liked shows that fell
    for cid, ret in table:
        inf = info[cid]
        good = (inf["score"] and inf["score"] >= 7.5) or (inf["collect_total"] and inf["collect_total"] >= top_coll)
        if not (good and ret < -0.2):
            continue
        f = facts[cid]
        c = comp[cid]
        rows = f["rows"][f["i0"]:]
        reasons, flag = [], None
        dev = [math.log(r["close"] / r["fair"]) for r in rows[-30:] if r["close"] > 0 and r["fair"] > 0]
        if dev and abs(st.median(dev)) > 0.15:
            reasons.append(f"最后 30 天价格相对估值的偏离中位数 {pct(math.exp(st.median(dev)) - 1)}，价格没有跟上估值")
            flag = "model"
        r4 = cn_ratio(cid, 4)
        if r4 is not None and r4 < 0.8:
            reasons.append(f"开播前的预估偏高：开播四周 Bangumi 实际热度只有预估的 {r4 * 100:.0f}%")
            flag = flag or "ok"
        if inf["collect_total"] and inf["collect_total"] < med_coll:
            reasons.append(f"评分高但观众规模在中位数以下（收藏 {inf['collect_total']:,}，中位数 {med_coll:,}），分红取决于热度份额")
            flag = flag or "ok"
        if c["long_runner"]:
            reasons.append("长篇连载：估值按每周约 0.5% 衰减")
            flag = flag or "ok"
        if not reasons:
            pk = max(rows, key=lambda r: r["fair"])
            reasons.append(f"估值从 {md(pk['day'])} 的 {yuan(pk['fair'])} 币降到 {yuan(rows[-1]['fair'])} 币，期间分红 {yuan(f['divs'])} 币："
                           "后期热度份额低于开季时的预期")
            flag = "ok"
        items.append((flag, f"《{c['title']}》Bangumi {inf['score'] or '–'} 分、收藏 {num(inf['collect_total'])}，季度总收益 {pct(ret)}", reasons))
    # (c) low-rated shows that rose a lot
    low = [(c, r) for c, r in table if info[c]["score"] and info[c]["score"] < 6.0 and r > 0.5]
    if low:
        rr = [x for x in (cn_ratio(c, 4) for c, _ in low) if x]
        ex = "、".join(f"《{comp[c]['title']}》{pct(r)}" for c, r in low[:3])
        why = [f"它们开播四周的中国热度是预估的 {st.median(rr) * 100:.0f}%（中位数）" if rr else "",
               "估值只看热度份额，不看口碑；评分低但看的人比预期多，分红就多"]
        items.append(("ok" if rr and st.median(rr) > 1 else "model",
                      f"{len(low)} 部 Bangumi 6 分以下的作品季度涨超 50%（{ex}）", why))
    # (d) value jumps with no episode and no flash: what changed in the data that day
    jumps = {"first": [], "weekly": [], "none": []}
    for cid, _ in table:
        f = facts[cid]
        rows = f["rows"][f["i0"]:]
        a_ = att.get(cid, {})
        ad = sorted(a_)
        flash_days = {n["day"] for n in news.get(cid, []) if n["kind"] == "flash"}
        air_days = {d for d, _ in f["air"]}
        for r0, r1 in zip(rows, rows[1:]):
            if r0["fair"] <= 0 or r1["fair"] <= 0:
                continue
            ch = (r1["fair"] + r0["div"]) / r0["fair"] - 1
            d = r0["day"]
            if abs(ch) <= 0.35 or d in air_days or d in flash_days or d not in a_:
                continue
            k = ad.index(d)
            prev = a_[ad[k - 1]]["att"] if k else {m: 0.0 for m in MARKETS}
            now = a_[d]["att"]
            first = [MARKET_NAME[m] for m in ("cn", "jp") if prev[m] <= 0 < now[m]]
            weekly = [MARKET_NAME[m] for m in ("cn", "jp") if prev[m] > 0 and now[m] > 0 and now[m] != prev[m]]
            kind = "first" if first else ("weekly" if weekly else "none")
            jumps[kind].append((abs(ch), ch, cid, d, first or weekly))
    labels = {
        "first": ("model", "某个市场的数据在开播几周后才第一次出现，模型把它当成一次性的大幅超预期（数据迟到，不是作品突然变火）"),
        "weekly": ("ok", "当天中国或日本的周数据更新了：每周一次的新数据，估值按超预期幅度修正"),
        "none": ("model", "当天三个市场的数据都没有明显的新信息，原因不明"),
    }
    for kind, lst in jumps.items():
        if not lst:
            continue
        lst.sort(reverse=True)
        seen, ex = set(), []
        for _, ch, cid, d, ms in lst:
            if cid not in seen:
                seen.add(cid)
                ex.append(f"《{comp[cid]['title']}》{md(d)} {pct(ch)}（{'、'.join(ms) or '无'}）")
        flag, why = labels[kind]
        items.append((flag, f"{len(seen)} 部作品在没有新集、也没有快讯的日子估值单日变动超过 35%（{'、'.join(ex[:3])}）", [why]))
    h.append("<p class=mut>筛选：秋季新番整体；Bangumi 7.5 分以上或收藏数前 20% 但季度跌超 20% 的作品逐个列出；6 分以下却涨超 50% 的作品合并成一条；"
             "没有新集也没有快讯却单日估值大变的情况合并成一条。</p><ul>")
    for flag, what, reasons in items:
        tag = '<span class="ok">合理</span>' if flag == "ok" else '<span class="bad">可能是模型问题</span>'
        h.append(f"<li>{tag} {esc(what)}。原因：{esc('；'.join(r for r in reasons if r))}。</li>")
    h.append("</ul>")
    if fall:
        h.append(f"<h3>秋季新番（9 月上市；收益从上市首日收盘算到 {md(days_all[-1])}）</h3><div class=wrap><table>"
                 + head.replace("季度总收益", "上市以来") + "".join(trow(c, r) for c, r in fall) + "</table></div>")
    g = metrics["groups"]["all"]
    h.append(f'<p class="mut">回放统计：{metrics["days"]} 个交易日，{metrics["stocks"]} 支股票，成交间隔中位数 {g["median_trade_gap_min"]:.1f} 分钟，'
             f'日波动中位数 {g["daily_sigma_median"] * 100:.1f}%，热度快讯 {sum(1 for n in sum(news.values(), []) if n["kind"] == "flash")} 条。'
             '生成器：sim/replay_report.py。</p>')
    h.append("</main></body></html>")
    a.out.write_text("\n".join(h), encoding="utf-8")
    print(f"{a.out}: {len(facts)} shows, {sum(len(ids) for _, ids in groups)} charts, {len(items)} 体感 cases")


if __name__ == "__main__":
    main()
