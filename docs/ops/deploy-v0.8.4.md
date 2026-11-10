# v0.8.4 上线步骤：系列前作注入、续作期权、按系列上市

这一版的内容（设计和检验见 `docs/design/series.md` §5、§12 的“v0.8.4：打开续作期权”、§13）：

1. 采集器从在播、待播和一年内首播的作品出发，登记它们系列里还没追踪的较早作品（`collect_series_members`），回填 450 天的 AniList 趋势；Bangumi 匹配和 Annict 也覆盖这些作品，并统计 Annict 每天的观看记录（评估窗口的日本数据）。
2. 系列划分：OVA、特别篇并入所属系列；`sim/series.json` 新增 `cut`（龙珠超 比鲁斯只并入“超”这条线；哆啦A梦 2005 不并入 1979 年版；航海王 MONSTERS 不并入）。
3. 日结：已上市系列首播早于 60 天的作品，按公开评估值和 20 日均价向制作委员会发新股，并入公司（`company.asset_injection = 1`）。退市时作品版权按评估值收购。
4. **续作期权打开**（`valuation.sequel_option = 1`）：估值包含将来出续作的可能；续作公布时收 20% 制作费（现金不够配股）。续作的开播前估计单独校准（`prior_bias_sequel`，约为首季的一半）。管理员可以按官方来源提前公布续作或宣布不再制作（`engine/sequel_override.py`）。
5. **按系列上市**：新作品上市时，系列里更早的、不属于任何公司的作品在认购开始那天并入，发行价含它们的评估值。
6. 名下有多部作品的公司改用系列名（约 40 家）。行情、本季新番筛选和指数、今晚放送、封面改按公司的“当前作品”；作品标签折叠已完结的前作。数据库迁移 V27（`company_current`），启动时自动执行。
7. 玩家规则（`web/content/rules.md`：公司与营收、续作、退市）。

**要停服约 30 分钟**：打开续作期权会改变每支股票的估值（检验时中位 ×1.21，已公布续作的完结作品最多 ×3.6），要从输入历史重建估值、把股价对齐到新估值，和 v0.8.3 的第 3、4 步相同。注入在之后的第一次日结里发生，发行股数 = 评估值 ÷ 重新定价以来的平均收盘价（`reprice` 会记下 `repriced_after`，不用再手工补）。

**前提**：v0.8.3 已经上线。在 **04:00 换日之后、06:00 采集之前**以外的时间都可以做，但不要和 06:30 的日结重叠。

命令按 fish 写。

## 1. 更新程序，停服，备份

```fish
cd ~/anistock
for line in (string match -rv '^\s*(#|$)' < .env)
    set -l kv (string split -m1 = -- $line)
    set -gx $kv[1] (string trim -c '"\'' -- $kv[2])
end
sqlite3 data/game.db "SELECT key, value FROM state WHERE key='platform_rule'"   # v0.8.3 已上线：应有一行
ANISTOCK_VERSION=v0.8.4 bash deploy/update.sh      # 会重启一次服务，V27 自动执行
systemctl --user stop anistock-server
cp data/game.db data/game-before-v0.8.4-(date +%Y%m%d-%H%M).db
```

## 2. 采集一次（约 15–30 分钟，AniList 限速），重建每日输入

续作期权读每日输入里的关系（`sequel`、`prequel`、`relations_known`），按新的配置重新生成全部输入：

```fish
uv run collect.py
sqlite3 data/anistock.db "SELECT count(*) FROM series_members"
sqlite3 data/anistock.db "SELECT min(day), count(DISTINCT external_id) FROM snapshots WHERE source='anilist' AND kind='relations'"
uv run engine/daily_inputs.py --from 2026-07-01 --day (date -d yesterday +%F) --force
python3 -c "
import json, datetime
d = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()
x = json.load(open(f'data/derived/inputs/{d}.json'))
s = x['shows']
print(d, len(s), '有关系', sum(v['relations_known'] for v in s), '本身是续作', sum(v['prequel'] for v in s), '已公布续作', sum(bool(v.get('sequel')) for v in s), '有评估值', sum('appraisal_att' in v for v in s))"
```

- `series_members` 约 170 行以上；关系快照从 2026-11-06 起。
- 最后一行：几乎所有作品“有关系”，“本身是续作”约 1/4，“已公布续作”十几部，“有评估值”约 100 部。“有关系”明显偏少就不要继续（采集器还没跑完）。

## 3. 从输入历史重建估值，股价对齐估值

