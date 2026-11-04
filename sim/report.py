# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Build sim/out/report.html from simulation results.

    uv run sim/report.py [--base sim/out/result.json] [--alt sim/out/result_fastcb.json]
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FEATURE_KEYWORDS = ["薬屋のひとりごと 第3期", "ONE PIECE", "サイバーパンク", "追放されたチート"]
N_FEATURED = 10
N_SHARE = 8
N_TABLE = 20


def weekly(rows: list[dict], keys: list[str]) -> list[dict]:
    """Sum daily flows into ISO weeks, labelled by the week's Sunday."""
    acc: dict[str, dict] = {}
    for r in rows:
        d = date.fromisoformat(r["d"])
        sunday = d.fromordinal(d.toordinal() + (6 - d.weekday()))
        w = acc.setdefault(sunday.isoformat(), {"d": sunday.isoformat(), **{k: 0.0 for k in keys}})
        for k in keys:
            w[k] += r[k] or 0.0
    return [acc[k] for k in sorted(acc)]


def build(base: dict, alt: dict | None, fiscal: dict | None, finale: dict | None, mstats: dict | None) -> dict:
    meta, stocks, macro = base["meta"], base["stocks"], base["macro"]
    last_day = macro[-1]["d"]

    # revenue over the last 7 days decides the ranking
    def recent_rev(sid):
        return sum(r["rev"] for r in stocks[sid][-7:])

    live = [sid for sid in stocks if stocks[sid]]
    total_recent = sum(recent_rev(s) for s in live) or 1.0
    by_rev = sorted(live, key=recent_rev, reverse=True)

    featured = []
    for kw in FEATURE_KEYWORDS:
        featured += [s for s in by_rev if kw in meta[s]["title"] and s not in featured][:1]
    featured += [s for s in by_rev if s not in featured][: N_FEATURED - len(featured)]
    featured.sort(key=recent_rev, reverse=True)

    # weekly revenue share for the top shows
    week_tot = defaultdict(float)
    week_rev = defaultdict(lambda: defaultdict(float))
    for sid in live:
        for r in stocks[sid]:
            d = date.fromisoformat(r["d"])
            wk = d.fromordinal(d.toordinal() + (6 - d.weekday())).isoformat()
            week_tot[wk] += r["rev"]
            week_rev[sid][wk] += r["rev"]
    weeks = sorted(week_tot)
    share_series = [{
        "id": sid, "title": meta[sid]["title"],
        "points": [{"time": w, "value": round(100 * week_rev[sid][w] / week_tot[w], 2) if week_tot[w] else 0}
                   for w in weeks],
    } for sid in by_rev[:N_SHARE]]

    def mix(sid):
        rm = [sum(r.get("rm", [0, 0, 0])[i] for r in stocks[sid][-7:]) for i in range(3)]
        t = sum(rm) or 1.0
        return [round(100 * v / t) for v in rm]   # us, cn, jp

    def stock_row(sid):
        s = stocks[sid]
        return {
            "mix": mix(sid),
            "id": sid, "title": meta[sid]["title"], "title_en": meta[sid]["title_en"],
            "studio": meta[sid]["studio"], "episodes": meta[sid]["episodes"],
            "ipo": s[0]["o"], "price": s[-1]["c"], "value": s[-1]["v"], "div_w": s[-1]["div_w"],
            "share": round(100 * recent_rev(sid) / total_recent, 2), "aired": s[-1]["aired"],
            "list_day": meta[sid]["list_day"],
        }

    flows = weekly(macro, ["injected", "tax_corp", "tax_div", "tax_stamp", "transfer", "interest"])
    for w in flows:
        w["tax"] = w["tax_corp"] + w["tax_div"] + w["tax_stamp"]

    m0, m1 = macro[0], macro[-1]
    # sleeper: biggest gain among shows that matter (≥1% of revenue), not a floor-priced micro-cap
    floor = base["params"]["company"]["ipo_price_min"]
    pool = [s for s in live if recent_rev(s) / total_recent >= 0.01 and stocks[s][0]["o"] > floor * 1.01] or live
    sleeper = max(pool, key=lambda s: stocks[s][-1]["c"] / stocks[s][0]["o"])
    op = next((s for s in live if "ONE PIECE" in meta[s]["title"]), None)
    stats = {
        "listed": m1["listed"],
        "days": len(macro),
        "start": macro[0]["d"],
        "end": last_day,
        "m2_start": m0["m2"], "m2_end": m1["m2"],
        "bills_end": m1["bills"], "gov_end": m1["gov"],
        "injected_total": sum(r["injected"] for r in macro),
        "tax_total": sum(r["tax_corp"] + r["tax_div"] + r["tax_stamp"] for r in macro),
        "transfer_total": sum(r["transfer"] for r in macro),
        "rate_start": m0["rate"], "rate_end": m1["rate"],
        "alt_rate_end": alt["macro"][-1]["rate"] if alt else None,
        "alt_growth_end": alt["macro"][-1]["m2_growth_ann"] if alt else None,
        "alt_bills_end": alt["macro"][-1]["bills"] if alt else None,
        "fiscal_rate_end": fiscal["macro"][-1]["rate"] if fiscal else None,
        "fiscal_growth_end": fiscal["macro"][-1]["m2_growth_ann"] if fiscal else None,
        "growth_end": m1["m2_growth_ann"],
        "no_cn_jp": sum(1 for s in live if mix(s)[1] == 0 and mix(s)[2] == 0),
        "market_stats": mstats,
        "decisions": sum(1 for r in macro if date.fromisoformat(r["d"]).weekday() == 6),
        "op_share": stock_row(op)["share"] if op else None,
        "top": stock_row(by_rev[0]),
        "sleeper": {**stock_row(sleeper), "mult": stocks[sleeper][-1]["c"] / stocks[sleeper][0]["o"]},
        "params": base["params"],
    }

    return {
        "stats": stats,
        "featured": [{**stock_row(s), "candles": stocks[s]} for s in featured],
        "table": [stock_row(s) for s in by_rev[:N_TABLE]],
        "share": share_series,
        "macro": [{k: r[k] for k in ("d", "m2", "bills", "gov", "rate", "m2_growth_ann", "mcap")} for r in macro],
        "alt_macro": [{k: r[k] for k in ("d", "rate", "m2", "bills", "m2_growth_ann")} for r in alt["macro"]] if alt else None,
        "fiscal_macro": [{k: r[k] for k in ("d", "rate", "m2_growth_ann")} for r in fiscal["macro"]] if fiscal else None,
        "finale": {k: finale[k] for k in ("shows", "median", "p25", "p75", "tail_decay")} if finale else None,
        "flows": flows,
        "window": base["window"],
        "generated_at": base["generated_at"],
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default=str(ROOT / "sim" / "out" / "result.json"))
    ap.add_argument("--alt", default=str(ROOT / "sim" / "out" / "result_fastcb.json"))
    ap.add_argument("--fiscal", default=str(ROOT / "sim" / "out" / "result_fiscal.json"))
    ap.add_argument("--template", default=str(ROOT / "sim" / "report_template.html"))
    ap.add_argument("--out", default=str(ROOT / "sim" / "out" / "report.html"))
    args = ap.parse_args()

    base = json.loads(Path(args.base).read_text(encoding="utf-8"))
    alt_path = Path(args.alt)
    alt = json.loads(alt_path.read_text(encoding="utf-8")) if alt_path.exists() else None
    def opt(path):
        f = Path(path)
        return json.loads(f.read_text(encoding="utf-8")) if f.exists() else None
    fiscal = opt(args.fiscal)
    finale = opt(ROOT / "data" / "derived" / "finale_profile.json")
    markets = opt(ROOT / "data" / "derived" / "markets.json")
    data = build(base, alt, fiscal, finale, markets["stats"] if markets else None)
    payload = json.dumps(data, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")
    html = Path(args.template).read_text(encoding="utf-8").replace("/*__DATA__*/null", payload)
    Path(args.out).write_text(html, encoding="utf-8")
    s = data["stats"]
    print(f"wrote {args.out} ({len(html) / 1024:.0f} KB), window {s['start']}..{s['end']}, "
          f"top={s['top']['title']} {s['top']['share']}%, sleeper={s['sleeper']['title']} x{s['sleeper']['mult']:.1f}")


if __name__ == "__main__":
    main()
