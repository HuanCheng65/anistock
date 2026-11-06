//! A self-contained HTML report (inline SVG charts, light/dark aware) of a simulation run.

use std::fmt::Write;

use serde_json::Value;

use super::metrics::Series;

fn f(v: &Value, digits: usize) -> String {
    match v.as_f64() {
        Some(x) if x.is_finite() => format!("{x:.digits$}"),
        Some(x) if x.is_infinite() => "∞".into(),
        _ => "–".into(),
    }
}

fn pct(v: &Value, digits: usize) -> String {
    match v.as_f64() {
        Some(x) if x.is_finite() => format!("{:.digits$}%", x * 100.0),
        _ => "–".into(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A line chart: several series over a shared x index.
fn line_chart(title: &str, xs: &[String], series: &[(&str, &[f64], &str)], h: f64) -> String {
    let w = 640.0;
    let (pl, pr, pt, pb) = (56.0, 12.0, 28.0, 28.0);
    let all: Vec<f64> = series.iter().flat_map(|s| s.1.iter().copied()).filter(|v| v.is_finite()).collect();
    if all.is_empty() {
        return String::new();
    }
    let (mut lo, mut hi) =
        (all.iter().cloned().fold(f64::INFINITY, f64::min), all.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
    if hi - lo < 1e-9 {
        hi += 1.0;
        lo -= 1.0;
    }
    let pad = (hi - lo) * 0.05;
    lo -= pad;
    hi += pad;
    let n = series.iter().map(|s| s.1.len()).max().unwrap_or(1).max(2);
    let x = |i: usize| pl + (w - pl - pr) * i as f64 / (n - 1) as f64;
    let y = |v: f64| pt + (h - pt - pb) * (1.0 - (v - lo) / (hi - lo));
    let mut s = format!(
        r#"<figure><svg viewBox="0 0 {w} {h}" role="img" aria-label="{t}"><text x="{pl}" y="16" class="ct">{t}</text>"#,
        t = esc(title)
    );
    for k in 0..=4 {
        let v = lo + (hi - lo) * k as f64 / 4.0;
        let _ = write!(
            s,
            r#"<line x1="{pl}" x2="{}" y1="{y}" y2="{y}" class="grid"/><text x="{}" y="{}" class="ax" text-anchor="end">{}</text>"#,
            w - pr,
            pl - 6.0,
            y(v) + 4.0,
            fmt_num(v),
            y = y(v)
        );
    }
    if let (Some(a), Some(b)) = (xs.first(), xs.last()) {
        let _ = write!(
            s,
            r#"<text x="{pl}" y="{}" class="ax">{}</text><text x="{}" y="{}" class="ax" text-anchor="end">{}</text>"#,
            h - 8.0,
            esc(a),
            w - pr,
            h - 8.0,
            esc(b)
        );
    }
    let mut legend_x = pl + 200.0;
    for (name, vals, cls) in series {
        let pts: Vec<String> = vals
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
            .map(|(i, v)| format!("{:.1},{:.1}", x(i), y(*v)))
            .collect();
        let _ = write!(s, r#"<polyline points="{}" class="{cls}" fill="none"/>"#, pts.join(" "));
        let _ = write!(
            s,
            r#"<rect x="{legend_x}" y="8" width="10" height="3" class="{cls}f"/><text x="{}" y="14" class="ax">{}</text>"#,
            legend_x + 14.0,
            esc(name)
        );
        legend_x += 110.0;
    }
    s.push_str("</svg></figure>");
    s
}

fn fmt_num(v: f64) -> String {
    if v.abs() >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if v.abs() >= 1e3 {
        format!("{:.0}k", v / 1e3)
    } else if v.abs() >= 10.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

fn bar_chart(title: &str, labels: &[String], vals: &[f64], h: f64) -> String {
    let w = 640.0;
    let (pl, pr, pt, pb) = (48.0, 12.0, 28.0, 30.0);
    let hi = vals.iter().cloned().filter(|v| v.is_finite()).fold(0.0f64, |a, b| a.max(b.abs())).max(1e-9);
    let lo = vals.iter().cloned().filter(|v| v.is_finite()).fold(0.0f64, f64::min);
    let (lo, hi) = (lo.min(0.0), hi);
    let y = |v: f64| pt + (h - pt - pb) * (1.0 - (v - lo) / (hi - lo));
    let bw = (w - pl - pr) / vals.len().max(1) as f64;
    let mut s = format!(
        r#"<figure><svg viewBox="0 0 {w} {h}" role="img" aria-label="{t}"><text x="{pl}" y="16" class="ct">{t}</text>"#,
        t = esc(title)
    );
    let _ = write!(s, r#"<line x1="{pl}" x2="{}" y1="{y0}" y2="{y0}" class="axis"/>"#, w - pr, y0 = y(0.0));
    for k in 0..=4 {
        let v = lo + (hi - lo) * k as f64 / 4.0;
        let _ = write!(
            s,
            r#"<text x="{}" y="{}" class="ax" text-anchor="end">{}</text>"#,
            pl - 6.0,
            y(v) + 4.0,
            fmt_num(v)
        );
    }
    for (i, v) in vals.iter().enumerate() {
        if !v.is_finite() {
            continue;
        }
        let (y0, y1) = (y(0.0), y(*v));
        let _ = write!(
            s,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" class="bar"/>"#,
            pl + bw * i as f64 + 1.0,
            y0.min(y1),
            (bw - 2.0).max(1.0),
            (y0 - y1).abs()
        );
        if labels.len() <= 30 || i % (labels.len() / 12).max(1) == 0 {
            let _ = write!(
                s,
                r#"<text x="{:.1}" y="{}" class="ax" text-anchor="middle">{}</text>"#,
                pl + bw * (i as f64 + 0.5),
                h - 10.0,
                esc(&labels[i])
            );
        }
    }
    s.push_str("</svg></figure>");
    s
}

fn histogram(title: &str, data: &[f64]) -> String {
    let bins = 40;
    let (lo, hi) = (-6.0, 6.0);
    let mut counts = vec![0.0; bins];
    for x in data {
        if x.is_finite() {
            let k = (((x - lo) / (hi - lo)) * bins as f64).floor().clamp(0.0, (bins - 1) as f64) as usize;
            counts[k] += 1.0;
        }
    }
    let n = data.len().max(1) as f64;
    let width = (hi - lo) / bins as f64;
    let dens: Vec<f64> = counts.iter().map(|c| (c / n / width).max(1e-4).log10()).collect();
    let normal: Vec<f64> = (0..bins)
        .map(|k| {
            let x = lo + width * (k as f64 + 0.5);
            ((-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt()).max(1e-4).log10()
        })
        .collect();
    let xs: Vec<String> = vec![format!("{lo}σ"), format!("{hi}σ")];
    line_chart(title, &xs, &[("模拟（log10 密度）", &dens, "s1"), ("正态分布", &normal, "s2")], 240.0)
}

pub fn render(
    base: &Value,
    series: &Series,
    bots: &[(String, Value)],
    config_toml: &str,
    threshold_pts: f64,
) -> String {
    let mut h = String::new();
    let g = &base["groups"];
    let row = |name: &str, target: &str, cell: &dyn Fn(&Value) -> String, verdict: &str| -> String {
        format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            name,
            target,
            cell(&g["hot"]),
            cell(&g["mid"]),
            cell(&g["cold"]),
            cell(&g["all"]),
            verdict
        )
    };
    let ok = |b: bool| if b { "<span class=ok>达标</span>" } else { "<span class=bad>未达标</span>" };
    let all = &g["all"];
    let hot = &g["hot"];
    let cold = &g["cold"];
    let gap_ok = all["median_trade_gap_min"].as_f64().is_some_and(|x| x <= 10.0)
        && hot["median_trade_gap_min"].as_f64() <= all["median_trade_gap_min"].as_f64();
    let sig = all["daily_sigma_median"].as_f64().unwrap_or(f64::NAN);
    let sig_ok = (0.02..=0.05).contains(&sig);
    let ev_ok = all["daily_sigma_event_days"].as_f64() > all["daily_sigma_other_days"].as_f64();
    let kurt_ok = all["kurtosis_daily"].as_f64().is_some_and(|k| k > 3.0);
    let acf_abs = all["acf_abs_5min"][0].as_f64().unwrap_or(0.0);
    let clus_ok = acf_abs > 0.0 && all["acf_abs_5min"][3].as_f64().unwrap_or(0.0) > 0.0;
    let pred_ok = all["acf_1min_lag1"].as_f64().is_some_and(|x| x.abs() < 0.1)
        && all["acf_1h_lag1"].as_f64().is_some_and(|x| x.abs() < 0.1);
    let hl = all["deviation_half_life_days"].as_f64().unwrap_or(f64::INFINITY);
    let hl_ok = (1.0..=10.0).contains(&hl);
    let sp_hot = hot["spread_median"].as_f64().unwrap_or(f64::NAN);
    let sp_ok = (0.003..=0.012).contains(&sp_hot) && cold["spread_median"].as_f64() > hot["spread_median"].as_f64();
    let npc_pct = base["npc"]["excess_vs_passive_pct"].as_f64().unwrap_or(f64::NAN);
    let npc_ok = npc_pct.abs() < 0.05;

    let _ = write!(
        h,
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>NPC 市场仿真报告</title>
<style>
:root{{--bg:#fbfaf7;--fg:#1d1d1f;--mut:#6b6b70;--line:#e3e1db;--c1:#2f6fbf;--c2:#c2583a;--c3:#3a8f5c;--ok:#2d7d46;--bad:#b3261e;--card:#fff}}
@media (prefers-color-scheme: dark){{:root:not([data-theme="light"]){{--bg:#151517;--fg:#ececee;--mut:#9a9aa1;--line:#2c2c30;--c1:#6ea8ff;--c2:#ff8a65;--c3:#6fcf97;--ok:#6fcf97;--bad:#ff6b6b;--card:#1d1d20}}}}
:root[data-theme="dark"]{{--bg:#151517;--fg:#ececee;--mut:#9a9aa1;--line:#2c2c30;--c1:#6ea8ff;--c2:#ff8a65;--c3:#6fcf97;--ok:#6fcf97;--bad:#ff6b6b;--card:#1d1d20}}
body{{background:var(--bg);color:var(--fg);font:15px/1.6 system-ui,-apple-system,"PingFang SC","Noto Sans CJK SC",sans-serif;margin:0;padding:24px 16px}}
main{{max-width:980px;margin:0 auto}} h1{{font-size:24px;margin:0 0 4px}} h2{{font-size:18px;margin:32px 0 8px;border-bottom:1px solid var(--line);padding-bottom:4px}}
.mut{{color:var(--mut)}} table{{border-collapse:collapse;width:100%;font-size:13.5px;font-variant-numeric:tabular-nums}}
th,td{{text-align:left;padding:6px 8px;border-bottom:1px solid var(--line);vertical-align:top}} th{{color:var(--mut);font-weight:600}}
.wrap{{overflow-x:auto}} .ok{{color:var(--ok);font-weight:600}} .bad{{color:var(--bad);font-weight:600}}
figure{{margin:12px 0;background:var(--card);border:1px solid var(--line);border-radius:8px;padding:8px}} svg{{width:100%;height:auto;display:block}}
.ct{{fill:var(--fg);font-size:13px;font-weight:600}} .ax{{fill:var(--mut);font-size:11px}} .grid{{stroke:var(--line)}} .axis{{stroke:var(--mut)}}
.s1{{stroke:var(--c1);stroke-width:1.6}} .s2{{stroke:var(--c2);stroke-width:1.6}} .s3{{stroke:var(--c3);stroke-width:1.6}}
.s1f{{fill:var(--c1)}} .s2f{{fill:var(--c2)}} .s3f{{fill:var(--c3)}} .bar{{fill:var(--c1)}}
pre{{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px;overflow-x:auto;font-size:12.5px}}
</style></head><body><main>
<h1>NPC 市场仿真报告</h1>
<p class="mut">{} – {}，{} 个交易日，{} 支股票，{} 笔成交，NPC 委托 {:.1} 笔/秒（交易时段平均），用时 {:.0} 秒。不变量：{}。</p>
"#,
        base["first_day"].as_str().unwrap_or(""),
        base["last_day"].as_str().unwrap_or(""),
        base["days"],
        base["stocks"],
        base["trades"],
        base["npc_orders_per_s"].as_f64().unwrap_or(0.0),
        base["elapsed_s"].as_f64().unwrap_or(0.0),
        if base["invariants_ok"].as_bool() == Some(true) { "全部成立" } else { "<b class=bad>不成立</b>" }
    );

    h.push_str(&valuation_acceptance(base, bots, threshold_pts));
    h.push_str(&money_table(base, bots));
    h.push_str(&stylised_table(g));
    h.push_str("<h2>市场指标（NPC 市场设计 §7）</h2><div class=wrap><table><tr><th>指标</th><th>目标</th><th>热门</th><th>中等</th><th>冷门</th><th>全部</th><th>结论</th></tr>");
    h.push_str(&row(
        "成交间隔中位数（分钟）",
        "几分钟一笔，热门更快",
        &|v| f(&v["median_trade_gap_min"], 1),
        ok(gap_ok),
    ));
    h.push_str(&row("每天成交笔数（中位数）", "", &|v| f(&v["median_trades_per_day"], 0), ""));
    h.push_str(&row("日收益标准差（中位数）", "2%–5%", &|v| pct(&v["daily_sigma_median"], 2), ok(sig_ok)));
    h.push_str(&row("　其中：隐藏估值自身的日波动", "（参考）", &|v| pct(&v["daily_sigma_fair_median"], 2), ""));
    h.push_str(&row(
        "　其中：错价 log(价格/估值) 的日变化",
        "（市场自身带来的部分）",
        &|v| pct(&v["daily_sigma_mispricing_median"], 2),
        ok(all["daily_sigma_mispricing_median"].as_f64().is_some_and(|x| x <= 0.05)),
    ));
    h.push_str(&row(
        "事件日 / 其他日 波动",
        "事件日明显放大",
        &|v| {
            format!(
                "{} / {}（{} 个事件日）",
                pct(&v["daily_sigma_event_days"], 2),
                pct(&v["daily_sigma_other_days"], 2),
                v["event_days"]
            )
        },
        ok(ev_ok),
    ));
    h.push_str(&row("日收益峰度（标准化后合并）", "> 3", &|v| f(&v["kurtosis_daily"], 1), ok(kurt_ok)));
    h.push_str(&row("5 分钟收益峰度", "> 3", &|v| f(&v["kurtosis_5min"], 1), ""));
    h.push_str(&row(
        "|5 分钟收益| 自相关（滞后 1/3/6/12）",
        "为正、缓慢衰减",
        &|v| {
            format!(
                "{} / {} / {} / {}",
                f(&v["acf_abs_5min"][0], 2),
                f(&v["acf_abs_5min"][1], 2),
                f(&v["acf_abs_5min"][2], 2),
                f(&v["acf_abs_5min"][3], 2)
            )
        },
        ok(clus_ok),
    ));
    h.push_str(&row(
        "|日收益| 自相关（滞后 1/2/5）",
        "为正",
        &|v| {
            format!(
                "{} / {} / {}",
                f(&v["acf_abs_daily"][0], 2),
                f(&v["acf_abs_daily"][1], 2),
                f(&v["acf_abs_daily"][2], 2)
            )
        },
        "",
    ));
    h.push_str(&row("1 分钟收益自相关（滞后 1）", "≈ 0", &|v| f(&v["acf_1min_lag1"], 3), ok(pred_ok)));
    h.push_str(&row("1 小时收益自相关（滞后 1）", "≈ 0", &|v| f(&v["acf_1h_lag1"], 3), ""));
    h.push_str(&row(
        "价格/估值偏离半衰期（天）",
        "几天",
        &|v| {
            format!(
                "{}（AR1 {}，偏离 sd {}）",
                f(&v["deviation_half_life_days"], 1),
                f(&v["deviation_ar1"], 2),
                pct(&v["deviation_sd"], 1)
            )
        },
        ok(hl_ok),
    ));
    h.push_str(&row(
        "买卖价差中位数",
        "热门 0.5%–1%，冷门更宽",
        &|v| {
            format!(
                "{}（p90 {}，双边报价 {}）",
                pct(&v["spread_median"], 2),
                pct(&v["spread_p90"], 2),
                pct(&v["two_sided_share"], 0)
            )
        },
        ok(sp_ok),
    ));
    h.push_str("</table></div>");
    let _ = write!(
        h,
        "<p class=mut>日收益按含分红的总收益计。隐藏估值（v1 的估值公式，本步不改）自身的日波动中位数就有 {}（赛季仿真里：播出新集的日子估值平均下跌约 6%，因为剩余集数少了一集；其余日子的日变化均方根也有约 10%，来自每日营收数据本身的起伏）。价格跟随估值，所以日收益标准差不可能落在 2%–5%，除非 NPC 不跟随隐藏估值（那样玩家就能用公开的公式预测价格）。市场本身叠加的部分是“错价的日变化”，目标按 ≤ 5% 判。未计入统计的低价股（中位价低于 1 币，tick 占价格 10% 以上、VCM 区间取整后只剩一个价位）：{} 支。</p>",
        pct(&all["daily_sigma_fair_median"], 1),
        base["penny_stocks_excluded"]
    );

    let n = &base["npc"];
    let _ = write!(
        h,
        "<h2>NPC 财富</h2><div class=wrap><table><tr><th></th><th>开始</th><th>结束</th><th>资金流入净额</th><th>分红/利息等收入</th><th>交易盈亏</th><th>结论</th></tr>\
<tr><td>NPC（{} 个）</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}（{}）</td><td></td></tr>\
<tr><td>做市商</td><td>{}</td><td>{}</td><td>{}</td><td>–</td><td>{}（{}）</td><td></td></tr></table></div>\
<p>NPC 相对“被动持有”的超额：<b>{}</b>（被动持有 = 开始时的现金和持仓原样拿到赛季末，含税后分红；NPC 实际 = 期末财富 − 资金流入净额）{}。目标：赛季内接近 0（按 |超额| ≤ 5% 判）。</p>\
<p class=mut>交易盈亏 = 期末财富 − 期初财富 − 资金流入净额 − 收入，含持仓随估值下跌的账面损失；财富按最后成交价计。资金流：流入 {} 币，流出 {} 币（主要是分红收入超过上限后流回外部世界）。</p>",
        base["npc_agents"],
        f(&n["agents_start"], 0), f(&n["agents_end"], 0), f(&n["agents_fund_net"], 0), f(&n["agents_income"], 0),
        f(&n["agents_trading_pnl"], 0), pct(&n["agents_trading_pnl_pct"], 2),
        f(&n["mm_start"], 0), f(&n["mm_end"], 0), f(&n["mm_fund_net"], 0), f(&n["mm_trading_pnl"], 0), pct(&n["mm_trading_pnl_pct"], 2),
        pct(&n["excess_vs_passive_pct"], 2), ok(npc_ok),
        f(&n["fund_in"], 0), f(&n["fund_out"], 0)
    );

    if let Some(p) = base.get("performance") {
        let _ = write!(
            h,
            "<h2>性能</h2><p>NPC 委托 {} 笔，引擎处理用时 {:.1} 秒（每笔委托含撤单约 {:.1} 微秒，单线程）。每分钟 NPC 委托数：中位数 {}，p99 {}，最多 {}（即平均 {:.1} 笔/秒，峰值 {:.1} 笔/秒）。</p>",
            p["npc_orders"], p["npc_step_seconds"].as_f64().unwrap_or(0.0), p["us_per_npc_order"].as_f64().unwrap_or(0.0),
            p["orders_per_min_median"], p["orders_per_min_p99"], p["orders_per_min_max"],
            base["npc_orders_per_s"].as_f64().unwrap_or(0.0), p["orders_per_min_max"].as_f64().unwrap_or(0.0) / 60.0
        );
    }

    if !bots.is_empty() {
        let baseline =
            bots.iter().find(|(k, _)| k == "buy_hold").map(|(_, v)| v["return"].as_f64().unwrap_or(0.0)).unwrap_or(0.0);
        h.push_str("<h2>§6 被测策略（每个策略单独一次赛季回放，同一种子，玩家初始资金，另有 39 个不交易的玩家分财政补贴）</h2><div class=wrap><table><tr><th>策略</th><th>赛季收益</th><th>相对买入持有</th><th>其中财政补贴</th><th>下单次数</th><th>结论</th></tr>");
        for (k, v) in bots {
            let r = v["return"].as_f64().unwrap_or(f64::NAN);
            let excess = r - baseline;
            let verdict = if k == "buy_hold" || k == "buy_hold_cap" {
                "基准".to_string()
            } else if excess * 100.0 <= threshold_pts {
                ok(true).to_string()
            } else {
                ok(false).to_string()
            };
            let label = match k.as_str() {
                "buy_hold" => "买入全市场持有（等权，基准）",
                "buy_hold_cap" => "买入全市场持有（按市值加权）",
                "scalper" => "吃价差：在做市商买一 +1 挂买、卖一 −1 挂卖",
                "scalper_taker" => "吃价差：直接买卖一（穿过价差）",
                "value" => "低于估值 15% 买入、高于估值卖出",
                "value_hold" => "低于估值 10% 买入，长期持有",
                "pump" => "拉抬到冷静期以下再出货",
                "spoof" => "挂大单撤单影响报价",
                "vcm_fade" => "推到 VCM 冷静期再反向操作",
                "air_calendar" => "日历：播出前一晚卖出、播出后买回",
                "air_early" => "日历：只在第 1、2 集播出前一晚卖出、播出后买回",
                "fan_0.1" => "懂行的粉丝：提前两天知道估值，误差 10%",
                "fan_0.3" => "懂行的粉丝：提前两天知道估值，误差 30%",
                "ipo_all" => "打新：认购每一只新股（每只预算相同），中签后持有",
                "fan_long_0.3" => "粉丝的判断：看过第 1 集后知道未来四周的价值，误差 30%，每周调仓",
                "ex_dividend" => "日历：除息前（周日晚）卖出、周一买回",
                "fade_flash" => "热度快讯：利空快讯后买入，一天后卖出",
                "ride_flash" => "热度快讯：利好快讯后买入，一天后卖出",
                "momentum_k" => "动量：持有过去 5 个交易日涨得最多的 5 支",
                "momentum_1" => "动量：持有前一个交易日涨得最多的 5 支",
                "contrarian_k" => "反转：持有过去 5 个交易日跌得最多的 5 支",
                "contrarian_1" => "反转：持有前一个交易日跌得最多的 5 支",
                "sentiment_timer" => "情绪择时：火热时清仓，冷淡时买回全市场",
                "premiere_rush" => "首播：首播前一晚买入，首播后一晚卖出",
                "bubble_rider" => "追涨：热门股 5 日涨幅刚达 25% 时买入，3 天后卖出",
                "bubble_rider_1" => "追涨：热门股 5 日涨幅刚达 25% 时买入，1 天后卖出",
                "disposition_fade" => "处置效应：每周持有热门股中低于 20 日均价最多的 5 支",
                "hold_hottest" => "持有最热的 3 支",
                "spoof_scare" => "挂大单吓退 NPC",
                "bad_price" => "挂坏价格等 NPC 成交",
                _ => k,
            };
            let tr = v["transfers"].as_f64().unwrap_or(0.0) / v["start"].as_f64().unwrap_or(1.0);
            let _ = write!(
                h,
                "<tr><td>{}</td><td>{:.2}%</td><td>{:+.2} 个百分点</td><td>{:.1}%</td><td>{}</td><td>{}</td></tr>",
                label,
                r * 100.0,
                excess * 100.0,
                tr * 100.0,
                v["orders"],
                verdict
            );
        }
        let _ = write!(
            h,
            "</table></div><p class=mut>标准：赛季收益不超过买入全市场持有 + {threshold_pts:.0} 个百分点。</p>"
        );
    }

    h.push_str("<h2>图表</h2>");
    let days = &series.days;
    h.push_str(&line_chart("NPC 总财富（币）", days, &[("NPC", &series.npc_wealth, "s1")], 220.0));
    h.push_str(&line_chart("做市商总财富（币）", days, &[("做市商", &series.mm_wealth, "s2")], 220.0));
    for (_, title, grp, closes, fairs) in &series.samples {
        let name = format!(
            "{}（{}）：收盘价与隐藏估值",
            title,
            match grp.as_str() {
                "hot" => "热门",
                "mid" => "中等",
                _ => "冷门",
            }
        );
        let d: Vec<String> = days.iter().take(closes.len()).cloned().collect();
        h.push_str(&line_chart(&name, &d, &[("收盘价", closes, "s1"), ("估值", fairs, "s2")], 240.0));
    }
    for (_, title, path) in &series.intraday {
        let vals: Vec<f64> = path.iter().map(|x| x.1).collect();
        let labels = vec!["09:00".to_string(), "23:00".to_string()];
        h.push_str(&line_chart(&format!("{title} 盘中逐笔成交价"), &labels, &[("成交价", &vals, "s3")], 220.0));
    }
    h.push_str(&histogram("标准化日收益分布（log10 密度）", &series.std_daily_returns));
    let lab5: Vec<String> = (1..=series.acf_abs_5min.len()).map(|k| format!("{}", k * 5)).collect();
    h.push_str(&bar_chart("|5 分钟收益| 的自相关（横轴：滞后分钟）", &lab5, &series.acf_abs_5min, 220.0));
    let labd: Vec<String> = (1..=series.acf_abs_daily.len()).map(|k| k.to_string()).collect();
    h.push_str(&bar_chart("|日收益| 的自相关（横轴：滞后天数）", &labd, &series.acf_abs_daily, 220.0));

    let _ = write!(
        h,
        "<h2>运行统计</h2><pre>{}</pre>",
        esc(&serde_json::to_string_pretty(&base["market"]).unwrap_or_default())
    );
    let _ = write!(h, "<h2>参数（[market]）</h2><pre>{}</pre>", esc(config_toml));
    h.push_str("</main></body></html>");
    h
}

fn verdict(b: bool) -> &'static str {
    if b { "<span class=ok>通过</span>" } else { "<span class=bad>未通过</span>" }
}

fn bot_ret(bots: &[(String, Value)], k: &str) -> Option<f64> {
    bots.iter().find(|(n, _)| n == k).and_then(|(_, v)| v["return"].as_f64())
}

/// docs/v2-valuation.md §7: the valuation's acceptance table.
fn valuation_acceptance(base: &Value, bots: &[(String, Value)], threshold_pts: f64) -> String {
    let r = &base["revisions"];
    let g = |k: &str| r[k].as_f64().unwrap_or(f64::NAN);
    let mut h = String::from("<h2>估值验收（v2 估值 §7）</h2><div class=wrap><table><tr><th>检验</th><th>要求</th><th>结果</th><th>结论</th></tr>");
    // 1. forecast coverage (fitted on last season, held-out shows)
    let fit = &base["filter_fit"];
    let mut cov = Vec::new();
    let mut cov_ok = fit.is_object();
    for m in ["us", "cn", "jp"] {
        let a = fit[m]["coverage80_in_sample"].as_f64().unwrap_or(f64::NAN);
        let b = fit[m]["coverage80_held_out"].as_f64().unwrap_or(f64::NAN);
        cov_ok &= (0.75..=0.90).contains(&b);
        cov.push(format!("{m} {:.1}% / {:.1}%", a * 100.0, b * 100.0));
    }
    let _ = write!(h, "<tr><td>预测是否准确</td><td>下一次观测落在 80% 预测区间的比例接近 80%（75%–90%，留出作品）</td><td>{}（样本内 / 留出）</td><td>{}</td></tr>", cov.join("；"), verdict(cov_ok));
    // 2. unpredictable revisions
    let wd: Vec<f64> = r["weekday_mean"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
    let wd_mean = wd.iter().sum::<f64>() / wd.len().max(1) as f64;
    let wd_dev = wd.iter().map(|x| (x - wd_mean).abs()).fold(0.0, f64::max);
    let lag_ok = g("lag1_corr").abs() < 0.1;
    let early = g("air_first2w_simple_mean");
    let se = g("air_first2w_se");
    let air_ok = early.abs() <= 2.0 * se.max(0.005) && (g("air_later_simple_mean") - g("other_simple_mean")).abs() < 0.02;
    let _ = write!(
        h,
        "<tr><td>修正是否不可预测</td><td>与前一天的修正、星期几、播出日都不相关</td><td>滞后 1 相关 {:.3}；星期几均值最大偏离 {:.2}%；播出日（首播后两周内）平均 {:+.2}%（标准误 {:.2}%，{} 个）；其后播出日 {:+.2}%，其他日子 {:+.2}%</td><td>{}</td></tr>",
        g("lag1_corr"), wd_dev * 100.0, early * 100.0, se * 100.0, r["air_first2w_n"], g("air_later_simple_mean") * 100.0, g("other_simple_mean") * 100.0,
        verdict(lag_ok && wd_dev < 0.01 && air_ok)
    );
    // 3. where the volatility is
    let vol_ok = g("info_rms") > 2.0 * g("no_info_rms");
    let _ = write!(
        h,
        "<tr><td>波动出现的位置</td><td>有新信息（播出、新数据、快讯）的日子明显大于其他日子，其他日子接近 0</td><td>有信息 {:.1}%（{} 天）/ 无信息 {:.1}%（{} 天，|修正| 中位数 {:.2}%）</td><td>{}</td></tr>",
        g("info_rms") * 100.0, r["info_days"], g("no_info_rms") * 100.0, r["no_info_days"], g("no_info_median_abs") * 100.0, verdict(vol_ok)
    );
    // 4. premiere week strongest
    let ord_ok = g("first_week_rms") > g("middle_rms") && g("middle_rms") > g("last_two_weeks_rms");
    let _ = write!(
        h,
        "<tr><td>首播最激烈</td><td>首播后一周 > 中段 > 最后两周（日修正均方根）</td><td>{:.1}% > {:.1}% > {:.1}%</td><td>{}</td></tr>",
        g("first_week_rms") * 100.0, g("middle_rms") * 100.0, g("last_two_weeks_rms") * 100.0, verdict(ord_ok)
    );
    // 5. calendar strategies vs the same holdings bought and held (equal weight)
    if let Some(bh) = bot_ret(bots, "buy_hold") {
        let mut parts = Vec::new();
        let mut ok = true;
        for k in ["air_calendar", "air_early", "ex_dividend"] {
            if let Some(x) = bot_ret(bots, k) {
                ok &= x <= bh;
                parts.push(format!("{k} {:+.2}", (x - bh) * 100.0));
            }
        }
        let _ = write!(h, "<tr><td>日历策略</td><td>播出前卖出播出后买回、只在第 1–2 集这样做、除息前卖出除息后买回：都不跑赢买入持有（等权，同样的持仓）</td><td>相对买入持有：{} 个百分点</td><td>{}</td></tr>", parts.join("；"), verdict(ok));
    }
    // 6. exploits vs cap-weighted buy and hold
    if let Some(cap) = bot_ret(bots, "buy_hold_cap") {
        let mut parts = Vec::new();
        let mut ok = true;
        for k in ["scalper", "scalper_taker", "value", "value_hold", "pump", "spoof", "vcm_fade"] {
            if let Some(x) = bot_ret(bots, k) {
                ok &= (x - cap) * 100.0 <= threshold_pts;
                parts.push(format!("{k} {:+.2}", (x - cap) * 100.0));
            }
        }
        let _ = write!(h, "<tr><td>防薅羊毛</td><td>跑赢按市值买入持有不超过 {threshold_pts:.0} 个百分点</td><td>{} 个百分点</td><td>{}</td></tr>", parts.join("；"), verdict(ok));
    }
    h.push_str("</table></div><p class=mut>修正 = 隐藏估值相邻两天开盘时的变化（中间是收盘），分红加回。买入持有在前五天分批建仓。</p>");
    h
}

/// docs/v2-sentiment.md §4: the stylised facts, per heat group (measurements; the verdicts need judgement and
/// a comparison with the run without the sentiment layer).
fn stylised_table(g: &Value) -> String {
    let mut h = String::from("<h2>市场特征（情绪层 §4）</h2><div class=wrap><table><tr><th>特征</th><th>指标</th><th>热门</th><th>中等</th><th>冷门</th><th>全部</th></tr>");
    let cell = |v: &Value| v.as_f64().map_or("–".to_string(), |x| format!("{x:.3}"));
    let rows: [(&str, &str, &dyn Fn(&Value) -> String); 9] = [
        ("波动聚集", "日收益绝对值自相关（滞后 1、2、5 天）", &|x: &Value| {
            (0..3).map(|i| cell(&x["acf_abs_daily"][i])).collect::<Vec<_>>().join(" / ")
        }),
        ("肥尾", "日收益峰度", &|x: &Value| cell(&x["kurtosis_daily"])),
        ("短期反转", "1 小时收益滞后 1 自相关", &|x: &Value| cell(&x["acf_1h_lag1"])),
        ("短期反转", "日收益滞后 1、2 自相关", &|x: &Value| {
            format!("{} / {}", cell(&x["stylised"]["acf_daily_lag1"]), cell(&x["stylised"]["acf_daily_lag2"]))
        }),
        ("中期动量", "价格/估值偏离的半衰期（天）", &|x: &Value| cell(&x["deviation_half_life_days"])),
        ("偏离有界", "log(价格/估值) 的 5%、50%、95% 分位", &|x: &Value| {
            let st = &x["stylised"];
            format!("{} / {} / {}", cell(&st["deviation_p05"]), cell(&st["deviation_p50"]), cell(&st["deviation_p95"]))
        }),
        ("偏离有界", "同向偏离超过 10% 的最长连续天数（p90 / 最长）", &|x: &Value| {
            let st = &x["stylised"];
            format!("{} / {}", cell(&st["deviation_run_10pct_p90_days"]), cell(&st["deviation_run_10pct_max_days"]))
        }),
        ("量与关注", "日成交笔数与热度的相关（全部 / 去掉个股均值）；事件日成交 ÷ 平日", &|x: &Value| {
            let st = &x["stylised"];
            format!("{} / {}；{}", cell(&st["corr_trades_heat_pooled"]), cell(&st["corr_trades_heat_within"]), cell(&st["trades_event_day_ratio_median"]))
        }),
        ("情绪", "火热日与冷淡日的平均 log(价格/估值) 之差", &|x: &Value| cell(&x["stylised"]["deviation_hot_minus_cold_sentiment"])),
    ];
    for (name, metric, f) in rows {
        let _ = write!(h, "<tr><td>{name}</td><td>{metric}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", f(&g["hot"]), f(&g["mid"]), f(&g["cold"]), f(&g["all"]));
    }
    h.push_str("</table></div>");
    h
}

/// docs/v2-valuation.md §5: where the money is and where it went.
fn money_table(base: &Value, bots: &[(String, Value)]) -> String {
    let mo = base["money"].as_array().cloned().unwrap_or_default();
    let (Some(a), Some(b)) = (mo.first(), mo.last()) else { return String::new() };
    let v = |x: &Value, k: &str| x[k].as_f64().unwrap_or(0.0);
    let mut h = String::from("<h2>资金检验（v2 估值 §5）</h2><div class=wrap><table><tr><th>存量（币）</th><th>期初</th><th>期末</th><th>变化</th></tr>");
    let row = |h: &mut String, name: &str, x0: f64, x1: f64| {
        let pct = if x0.abs() > 0.0 { format!("（{:+.1}%）", (x1 / x0 - 1.0) * 100.0) } else { String::new() };
        let _ = write!(h, "<tr><td>{name}</td><td>{}</td><td>{}</td><td>{}{}</td></tr>", fmt_num(x0), fmt_num(x1), fmt_num(x1 - x0), pct);
    };
    row(&mut h, "玩家现金（= 财富，基准回放里的玩家不交易）", v(a, "players_cash"), v(b, "players_cash"));
    row(&mut h, "NPC 现金", v(a, "npc_cash"), v(b, "npc_cash"));
    row(&mut h, "NPC 财富（现金 + 持仓市值）", v(a, "npc_wealth"), v(b, "npc_wealth"));
    row(&mut h, "居民财富合计（玩家 + NPC）", v(a, "players_wealth") + v(a, "npc_wealth"), v(b, "players_wealth") + v(b, "npc_wealth"));
    row(&mut h, "公司现金（留存收益和新股募资，计入每股净资产，退市时返还股东）", v(a, "company_cash"), v(b, "company_cash"));
    row(&mut h, "做市商现金", v(a, "mm_cash"), v(b, "mm_cash"));
    row(&mut h, "做市商财富", v(a, "mm_wealth"), v(b, "mm_wealth"));
    row(&mut h, "货币总量（玩家 + NPC + 公司 + 做市商的现金）", v(a, "money_stock"), v(b, "money_stock"));
    row(&mut h, "政府（税收减去转移支付和利息）", v(a, "gov_cash"), v(b, "gov_cash"));
    row(&mut h, "总市值（全部上市公司）", v(a, "market_cap"), v(b, "market_cap"));
    row(&mut h, "期初上市公司的市值", v(a, "start_set_cap"), v(b, "start_set_cap"));
    let _ = write!(
        h,
        "<tr><td>　其间这些公司付出的分红和退市返还（所有持有人，含税和版权方部分）</td><td></td><td>{}</td><td></td></tr><tr><td>上市公司数</td><td>{}</td><td>{}</td><td></td></tr></table></div>",
        fmt_num(v(b, "start_set_paid")), a["listed"], b["listed"]
    );
    // flows
    let l = &base["ledger_by_reason"];
    let f = |k: &str| l[k].as_f64().unwrap_or(0.0);
    h.push_str("<div class=wrap><table><tr><th>流量（整季，币）</th><th>金额</th><th>方向</th></tr>");
    for (k, name, dir) in [
        ("revenue", "营收", "外部世界 → 公司（注入）"),
        ("operating_cost", "运营成本", "公司 → 外部世界"),
        ("corp_tax", "企业所得税", "公司 → 政府"),
        ("dividend", "分红（给股东，税后）", "公司 → 居民、做市商"),
        ("dividend_tax", "分红税", "公司 → 政府"),
        ("dividend_export", "分红（版权方等外部股东）", "公司 → 外部世界"),
        ("production_fee", "制作费", "公司 → 外部世界"),
        ("ipo", "新股募资", "居民、做市商 → 公司"),
        ("delisting", "退市返还", "公司 → 持有人"),
        ("transfer", "转移支付（按人头，含 NPC）", "政府 → 居民"),
        ("interest", "利息", "政府 → 居民"),
        ("npc_fund_in", "NPC 资金流入", "外部世界 → NPC"),
        ("npc_fund_out", "NPC 资金流出", "NPC → 外部世界"),
        ("grant", "玩家初始资金", "外部世界 → 玩家"),
    ] {
        let _ = write!(h, "<tr><td>{name}</td><td>{}</td><td>{dir}</td></tr>", fmt_num(f(k)));
    }
    h.push_str("</table></div>");
    // checks
    let n = &base["npc"];
    let npc_drift = n["agents_wealth_drift_pct"].as_f64().unwrap_or(f64::NAN);
    let excess = n["excess_vs_passive_pct"].as_f64().unwrap_or(f64::NAN);
    let pl0 = v(a, "players_wealth");
    let pl1 = v(b, "players_wealth");
    let tr_pct = bots
        .iter()
        .find(|(k, _)| k == "buy_hold")
        .map(|(_, x)| x["transfers"].as_f64().unwrap_or(0.0) / x["start"].as_f64().unwrap_or(1.0))
        .unwrap_or(f64::NAN);
    let cap_ratio = v(b, "start_set_cap") / v(a, "start_set_cap").max(1.0) - 1.0;
    h.push_str("<div class=wrap><table><tr><th>检验</th><th>要求</th><th>结果</th><th>结论</th></tr>");
    let _ = write!(h, "<tr><td>账本总和</td><td>恒为 0</td><td>{}</td><td>{}</td></tr>", if base["invariants_ok"].as_bool() == Some(true) { "所有不变量成立" } else { "不成立" }, verdict(base["invariants_ok"].as_bool() == Some(true)));
    let _ = write!(
        h,
        "<tr><td>居民财富</td><td>不因注入持续单向增减：NPC 财富漂移 ≤ 5%，玩家只多出转移支付</td><td>NPC {:+.1}%；不交易的玩家 {:+.1}%（转移支付 {:.1}%）</td><td>{}</td></tr>",
        npc_drift * 100.0, (pl1 / pl0.max(1.0) - 1.0) * 100.0, tr_pct * 100.0, verdict(npc_drift.abs() <= 0.05 && pl1 / pl0.max(1.0) - 1.0 <= tr_pct + 0.01)
    );
    let _ = write!(
        h,
        "<tr><td>综合指数</td><td>不因资金注入上涨：期初上市公司的市值随作品完结下降，价值以分红付出</td><td>市值 {:+.1}%，其间付出 {} 币</td><td>{}</td></tr>",
        cap_ratio * 100.0, fmt_num(v(b, "start_set_paid")), verdict(cap_ratio <= 0.05)
    );
    let _ = write!(h, "<tr><td>玩家每季转移支付</td><td>占初始资金的合理比例（≤ 10%）</td><td>{:.1}%</td><td>{}</td></tr>", tr_pct * 100.0, verdict(tr_pct <= 0.10));
    let _ = write!(h, "<tr><td>玩家与 NPC 的财富占比</td><td>NPC 相对被动持有的超额 |x| ≤ 3%</td><td>{:+.1}%</td><td>{}</td></tr>", excess * 100.0, verdict(excess.abs() <= 0.03));
    h.push_str("</table></div><p class=mut>货币总量的增长落在公司现金（每周利润的 10% 留存、新股募资减制作费；计入每股净资产，退市时按每股净资产返还股东）和做市商现金（持仓收到的分红；做市商财富基本不变，持仓随作品完结贬值）。居民的现金和财富由 NPC 资金流（超过上限流回外部世界）和税收约束。</p>");
    h
}