```fish
cp data/game.db data/values.db
ANISTOCK_GAME_DB=$PWD/data/values.db ANISTOCK_PORT=18699 ANISTOCK_NO_LOOPS=1 ANISTOCK_VALUATION_REBUILD=1 bin/anistock
```

看到 `valuation: started the filter` 和 `listening on` 后按 Ctrl-C。

```fish
python3 -c "
import sqlite3, statistics as s
a = sqlite3.connect('file:data/game.db?mode=ro', uri=True)
b = sqlite3.connect('file:data/values.db?mode=ro', uri=True)
old = dict(a.execute(\"SELECT id, fair_value FROM companies WHERE status='listed' AND fair_value > 0\"))
new = dict(b.execute(\"SELECT id, fair_value FROM companies WHERE status='listed' AND fair_value > 0\"))
r = sorted(new[i] / old[i] for i in old if i in new)
print(len(r), '只  新/旧估值 中位', round(s.median(r), 3), ' P10', round(r[len(r) // 10], 3), ' P90', round(r[len(r) * 9 // 10], 3), ' 最大', round(r[-1], 3))
t = dict(b.execute('SELECT id, title FROM companies'))
for i in sorted(old, key=lambda i: -new.get(i, 0) / old[i])[:12]:
    print(' ', t[i][:20], round(new.get(i, 0) / old[i], 2))"
bin/anistock reprice data/values.db
rm -f data/values.db data/values.db-wal data/values.db-shm
```

- 新/旧估值中位应在 1.2 左右（检验 1.21，P10 1.05，P90 1.87）；变化最大的应是已公布续作的完结作品（恶女不才、无职转生 第三季、画完这个再去死、猫与龙、碧蓝之海 第三季等，约 ×3）。中位明显偏离 1.2，或者变化最大的不是这类作品，先不要继续。
- `reprice` 的最后一行是新旧价格之比，应和上面的估值比相近。

## 4. 启动（正式数据库上也重建估值），发公告

```fish
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user start anistock-server
journalctl --user -u anistock-server -n 30 --no-pager | grep valuation
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
sqlite3 data/game.db "SELECT key, value FROM state WHERE key='repriced_after'"
python3 -c "
import sqlite3, statistics as s
c = sqlite3.connect('file:data/game.db?mode=ro', uri=True)
r = sorted(f / p for f, p in c.execute(\"SELECT fair_value, last_price FROM companies WHERE status='listed' AND fair_value > 0\"))
print(len(r), '只  估值/现价 中位', round(s.median(r), 3), ' 最小', round(r[0], 3), ' 最大', round(r[-1], 3))"
bin/anistock announce "续作期权与系列前作" docs/ops/deploy-v0.8.4-announcement.md
```

- 日志里应有 `valuation: started the filter from the inputs history`。**启动后一定取消这个环境变量。**
- `repriced_after` 是昨天（`reprice` 写的）。
- 估值/现价中位应接近 1。
- 重建时遇到的、已经公布的续作不发“续作公布”公告，也不收制作费（按已付处理）。之后日结里新公布的才发公告、收制作费。

## 5. 次日日结之后检查

```fish
sqlite3 data/game.db "SELECT kind, count(*) FROM news WHERE day=date('now','-1 day','localtime') GROUP BY kind"
sqlite3 data/game.db "SELECT title FROM news WHERE day=date('now','-1 day','localtime') AND (title LIKE '%注入%' OR title LIKE '%续作%') LIMIT 20"
```

- 注入公告约 100 条（`sequel` 类），每条写明评估值、依据的时间段、发行股数和占比；更名公告约 40 条（`system` 类）。还在“等待”数据的作品之后几天陆续注入。
- 不应有一批“续作公布”公告（第 4 步之后才公布的除外）。
- 网页上看一家注入了前作的股票（例如药屋少女的呢喃）：名字是系列名，作品标签里有“系列共 N 部”，前作折叠成一行。

## 续作覆盖（之后需要时）

只用官方来源。在服务器上：

```fish
uv run engine/sequel_override.py announce <作品 AniList id> --source <官方公告链接> [--premiere 2027-04]
uv run engine/sequel_override.py cancel <作品 AniList id> --source <官方声明链接>
uv run engine/sequel_override.py list
```

下一次 06:00 生成输入、06:30 日结时生效，日结发公告并附来源。作品 id 是系列最新那部作品的（续作是它的续作）。

## 回滚

停服，用第 1 步的备份替换 `data/game.db`，`ANISTOCK_VERSION=v0.8.3 bash deploy/update.sh` 并 `git checkout v0.8.3`，重新运行第 2 步的 `daily_inputs`。注入和配股发生在日结里，之后的交易会随备份一起丢掉。
