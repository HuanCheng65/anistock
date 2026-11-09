# v0.8.3 上线步骤：日本长尾实测、绝对营收立即生效、Bangumi 存档匹配

这一版的内容（设计和检验见 `docs/design/series.md` §12 末尾）：

1. 日本市场完结后的长尾改为按 Annict 观看记录拟合（原来照搬欧美），长尾现值约降 40%。
2. 欧美平台规模用 AniList 历史回填补齐过去一年（`data/derived/platform_us.json`），绝对营收不再等到 12 月上旬，下一次日结起生效；重建估值时也按同样的规则切换。
3. Bangumi 存档匹配（`sim/matches.json` `archive`），中国数据的起始日按存档算；采集时排除成人作品和宣传片。
4. 第 3 步的前提修复：重建估值时走公司的每部作品；“长篇连载”按实际播出时长判定；并入公司的作品读自己的播出日期。

`replica_check`（生产采集库副本，11-08，139 支）：估值中位数 0.93，已完结 0.90，播出中 0.96，长篇 1.03，范围 0.60–1.11；超过 1.5 倍的两支（Let's Go 怪奇组、星球大战：幻境 第九位绝地武士）是热度以日本为主的已完结作品。

和 11-09 那次一样要重建估值、对齐股价，约 30 分钟，期间游戏不可用。**避开 06:30 的日结，在日结完成之后做**：变现系数取自最后一天的输入，那一天必须是完整的一天（第 2 步检查）。

命令按 fish 写。

## 0. 开发机：拷文件到服务器

```fish
scp /home/starrydream/anisotck/data/derived/platform_us.json <服务器>:~/anistock/data/derived/
```

`data/derived/heat_backfill.db`（Bangumi 存档）11-09 已经拷过，这一版每天的日结也要读它，确认还在（第 2 步检查）。

## 1. 更新程序，停服，备份

```fish
cd ~/anistock
for line in (string match -rv '^\s*(#|$)' < .env)
    set -l kv (string split -m1 = -- $line)
    set -gx $kv[1] (string trim -c '"\'' -- $kv[2])
end
ANISTOCK_VERSION=v0.8.3 bash deploy/update.sh
systemctl --user stop anistock-server
cp data/game.db data/game-before-v0.8.3-(date +%Y%m%d-%H%M).db
```

## 2. 重建中国、日本序列和每日输入，检查最后一天

```fish
ls -la data/derived/heat_backfill.db data/derived/platform_us.json   # 两个都要在
uv run sim/reconstruct.py build
uv run engine/daily_inputs.py --from 2026-07-01 --day (date -d yesterday +%F) --force
python3 -c "
import json, datetime, sqlite3
d = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()
x = json.load(open(f'data/derived/inputs/{d}.json'))
c = sqlite3.connect('file:data/game.db?mode=ro', uri=True)
listed = {r[0] for r in c.execute(\"SELECT id FROM companies WHERE status='listed'\")}
s = [v for v in x['shows'] if v['id'] in listed]
r = {m: round(sum(v['att'][m] for v in s) / max(1e-9, sum(v['expected'][m] for v in s)), 2) for m in ('us', 'cn', 'jp')}
print(d, 'platform_days', x['platform_days'], '平台', {k: round(v) for k, v in x['platform'].items()}, '热度/预期', r)"
```

- `platform_days` 应是 365，欧美平台规模约 11.5 万。
- **热度/预期三个市场都应在 0.9 以上。** 中国或日本明显偏低，说明那一天的序列还没齐（采集器还没跑完前一天），不要继续：等采集器跑完再从 `reconstruct.py build` 重来。偏低时定下的变现系数会让所有作品的营收偏离。

## 3. 从输入历史重建估值，股价对齐估值

```fish
cp data/game.db data/values.db
ANISTOCK_GAME_DB=$PWD/data/values.db ANISTOCK_PORT=18699 ANISTOCK_NO_LOOPS=1 ANISTOCK_VALUATION_REBUILD=1 bin/anistock
```

看到 `valuation: started the filter` 和 `listening on` 后按 Ctrl-C。确认已经切到绝对营收：

```fish
sqlite3 data/values.db "SELECT key, value FROM state WHERE key LIKE 'platform%'"
```

应有一行 `platform_rule`（六个数：三个平台规模、三个变现系数）。然后：

```fish
bin/anistock reprice data/values.db
rm -f data/values.db data/values.db-wal data/values.db-shm
```

`reprice` 的最后一行是新旧价格之比：中位数应在 0.93 左右，p10 约 0.8，p90 约 1.0。

## 4. 启动（正式数据库上也重建估值），发公告

```fish
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user start anistock-server
journalctl --user -u anistock-server -n 30 --no-pager | grep valuation
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
sqlite3 data/game.db "SELECT key, value FROM state WHERE key LIKE 'platform%'"
bin/anistock announce "估值更新" docs/ops/deploy-v0.8.3-announcement.md
```

- 日志里应有 `valuation: started the filter from the inputs history`。**启动后一定取消这个环境变量。**
- 正式数据库也应有 `platform_rule`，变现系数和第 3 步的相同（同一份输入）。
- 检查估值和价格一致：

```fish
python3 -c "
import sqlite3, statistics as s
c = sqlite3.connect('file:data/game.db?mode=ro', uri=True)
r = sorted(f / p for f, p in c.execute(\"SELECT fair_value, last_price FROM companies WHERE status='listed' AND fair_value > 0\"))
print(len(r), '只  估值/现价 中位', round(s.median(r), 3), ' 最小', round(r[0], 3), ' 最大', round(r[-1], 3))"
```

## 5. 第二天检查

- 日结后 `platform_rule` 的变现系数不变（平台规模三个数会每天更新）。
- 行情没有大面积异常：估值已对齐，第二天的变化应和平时差不多。
- 采集器（`collect.py` 从仓库运行，下一次运行自动用新代码）的日志里没有错误；AniList 不再抓成人作品和宣传片。

## 回滚

停服，用第 1 步的备份替换 `data/game.db`，`ANISTOCK_VERSION=v0.8.2 bash deploy/update.sh`（并 `git checkout v0.8.2`，让配置、校准和 Python 代码也回到上一版），重新运行第 2 步的 `build` 和 `daily_inputs`。
