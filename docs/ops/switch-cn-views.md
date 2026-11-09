# 中国热度改为观看量、绝对营收、热度指数第 2 版：上线步骤

一次上线三件事，必须一起切：

1. **中国市场的序列**改为 Bangumi 用户的观看集数（`engine/cn_views.py`，配置 `revenue.cn_model = "views"`），开播前换算系数改为 0.707，估值参数换成重新拟合的版本；
2. **绝对营收**打开（`revenue.absolute_revenue = 1`）；
3. **热度指数第 2 版**（`engine/heat_index.py`），所有未到期的热度期货合约在上线后第一次日结时按当天结算价了结（`[futures] terminate_on`）。

估值模型的状态里存着每部作品的中国热度水平，单位还是旧序列（吐槽数）。所以要先用新序列重建全部历史输入，再从输入历史重建估值状态，最后把股价对齐到新估值。和 v0.6.6 重新开服的第 2、5 步相同，账号和资产都不动。

新旧对比（`sim/replica_check.py`）：385 只股票估值中位数 0.991；变动超过 1.5 倍的 21 只都查明了原因，主要是三类：长期连载作品和国漫第一次有中国数据；吐槽多、观众少的作品下跌；完结作品的中国长尾按实测下调。

## 时间

约 30 分钟，期间游戏不可用。**在 04:00 换日之前做完**：`terminate_on` 是上线后第一次日结的交易日。04:00 之前上线，这个交易日就是今天；04:00 以后上线，要改成那一天，并重新提交、打版本标签。

## 步骤

```bash
cd ~/anistock
set -a; source .env; set +a
```

### 1. 更新程序，停服，备份

```bash
bash deploy/update.sh            # 迁移 V26（快讯的 updated_at）自动执行；会重启一次服务
systemctl --user stop anistock-server   # 马上停掉
cp data/game.db data/game-before-cn-views-$(date +%Y%m%d-%H%M).db
```

### 2. 用新序列重建中国、日本序列和每日输入

```bash
uv run sim/reconstruct.py airtimes
uv run sim/reconstruct.py build          # 读 game.toml 的 revenue.cn_model = "views"
uv run engine/daily_inputs.py --from 2026-07-01 --day $(date -d yesterday +%F) --force
python3 -c "
import json, datetime
d = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()
x = json.load(open(f'data/derived/inputs/{d}.json'))
print(d, len(x['shows']), '中国有数据', sum(s['sources']['cn'] for s in x['shows']), '平台', {k: round(v) for k, v in x['platform'].items()})"
```

- `build` 输出里 `cn shows` 应在 150 以上（旧序列约 80）。
- 平台规模中国是 10000、日本是 1000（固定常数，单位由切换当天的变现系数承担）。

### 3. 从输入历史重建估值，股价对齐估值

```bash
cp data/game.db data/values.db
ANISTOCK_GAME_DB=$PWD/data/values.db ANISTOCK_PORT=18699 ANISTOCK_NO_LOOPS=1 ANISTOCK_VALUATION_REBUILD=1 bin/anistock
```

看到 `valuation: started the filter` 和 `listening on` 后按 Ctrl-C。然后：

```bash
bin/anistock reprice data/values.db
rm data/values.db*
```

- `reprice` 撤销所有挂单，把每支上市股票的价格设为新估值；输出最后一行是新旧价格之比的中位数和 p10、p90，应接近 1、约 0.78、约 1.06。
- 写入前自动备份到 `data/backups/`。

### 4. 启动，发公告

```bash
systemctl --user start anistock-server
bin/anistock announce "热度算法更新" docs/ops/switch-cn-views-announcement.md
```

### 5. 第二天检查

- 日结日志里 `futures` 一行：`settled` 是当天所有未到期合约，两位玩家的仓位已平、保证金已退回。
- `data/derived/heat_index.db` 里当天的行是 `v2`，在播作品指数的中位数在 100 左右。
- 行情页没有大面积异常的涨跌：重算估值后价格已经对齐，第二天的变化应和平时差不多。

## 可选：回填的指数改为第 2 版

股票页图上颜色较浅的回填指数还是第 1 版。在有回填数据的机器上：

```bash
ANISTOCK_DB=<带四季回填的采集库> uv run sim/heat_history.py --archive --cn-views --from 2025-11-01
uv run python -m server.tools.import_heat --export-backfill heat_backfill.csv.gz
```

把 `heat_backfill.csv.gz` 拷到服务器，再运行 `uv run python -m server.tools.import_heat --backfill heat_backfill.csv.gz`。已公布的实时值不会被改。

## 回滚

停服，用第 1 步的备份替换 `data/game.db`，把 `server/game.toml` 的三项改回（`revenue.cn_model = "comments"`、`valuation.preair_cn_per_member` 删除、`revenue.absolute_revenue = 0`），`sim/calibration/valuation_filter.json` 恢复成上一版，重新运行第 2 步的 `build` 和 `daily_inputs`。热度指数已经公布的第 2 版数值不改。
