# v0.6.6：重新开服（保留账号，重置资产）

## 为什么重开

1. **开盘价是旧方法的估值。** `server.seed` 按 v1（Python）的估值定开盘价，服务器启动时换成自己的滤波估值，但不改价格。11 月 4 日开服时，估值约是价格的 2.27 倍，价格开服两天中位数涨了 47%，还会继续涨。
2. **估值少了中国、日本两个市场。** 4c92166 按“采集库第一次看到这部作品的那天”判断数据源接入；采集库是 11 月 5 日新建的，所以之前每一天的中国、日本数据源都判成没有。重建估值（v0.6.4 第 4 步）和开市前走势都只用了欧美数据。修正后同一支股票的估值相差 0.5–2 倍。

v0.6.6 修正第 2 条（`engine/daily_inputs.py`），并带上重开要用的工具：`server.tools.reset_assets`（保留账号）、`server.tools.reanchor`（开盘价对齐估值）、`anistock announce`（公告弹窗）。

## 时间

整个过程约 40 分钟（其中重算开市前走势约 20 分钟），期间游戏不可用。**04:00 之前做完**：下面的 `seed --until 2026-11-05` 让开市日是 11 月 6 日这个交易日，04:00 换日以后要改日期（见最后一节）。

## 步骤

```bash
cd ~/anistock
set -a; source .env; set +a
```

### 1. 更新程序，停服，备份

```bash
ANISTOCK_VERSION=v0.6.6 bash deploy/update.sh
systemctl --user stop anistock-server
cp data/game.db data/game-before-restart-$(date +%Y%m%d-%H%M).db
mv data/game.db data/game-old.db
rm -f data/game.db-wal data/game.db-shm
```

- `update.sh` 会重启一次服务（执行迁移 V14），马上停掉即可。
- `game-old.db` 是账号的来源，第 3 步要用；`game-before-restart-*.db` 是回滚用的备份。

### 2. 重新生成每日输入

```bash
uv run engine/daily_inputs.py --from 2026-07-01 --day 2026-11-05 --force
python3 -c "
import json
for d in ('2026-07-15','2026-10-15','2026-11-05'):
    s=json.load(open(f'data/derived/inputs/{d}.json'))['shows']
    print(d, len(s), '中国', sum(x['sources']['cn'] for x in s), '日本', sum(x['sources']['jp'] for x in s))"
```

三天的“中国”“日本”都应该是大部分作品（例如 7 月 15 日 78 部里约 77 部），不能是 0。

### 3. 新开一局，带上账号，发公告

```bash
uv run python -m server.seed --force --until 2026-11-05
bin/anistock announce "重新开服说明" docs/restart-v0.6.6-announcement.txt
uv run python -m server.tools.reset_assets data/game-old.db data/game.db
```

- `announce` 先把新库迁移到当前版本，再写入公告。公告只弹给开户早于它的账号，所以要在 `reset_assets` 之前发：带过来的账号都保留原来的开户时间。
- `reset_assets` 最后一行列出带过来的玩家数和各项记录数，玩家数应等于旧库里的玩家数（注销的不算）。

### 4. 用当前模型重算开市前走势

`server.seed` 的开市前 K 线和营收来自 Python 的旧模拟器（开盘价总是贴着昨收，没有分红记录，营收规模和服务器不同）。用服务器自己的引擎重算一遍，约 20 分钟：

```bash
uv run python -m server.tools.rebuild_replay data/game.db --work /tmp/rebuild-replay
rm -rf /tmp/rebuild-replay
```

- 必须在第 5 步之前：估值从开市前每天的已播集数（`company_days.aired`）推出播出日，重算会改写这些行；第 5 步在重算之后的数据上取估值，开服时服务器算出的估值才和它相同。
- 新走势先缩放到 seed 的收盘价，第 5 步再把整条回放 K 线和发行价按同一比例缩放到估值：两步都是按股票整体乘一个比例，形状不变，开市处没有缺口。
- 输出的表里每支股票“last close”的原值和新值相同；最后一行是 `rewrote the replay history of N stocks; untouched tables verified (...)`。
- `kept as is` 的股票（新回放里开市前已退市或没有上市）保留原来的走势。
- 写入前自动备份为 `data/game-before-rebuild-<时间>.db`；结果不对就用它恢复，再从这一步重来或跳过这一步。

### 5. 开盘价对齐估值

```bash
cp data/game.db data/golive.db
ANISTOCK_GAME_DB=$PWD/data/golive.db ANISTOCK_PORT=18699 ANISTOCK_NO_LOOPS=1 bin/anistock
```

看到 `valuation: started the filter from the inputs history, N companies valued` 后按 Ctrl-C（`ANISTOCK_NO_LOOPS=1`：不跑 NPC、不日结，副本里不会有成交）。

```bash
uv run python -m server.tools.reanchor data/game.db data/golive.db
rm -f data/golive.db*
```

### 6. 开服，检查

```bash
systemctl --user start anistock-server
journalctl --user -u anistock-server -n 30 --no-pager
python3 -c "
import sqlite3,statistics as s
c=sqlite3.connect('file:data/game.db?mode=ro',uri=True)
r=sorted(f/p for f,p in c.execute(\"SELECT fair_value,prev_close FROM companies WHERE status='listed' AND fair_value>0\"))
print(len(r),'只  估值/现价 中位',round(s.median(r),2),' P10',round(r[len(r)//10],2),' P90',round(r[len(r)*9//10],2))"
```

- 日志里有 `valuation: started the filter from the inputs history`，没有 `valuation bootstrap failed`。
- 估值/现价的中位、P10、P90 都在 1 附近。偏离大就停服，回到第 3 步之前排查。
- 用自己的账号登录：弹出“重新开服说明”，点“知道了”后不再出现；资金是 100,000，QQ 绑定还在。
- 任意一支股票的日 K：开市前的走势和开市后接得上，开市处有一条细线。

## 回滚

```bash
systemctl --user stop anistock-server
cp data/game-before-restart-<时间>.db data/game.db
rm -f data/game.db-wal data/game.db-shm
systemctl --user start anistock-server
```

## 04:00 之后才做

交易日已经换成 11 月 7 日。要等 06:30 采集和日结跑完（生成 11 月 6 日的每日输入），再把上面的 `2026-11-05` 都换成 `2026-11-06`。否则服务器开服时会补一次 11 月 6 日的日结：那一天没有任何成交，估值又被推进一天，和对齐好的开盘价错开。
