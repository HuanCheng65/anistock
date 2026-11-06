# v0.6.7：修正中国、日本的完结后数据，按新估值调整价格

## 原因

`data/derived/markets.json`（中国、日本的重建序列）由每天日结前的 `sim/reconstruct.py build` 生成。服务器上那一份是 11 月 6 日 06:30 由 v0.6.0 的代码生成的：作品完结后的中国、日本数据按 v1 的完结后曲线外推（完结后第一周约 0.4 倍，之后每周再降）。v0.6.6 重开时只重新生成了每日输入，没有重建它，所以估值把这些外推值当成了观测，而长尾模型预期的水平高得多：已完结作品的估值被逐周压低。

本地按同样的输入估算：正确估值相对现在，已完结作品中位数 1.33 倍（P10–P90 1.16–1.80），长篇连载 1.61 倍，播出 4 周以上的作品 0.87 倍。

从下一次日结起，服务器会用当前代码重建 `markets.json`（完结后的日子记为缺失），不会再错；但滤波里已经被压低的水平不会自己恢复，所以要重建一次估值，并把价格调整到新估值。

## 步骤

约 10 分钟，期间游戏不可用。06:30 日结之前做完（否则把下面的 `2026-11-05` 换成最近一个已结算的交易日）。

```bash
cd ~/anistock
set -a; source .env; set +a
pkill -f server.tools.rebuild_replay || true     # 后台在算开市前走势的话先停掉：它用的是旧输入
```

### 1. 更新程序，停服

```bash
ANISTOCK_VERSION=v0.6.7 bash deploy/update.sh
systemctl --user stop anistock-server
```

### 2. 重建中国、日本序列，重新生成每日输入

```bash
uv run sim/reconstruct.py airtimes
uv run sim/reconstruct.py build
uv run engine/daily_inputs.py --from 2026-07-01 --day 2026-11-05 --force
```

`build` 的输出里 `jp` 和 `cn` 的 `extended` 都应该是 0。

### 3. 在副本上重建估值

```bash
cp data/game.db data/values.db
ANISTOCK_GAME_DB=$PWD/data/values.db ANISTOCK_VALUATION_REBUILD=1 ANISTOCK_NO_LOOPS=1 ANISTOCK_PORT=18699 bin/anistock
```

看到 `valuation: started the filter from the inputs history, N companies valued` 后按 Ctrl-C。

### 4. 撤单，按新估值调整价格

```bash
bin/anistock reprice data/values.db
rm -f data/values.db*
```

- 撤销所有未成交的委托（玩家和 NPC 的，和日结撤单相同：释放冻结的资金和持仓），再把每支股票的估值、最新价和当天冷静期的参考价设为新估值。昨收不变，所以今天的涨跌幅显示这次调整。
- 输出：备份路径（`data/backups/game-before-reprice-<时间>.db`）、撤单数、新价 ÷ 旧价的中位数和 P10、P90（预计中位数 1.1 左右）。

### 5. 发公告，开服

```bash
bin/anistock announce "估值修正" docs/revalue-v0.6.7-announcement.txt
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user start anistock-server
journalctl --user -u anistock-server -n 30 --no-pager
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
```

- 日志里有 `valuation: started the filter from the inputs history`：服务器在同样的数据上重建估值，结果和第 3 步相同。
- **一定要取消这个环境变量**，否则以后每次重启都会重建一次。

### 6. 检查

```bash
python3 -c "
import sqlite3,statistics as s
c=sqlite3.connect('file:data/game.db?mode=ro',uri=True)
r=sorted(f/p for f,p in c.execute(\"SELECT fair_value,last_price FROM companies WHERE status='listed' AND fair_value>0\"))
print(len(r),'只  估值/现价 中位',round(s.median(r),3),' 最小',round(r[0],3),' 最大',round(r[-1],3))"
```

估值/现价应该都是 1（NPC 开始交易后会有小幅偏离）。

## 回滚

```bash
systemctl --user stop anistock-server
cp data/backups/game-before-reprice-<时间>.db data/game.db
rm -f data/game.db-wal data/game.db-shm
systemctl --user start anistock-server
```

## 之后

开市前走势（`server.tools.rebuild_replay`）要在第 2 步的新输入上重新计算，方法见 docs/restart-v0.6.6.md 第 4 步的说明，服务器运行时用 `--dry-run --force` 在后台算，停服后用 `--history` 写入。
