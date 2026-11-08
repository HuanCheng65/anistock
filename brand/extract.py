"""Extract the glyph outlines the AniEx wordmark uses into glyphs.json.

Run only when the typefaces or weights change; build.py works from the committed glyphs.json.
    uv run --with fonttools --with brotli brand/extract.py [FONT_DIR]
FONT_DIR may hold already unpacked fontsource packages (fontsource-variable-geist-*/package/...);
packages not found there are fetched with `npm pack` (Noto Sans SC is tens of MB).
Fonts come from npm (fontsource, SIL OFL): Geist for "niEx", Noto Sans SC (思源黑体) for 安易.
"""
import json, pathlib, subprocess, tarfile, tempfile
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.boundsPen import BoundsPen

HERE = pathlib.Path(__file__).parent
LATIN = ("@fontsource-variable/geist", "geist-latin-wght-normal.woff2", 750, "niExH")
CJK = [("@fontsource-variable/noto-sans-sc", "noto-sans-sc-118-wght-normal.woff2", 650, "安"),
       ("@fontsource-variable/noto-sans-sc", "noto-sans-sc-116-wght-normal.woff2", 650, "易")]

import glob, sys
FONT_DIR = sys.argv[1] if len(sys.argv) > 1 else None

def fetch(pkg, tmp):
    if FONT_DIR:
        hit = glob.glob(f"{FONT_DIR}/{pkg[1:].replace('/', '-')}-*/package/files")
        if hit:
            return pathlib.Path(hit[0])
    name = subprocess.run(["npm", "pack", "--silent", pkg], cwd=tmp, check=True, capture_output=True, text=True).stdout.strip().splitlines()[-1]
    out = pathlib.Path(tmp) / pkg.replace("/", "_")
    with tarfile.open(pathlib.Path(tmp) / name) as t:
        t.extractall(out, filter="data")
    return out / "package" / "files"

def glyphs(path, wght, chars, key):
    f = instantiateVariableFont(TTFont(path), {"wght": wght})
    gs, cmap = f.getGlyphSet(), f.getBestCmap()
    res = {}
    for ch in chars:
        name = cmap[ord(ch)]
        rec = DecomposingRecordingPen(gs); gs[name].draw(rec)
        svg = SVGPathPen(gs); rec.replay(svg)
        bounds = BoundsPen(gs); gs[name].draw(bounds)
        res[ch] = {"adv": gs[name].width, "b": bounds.bounds, "d": svg.getCommands()}
    return {key: {"upm": f["head"].unitsPerEm, "wght": wght, "glyphs": res}}

with tempfile.TemporaryDirectory() as tmp:
    out = {}
    pkg, file, w, chars = LATIN
    out.update(glyphs(fetch(pkg, tmp) / file, w, chars, "latin"))
    cjk_dir = fetch(CJK[0][0], tmp)
    cjk = {"upm": 1000, "wght": CJK[0][2], "glyphs": {}}
    for _, file, w, ch in CJK:
        cjk["glyphs"].update(glyphs(cjk_dir / file, w, ch, "x")["x"]["glyphs"])
    out["cjk"] = cjk
(HERE / "glyphs.json").write_text(json.dumps(out, ensure_ascii=False, indent=1))
print("wrote", HERE / "glyphs.json")
