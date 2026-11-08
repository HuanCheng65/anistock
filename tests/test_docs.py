"""The reading documents (web/content/*.md) state the game's numbers as text; this keeps them equal to the
configuration the server runs: server/game.toml, and the code defaults for what game.toml leaves out. A failure
names the number and the document: change the document together with the rule (and announce it)."""

import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONTENT = ROOT / "web" / "content"
TOML = tomllib.loads((ROOT / "server" / "game.toml").read_text())
GAME = TOML["game"]
RUST = ROOT / "backend" / "src"


def doc(name: str) -> str:
    # the table cells and sentences compared as one line each
    return (CONTENT / name).read_text()


def pct(x: float) -> str:
    return f"{x * 100:.4f}".rstrip("0").rstrip(".") + "%"


def rust_default(file: str, pattern: str) -> str:
    m = re.search(pattern, (RUST / file).read_text())
    assert m, f"{file}: {pattern} not found (the code moved: update this test)"
    return m.group(1)


def has(name: str, *texts: str) -> None:
    text = doc(name)
    for t in texts:
        assert t in text, f"{name}: expected “{t}”"


def test_session_times():
    g = GAME
    for name in ("rules.md", "api.md"):
        has(name, f"{g['auction_start']}–{g['trading_open']}", f"{g['trading_open']} 至次日 {g['trading_close']}")
    has("rules.md", f"| 日结 | {g['daily_step_at']} |", f"凌晨 {g['day_rollover']} 以前")
    has("api.md", f"{g['trading_close']}–{g['auction_start']}")


def test_fees_and_tax():
    g = GAME
    has("rules.md", f"| 佣金 | {pct(g['commission'])} |", f"| 印花税 | {pct(g['stamp_duty'])} |",
        f"约为成交额的 {pct(2 * g['commission'] + g['stamp_duty'])}")
    (d0, r0), (d1, r1), (_, r2) = g["dividend_tax"]
    assert d0 == 0 and (d1 + 1) % 7 == 0, "the tax table's wording assumes [same day, whole weeks, rest]"
    weeks = (d1 + 1) // 7
    has("rules.md", f"| 当天买入 | {pct(r0)} |", f"| 持有 1 天到不满 {weeks} 周 | {pct(r1)} |", f"| 持有满 {weeks} 周 | {pct(r2)} |")


def test_vcm_and_price_limit():
    g = GAME
    has("rules.md", f"比 {g['vcm_window_min']} 分钟前偏离超过 {pct(g['vcm_threshold'])}",
        f"进入 {g['vcm_cool_min']} 分钟冷静期", f"参考价 ±{pct(g['vcm_threshold'])}")
    later = re.search(r"later_bands: vec!\[([^\]]+)\]", (RUST / "api/v1/market.rs").read_text()).group(1)
    has("rules.md", "偏离超过 " + "、".join(pct(float(x)) for x in later.split(",")))
    mult = rust_default("exchange.rs", r"sanity_mult: (\d+)")
    has("rules.md", f"超过昨收 {mult} 倍")


def test_ipo():
    g = GAME
    has("rules.md", f"上市前 {g['ipo_subscription_days']} 天开放认购")
    bands = g["ipo_clawback"]
    has("rules.md", f"| {bands[0][0]} 倍以下 | {pct(g['ipo_public_base'])} |")
    for times, share in bands:
        has("rules.md", f"| {times} 倍以上 | {pct(share)} |")


def test_account():
    g = GAME
    has("rules.md", f"注册获得 {g['initial_cash']:,} 星辉", f"总资产低于初始资金的 {pct(g['restructure_threshold'])}")


def test_heat_index():
    f = "futures/rules.rs"
    over = {"index_weights", "index_platform", "index_scale", "index_version"} & set(TOML.get("futures", {}))
    assert not over, f"game.toml [futures] sets {over}: read it here"
    # the defaults (the struct declares the same names with a type)
    w = [float(x) for x in rust_default(f, r"index_weights: \[([\d., ]+)\]").split(",")]
    p = [float(x) for x in rust_default(f, r"index_platform: \[([\d., ]+)\]").split(",")]
    scale = float(rust_default(f, r"index_scale: ([\d.]+)"))
    version = rust_default(f, r'index_version: "v(\d+)"')
    has("heat-index.md", f"指数 = {scale:,.0f} ×", f"现行为第 {version} 版")
    for wt, size, label in zip(w, p, ["欧美", "中国", "日本"]):
        has("heat-index.md", f"{wt:g} × {label} ÷ {size:,.0f}")


def test_api_limits():
    src = (RUST / "api/v1/ratelimit.rs").read_text()
    lim = {k: float(v) for k, v in re.findall(r'f\("(\w+)", ([\d.]+)\)', src)}
    lim |= {k: float(v) for k, v in TOML.get("api", {}).get("rate_limit", {}).items()}  # game.toml overrides
    has("api.md",
        f"每秒 {lim['player_rps']:g} 次，可短时达到 {lim['player_burst']:g} 次",
        f"每秒 {lim['order_rps']:g} 次，可短时达到 {lim['order_burst']:g} 次",
        f"| {lim['orders_per_day']:,.0f} 笔 |",
        f"同时 {lim['max_open_orders']:g} 笔，单支股票 {lim['max_open_orders_per_stock']:g} 笔",
        f"每个账号 {lim['ws_per_account']:g} 条")
    keys = (RUST / "api/v1/keys.rs").read_text()
    n = re.search(r"MAX_KEYS: i64 = (\d+)", keys).group(1)
    life = re.search(r"LIFETIMES: \[i64; 3\] = \[(\d+), (\d+), (\d+)\]", keys).groups()
    has("api.md", f"最多同时持有 {n} 个有效密钥", f"有效期 {life[0]}、{life[1]} 或 {life[2]} 天")
    stocks = rust_default("live.rs", r"MAX_STOCKS: usize = (\d+)")
    has("api.md", f"每条连接最多 {stocks} 支", f"每条连接最多 {stocks} 个")
