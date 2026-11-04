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


def build(base: dict, alt: dict | None) -> dict:
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

    def stock_row(sid):
        s = stocks[sid]
        return {
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
    sleeper = max(live, key=lambda s: stocks[s][-1]["c"] / stocks[s][0]["o"] if stocks[s][0]["o"] else 0)
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
        "alt_macro": [{k: r[k] for k in ("d", "rate", "m2", "bills")} for r in alt["macro"]] if alt else None,
        "flows": flows,
        "window": base["window"],
        "generated_at": base["generated_at"],
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default=str(ROOT / "sim" / "out" / "result.json"))
    ap.add_argument("--alt", default=str(ROOT / "sim" / "out" / "result_fastcb.json"))
    ap.add_argument("--template", default=str(ROOT / "sim" / "report_template.html"))
    ap.add_argument("--out", default=str(ROOT / "sim" / "out" / "report.html"))
    args = ap.parse_args()

    base = json.loads(Path(args.base).read_text(encoding="utf-8"))
    alt_path = Path(args.alt)
    alt = json.loads(alt_path.read_text(encoding="utf-8")) if alt_path.exists() else None
    data = build(base, alt)
    payload = json.dumps(data, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")
    html = Path(args.template).read_text(encoding="utf-8").replace("/*__DATA__*/null", payload)
    Path(args.out).write_text(html, encoding="utf-8")
    s = data["stats"]
    print(f"wrote {args.out} ({len(html) / 1024:.0f} KB), window {s['start']}..{s['end']}, "
          f"top={s['top']['title']} {s['top']['share']}%, sleeper={s['sleeper']['title']} x{s['sleeper']['mult']:.1f}")


if __name__ == "__main__":
    main()
