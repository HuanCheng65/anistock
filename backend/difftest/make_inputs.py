"""Derive a richer inputs set for the close difftest from real inputs files (seeded, deterministic).

    python backend/difftest/make_inputs.py SRC_DIR OUT_DIR [SEED]

Real collector data ends after a few days, so later files carry zero attention and nothing airs. This
keeps the first day as is and, for every later day: gives each show the first day's attention times
seeded noise, airs one more episode a week (premieres and finales), adds three synthetic upcoming shows
on different days (more IPOs, one titled only in Japanese, one with an unknown episode count), and
renames one company. It also writes <day>.bad for one day: that day's inputs with a listed, not
long-running show missing its episode count, which makes v1's close raise half-way (rollback test).
"""

import copy
import json
import random
import sys
from datetime import date, timedelta
from pathlib import Path


def main(src, out, seed=7):
    rng = random.Random(seed)
    files = sorted(Path(src).glob("*.json"))
    first = json.loads(files[0].read_text(encoding="utf-8"))
    base = {s["id"]: s for s in first["shows"]}
    Path(out).mkdir(parents=True, exist_ok=True)
    d0 = date.fromisoformat(first["day"])
    template = next(s for s in first["shows"] if s["aired"] == 0 and not s["long_runner"])
    synth = []
    for k, (lag, cn, known) in enumerate([(1, "合成新番甲", True), (4, None, True), (8, "合成新番丙", False)]):
        s = copy.deepcopy(template)
        s.update(id=990001 + k, title_ja=f"合成アニメ{k}", title_en=None if k == 1 else f"Synthetic {k}",
                 studio="Studio Synth", episodes=13 if known else 13, episodes_known=known,
                 popularity_end=40000 + k)
        synth.append((d0 + timedelta(days=lag), s, cn))
    for i, f in enumerate(files):
        d = json.loads(f.read_text(encoding="utf-8"))
        day = date.fromisoformat(d["day"])
        if i > 0:
            for s in d["shows"]:
                b = base.get(s["id"])
                if b and all(v == 0 for v in s["att"].values()):
                    noise = rng.lognormvariate(0, 0.25)
                    s["att"] = {m: v * noise for m, v in b["att"].items()}
                    s["expected"] = {m: max(v, s["att"][m]) for m, v in b["expected"].items()}
                fa = date.fromisoformat(s["first_air"])
                if fa <= day and s["episodes"] and not s["long_runner"]:
                    s["aired"] = max(s["aired"], min(s["episodes"], (day - fa).days // 7 + 1))
                    s["has_real_data"] = (day - fa).days >= 2
            if i == 5:
                d["cn_titles"][str(d["shows"][0]["id"])] = "改过的中文名"
        for when, s, cn in synth:
            if day >= when:
                t = copy.deepcopy(s)
                t["first_air"] = (when + timedelta(days=26)).isoformat()
                d["shows"].append(t)
                if cn:
                    d["cn_titles"][str(t["id"])] = cn
        text = json.dumps(d, ensure_ascii=False, separators=(",", ":"))
        (Path(out) / f.name).write_text(text, encoding="utf-8")
        if i == 4:
            bad = json.loads(text)
            victim = next(s for s in bad["shows"] if s["episodes"] and not s["long_runner"] and s["aired"] > 0)
            victim["episodes"] = None
            (Path(out) / f"{d['day']}.bad").write_text(json.dumps(bad, ensure_ascii=False), encoding="utf-8")
    print(f"{len(files)} days written to {out}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 7)
