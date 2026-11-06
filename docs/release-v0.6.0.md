# v0.6.0 发布说明

## 玩家看到的变化

- **一个系列一支股票。** 已上市作品的新一季并入原来的公司，不再单独上市；公司营收是名下各季之和。
- **完结后还有长尾收入。** 作品完结后热度落到一个底，营收和分红随之持续，估值把这部分计算在内。长期营收太低、续作希望又小的公司会退市，按每股净资产返还现金。
- **公司公告与资本政策。** 排期延期、暂停播出会发公告。管理层按公开规则拆股（股价连续 5 个交易日高于 100 币）、配股（续作制作费不够时）和回购（股价长期低于公司估计的价值时）。规则见规则页“拆股、配股、回购”。
- **K 线默认前复权**，可以切换为不复权。
- **个股页改版**：下半部分分为经营、作品、公告、股东四栏。
- **指数改为全收益**：分红当天加回每股分红；成分按当天在市的股票计算，退市的股票退市前的涨跌照样计入；拆股、配股不改变指数。
- **市场里的 NPC**：热门股的价差更窄、挂单更深；股价连续大涨后，有时会出现追涨泡沫，随后破裂。

## 上线步骤（运营）

上线前确认 GitHub 上已经发布了 `v0.6.0` 的服务程序（`deploy/update.sh` 从发布页下载）。避开每天 06:30 的日结，整个过程约 20 分钟。

### 1. 更新

```bash
cd ~/anistock
bash deploy/update.sh
```

- 启动时自动执行迁移 **V9**（`company_works`，作品与公司的对应）和 **V10**（`corporate_actions`、`rights_subscriptions`、`price_adjustments`，资本政策）。迁移前自动备份到 `data/backups/game-before-migration-<时间>.db`。
- 日志里应看到：

```bash
journalctl --user -u anistock-server -n 50 --no-pager
```

```
applied database migrations: ["V9__company_works", "V10__corporate"]
AniStock v2 listening on http://127.0.0.1:18620 ...
```

### 2. 重新生成每日输入

新版估值要用每日输入的新字段（系列、平台规模、公开排期、暂停播出、下一集），旧文件没有这些字段。从数据窗口的第一天（游戏状态 `window_start`，线上是 2026-07-01）到上一个已结算的交易日全部重写：

```bash
cd ~/anistock
set -a; source .env; set +a      # 与服务相同的环境（ANISTOCK_DB、ANISTOCK_GAME_DB、ANISTOCK_INPUTS_DIR 等，未设置时用默认路径）
uv run engine/daily_inputs.py --from 2026-07-01 --day <上一个已结算的交易日> --force
```

- `<上一个已结算的交易日>` 可以这样查：`sqlite3 data/game.db "SELECT value FROM state WHERE key='last_closed'"`。
- 文件写到 `data/derived/inputs/`（或 `ANISTOCK_INPUTS_DIR`）。每天一行输出，例如 `data/derived/inputs/2026-07-01.json: 74 shows`。
- 服务重建估值时只读最近 120 天的文件，更早的文件不影响结果，只是保留完整历史。
- 不要设置 `ANISTOCK_TAIL_INGEST`（中国、日本的真实长尾数据还没有校准，默认关闭）。

### 3. 用新模型重建估值（只重启这一次）

```bash
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user restart anistock-server
journalctl --user -u anistock-server -n 50 --no-pager
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
```

- 日志里应看到 `valuation: started the filter from the inputs history, N companies valued`，N 约等于上市公司数。
- 不应出现 `valuation bootstrap: skipping <日期>: ...`（某天的输入文件读不了，回到第 2 步检查这一天）或 `valuation bootstrap failed`（重建失败，估值会从下一次日结起重新开始，需要排查后再做一次第 3 步）。
- 一定要取消这个环境变量：留着的话，以后每次重启都会重建一次。
- 重建后估值会一次性变化：营收规模下调（`demand_base_week` 250 万 → 162 万），长尾让估值上升，两者大体抵消，但单支股票会有明显的涨跌。

### 4. 检查

- 网页：市场页的指数、个股页的四个分栏、K 线的前复权切换、规则页“拆股、配股、回购”。
- 第二天日结之后的日志：没有 `inputs` 相关的警告；`/api/v1/market/indices` 返回 `"basis": "total_return"`。
- 系列合并：`uv run python -m server.tools.series_dryrun data/game.db` 预演换股（只读）。2026-11-06 的副本上没有需要合并的公司；如果有，暂不处理，记录下来。

### 回滚

停服，用第 1 步的备份替换 `data/game.db`，再用 `ANISTOCK_VERSION=v0.5.2 bash deploy/update.sh` 装回上一个版本。

## 暂不打开的功能

两项已经实现，按配置关闭，约 12 月初再打开：

| 功能 | 配置（`server/game.toml` 的 `[overrides]`） | 打开的条件 |
|---|---|---|
| 绝对营收（按平台规模归一，不随平台增长漂移） | `"revenue.absolute_revenue" = 1` | 平台规模有 182 天真实历史（`revenue.platform_min_days`）。采集器从 2026-06-06 起有数据，约 12 月 5 日满足。打开后不会立即切换：每日输入的 `platform_days` 达到 182 的那一天自动切换，变现系数按“当天营收总额不变”定一次 |
| 续作期权（估值包含续作公布的可能） | `"valuation.sequel_option" = 1` | 采集器积累了足够的 AniList 关系快照，能识别续作和“本身是续作” |

打开方式：在仓库里改这两行，发布新版本，然后 `bash deploy/update.sh`（`update.sh` 用 `git pull --ff-only`，服务器上不要直接改 `game.toml`）。只改配置时不需要重建估值，也不需要重新生成每日输入。

处置效应（情绪层第二批）同样已经实现，暂不打开（`disposition_ratio = 1.0`），原因见 `docs/v2-sentiment.md` §7.3。
