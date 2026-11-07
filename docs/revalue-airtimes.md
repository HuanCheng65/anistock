# 日本市场放送时间修正（2026-11-07）

## 原因

日本市场的热度来自 Annict 各集的观看记录，按每集的播出时间排到日期上（`sim/reconstruct.py`）。播出时间取自 Annict 的放送列表，原来只读最近的 150 条：一集在多个电视台播出时，最早那个台的记录已经不在这 150 条里，留下的是其他台后来的播出时间。15 部作品的部分集因此晚了几周到两个月：

- 《擅长逃跑的殿下 第二季》《恶女不才，请多关照 ～雏宫蝶鼠换身传～》9 月底完结，但最后几集被排到 10、11 月，完结后的一个月仍按播出期计热度，日本热度是 Annict 实际观看记录的 7–10 倍，估值约高一倍；
- 《宝可梦 地平线》9 月的几集被排到 10 月，9 月没有日本数据；
- 几部作品的日本数据晚一个多月才开始。

同时修正（`engine/daily_inputs.py`）：

- 一次放出多集的作品（Bangumi 排期上同一天有两集以上：《赛博朋克：边缘行者 2》、《星球大战：幻境 - 第九位绝地武士》、首播连播三集的作品），放出当天全部计为已播。AniList 只记第一集，作品一直显示在播。
- “播出中、某个市场 28 天没有数据就不计这个市场”的规则，改为还要求这期间播出了至少 2 集，并且这个市场以前有过数据。停播期间不再被关掉（《死神 千年血战篇-祸进谭-》10 月停播一个月，估值先降到三分之一、再涨回 3.7 倍），从未有过数据的新番也不再被关掉。
- 先行放送（首播前两周以内）的播出时间按首播日计。

## 处理方式

只重建隐藏估值，不调整价格。价值型 NPC 按新估值交易，价格由市场决定，相当于这几支股票出了重大利空。不撤单。

本地复现（`sim/replica_check.py`，11 月 5 日的输入）的变化：139 支中位数 ×1.009。

| 作品 | 估值 | 修正后 | 比例 |
|---|---|---|---|
| 擅长逃跑的殿下 第二季 | 45.24 | 17.93 | ×0.40 |
| 恶女不才，请多关照 ～雏宫蝶鼠换身传～ | 57.68 | 29.43 | ×0.51 |
| 穹庐下的魔女 | 44.72 | 35.19 | ×0.79 |
| 炒翻天 | 15.95 | 13.19 | ×0.83 |
| 宝可梦 地平线 | 9.98 | 8.83 | ×0.88 |
| 转生哥布林有问题吗？ | 11.75 | 10.08 | ×0.86 |
| 航海王 | 78.00 | 84.79 | ×1.09 |
| 和没有信徒的女神大人一起攻略异世界 | 9.33 | 10.63 | ×1.14 |

另有两支低于 0.3 的股票变化超过 1.5 倍（游戏王卡片游戏 故事编年史 0.03 → 0.11，星球大战：幻境 0.08 → 0.29），其余变化在 10% 以内。

## 步骤

服务不用停，只在第 3 步重启一次（约 1 分钟）。避开 06:30 的日结。

```bash
cd ~/anistock
set -a; source .env; set +a
git pull --ff-only
```

### 1. 重新生成数据

```bash
uv run sim/reconstruct.py airtimes
uv run sim/reconstruct.py build
uv run engine/daily_inputs.py --from 2026-07-01 --day 2026-11-06 --force
```

`airtimes` 现在读完整的放送列表，约 2–3 分钟。`--day` 是最近一次日结的日期（11 月 7 日 06:30 之后是 2026-11-07）。

```bash
python3 -c "
import json
s={x['id']:x for x in json.load(open('data/derived/inputs/2026-11-06.json'))['shows']}
print(s[195539]['aired'], s[185874]['sources'])"
```

应该是 `10 {'us': True, 'cn': True, 'jp': True}`。

### 2. 在副本上重建估值，核对

```bash
sqlite3 data/game.db ".backup data/values.db"
ANISTOCK_GAME_DB=$PWD/data/values.db ANISTOCK_VALUATION_REBUILD=1 ANISTOCK_NO_LOOPS=1 ANISTOCK_PORT=18699 bin/anistock
```

看到 `valuation: started the filter from the inputs history` 后按 Ctrl-C，然后：

```bash
python3 -c "
import sqlite3, statistics as s
new={i:v for i,v in sqlite3.connect('file:data/values.db?mode=ro',uri=True).execute(\"SELECT id,fair_value FROM companies WHERE status='listed'\")}
old={i:v for i,v in sqlite3.connect('file:data/game.db?mode=ro',uri=True).execute(\"SELECT id,fair_value FROM companies WHERE status='listed'\")}
r={i:new[i]/old[i] for i in old if old[i]>0 and new.get(i)}
print(len(r),'只  中位',round(s.median(r.values()),3))
want={182616:0.40, 188139:0.51, 190569:0.79, 204060:0.83, 21:1.09}
for i,w in want.items(): print(i, round(r[i],2), 'OK' if abs(r[i]/w-1)<0.1 else '不一致，停下')
odd=[(i,round(x,2),old[i]/100) for i,x in r.items() if (x>1.5 or x<0.67) and i not in want and old[i]>=30]
print('其他变化超过 1.5 倍的：', odd or '无')"
```

五行都应该是 `OK`，最后一行是 `无`。服务器多结算了一两天，比例会和本地略有不同，差别超过 10% 或出现其他大幅变化就停下来，把输出发出来再说。

### 3. 备份，带重建标志重启

```bash
rm -f data/values.db*
sqlite3 data/game.db ".backup data/backups/game-before-revalue-$(date +%Y%m%d-%H%M).db"
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user restart anistock-server
journalctl --user -u anistock-server -n 30 --no-pager
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
```

日志里应有 `valuation: started the filter from the inputs history`。**一定要取消这个环境变量。**

### 4. 发公告

```bash
bin/anistock announce "日本市场数据修正" docs/revalue-airtimes-announcement.txt
```

## 回滚

停服，用第 3 步的备份替换 `data/game.db`，`git checkout 556871b -- engine/daily_inputs.py sim/reconstruct.py` 后重新执行第 1 步，再启动。
