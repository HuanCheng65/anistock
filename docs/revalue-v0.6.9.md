# v0.6.9：开播头四周的衰减前快后慢

## 原因

估值的漂移原来对开播第 0–3 周用同一个数（欧美每天 −0.019）。实际的衰减前快后慢（历史 32 季和游戏两季都是这样），估值在第 1 周可以预期地下修、第 3 周可以预期地上修。v0.6.9 把这四周拆开，每周一个数，并重新拟合了第 1–3 周、联合先验和欧美周内观测（docs/v2-valuation.md §3.13）。日本的衰减原来是在放送时间错误的旧序列上拟合的，按修正后的序列放缓到原来的一半左右，日本的先验也随之变化。

## 同版本的其他改动

v0.6.9 同时包含热度快讯的修复（docs/v2-api-access.md §7.1，docs/v2-valuation.md §3.4，docs/v2-npc-market.md §14）：

- 快讯的观测噪声重新拟合；日结时先撤掉当天快讯对估值的修正，再按整天的数据计算，同一集不重复计算。
- 做市商随估值修正立即调整报价（`server/game.toml` 的 `mm_follow_revisions = true`）。

对本次操作没有影响：重建估值只读每日输入，不读快讯，第 2 步的核对比例不变；做市商记录的上一次估值只在内存里，重启后从空开始，重建不会被当作一次修正，价格不动；v0.6.8 留下的快讯记录在下一次日结时只标记为已结算，不会再扣一次。

## 处理方式

和日本市场数据修正（docs/revalue-airtimes.md）一样：只重建隐藏估值，不调整价格，不撤单。

本地复现（`sim/replica_check.py`，11 月 5 日的输入，新旧参数）：139 支中位数 ×0.994，P10 0.985，P90 1.058。

| 作品 | 比例 | 原因 |
|---|---|---|
| 蜡笔小新（966） | ×0.62 | 只接入日本市场的长篇，日本数据时断时续，水平主要来自先验；新的日本先验在欧美热度低的作品上低约三分之一 |
| 星球大战：幻境 - 第九位绝地武士（213847） | ×0.69 | 低于 0.3 的股票 |
| 赛博朋克：边缘行者 2（195539） | ×0.91 | 一次放出全季，第 1–3 周衰减更快 |
| 正能量企鹅（185646） | ×1.30 | 低于 1.1 的股票 |
| 暗机 DARK MACHINE（179876） | ×1.11 | |

秋季在播作品大多上调 5–10%（第 1、2 周衰减更快，第 3 周以后更慢，后者影响更大）。

## 步骤

约 10 分钟，避开 06:30 的日结。需要 v0.6.9 的发布版本已经构建完成（GitHub Actions）。

```bash
cd ~/anistock
set -a; source .env; set +a
```

### 1. 更新代码和服务程序，停服

```bash
ANISTOCK_VERSION=v0.6.9 bash deploy/update.sh
systemctl --user stop anistock-server
```

`update.sh` 会启动一次新版本，马上停掉即可。

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
want={966:0.62, 195539:0.91, 179876:1.11}
for i,w in want.items(): print(i, round(r[i],2), 'OK' if abs(r[i]/w-1)<0.12 else '不一致，停下')
odd=[(i,round(x,2),old[i]/100) for i,x in r.items() if (x>1.5 or x<0.67) and i not in want and old[i]>=30]
print('其他变化超过 1.5 倍的：', odd or '无')"
```

中位数应在 0.98–1.02 之间，三行都是 `OK`，最后一行是 `无`。服务器比本地多结算了两天，比例会略有不同；差别超过 12% 或出现其他大幅变化就停下来，把输出发出来再说。

### 3. 备份，带重建标志启动

```bash
rm -f data/values.db*
sqlite3 data/game.db ".backup data/backups/game-before-v069-$(date +%Y%m%d-%H%M).db"
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user start anistock-server
journalctl --user -u anistock-server -n 30 --no-pager
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
```

日志里应有 `valuation: started the filter from the inputs history`。**一定要取消这个环境变量。**

### 4. 发公告

```bash
bin/anistock announce "估值模型更新" docs/revalue-v0.6.9-announcement.txt
```

## 回滚

停服，用第 3 步的备份替换 `data/game.db`，`ANISTOCK_VERSION=v0.6.8 bash deploy/update.sh`。
