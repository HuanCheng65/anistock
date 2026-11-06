# 长篇连载估值修正（v0.6.7 之后，同一晚）

## 原因

v0.6.7 重建了中国、日本序列。长篇连载的序列来自 Bangumi、Annict 的分集页面，很多作品的分集早已不更新（吉伊卡哇的到 2025 年 5 月为止），新序列把这些日子记为缺失；缺失的市场由滤波按“欧美水平 → 中国、日本水平”的关系推算，并按推算值记营收。这个关系是在新番开播头几周上拟合的，用在长篇连载上没有依据，几部作品被高估了数倍（吉伊卡哇 2.02 → 16.14，斗罗大陆2 0.51 → 6.77）。

修正（`engine/daily_inputs.py` `airing_sources`）：仍在播出的作品（长篇连载，或已播集数少于公布集数），首播满 28 天、某个市场最近 28 天没有任何数据时，这一天在这个市场按没有数据源处理（不计营收）。已完结的作品不变（完结后的缺失由长尾模型处理）。

本地用同样的 seed 和输入复现了线上：改价前、改价后的估值都和线上一致。修正后相对现在，只有 7 支股票变化，都回到改价前附近；其余 135 支变化在 1% 以内（见下面第 3 步的核对）。

## 步骤

约 10 分钟。06:30 日结之前做完，**而且必须做完第 1 步**：日结会用服务器上的 `engine/daily_inputs.py` 生成当天的输入，旧代码会让这几支股票重新被高估。

```bash
cd ~/anistock
set -a; source .env; set +a
```

### 1. 更新代码，停服

```bash
ANISTOCK_VERSION=v0.6.7 bash deploy/update.sh
systemctl --user stop anistock-server
```

只有 Python 代码变了，服务程序还是 v0.6.7。

### 2. 重新生成每日输入

```bash
uv run sim/reconstruct.py airtimes
uv run sim/reconstruct.py build
uv run engine/daily_inputs.py --from 2026-07-01 --day 2026-11-05 --force
python3 -c "
import json
s={x['id']:x for x in json.load(open('data/derived/inputs/2026-11-05.json'))['shows']}
for i in (140842, 137683, 137653): print(i, s[i]['sources'])"
```

三行的 `cn` 都应该是 `False`（140842 的 `jp` 也是 `False`）。

### 3. 在副本上重建估值，核对

```bash
cp data/game.db data/values.db
ANISTOCK_GAME_DB=$PWD/data/values.db ANISTOCK_VALUATION_REBUILD=1 ANISTOCK_NO_LOOPS=1 ANISTOCK_PORT=18699 bin/anistock
```

看到 `valuation: started the filter from the inputs history` 后按 Ctrl-C，然后核对：

```bash
python3 -c "
import sqlite3
c=sqlite3.connect('file:data/values.db?mode=ro',uri=True)
want={140842:2.02, 137683:0.50, 137653:1.61, 8687:0.39, 165159:0.68, 118123:0.65, 21:None, 178789:111.41}
for i,w in want.items():
    v=c.execute('SELECT fair_value FROM companies WHERE id=?',(i,)).fetchone()[0]/100
    print(i, v, '' if w is None else ('OK' if abs(v/w-1)<0.05 else '不一致，停下'))"
```

除 21（航海王，只看数值）以外都应该是 `OK`。本地的数字来自 11 月 5 日的输入；服务器如果已经多结算了一天，会略有不同，差别超过 5% 就停下来，把输出发出来再说。

### 4. 撤单，按新估值调整价格

```bash
bin/anistock reprice data/values.db
rm -f data/values.db*
```

新价 ÷ 旧价的中位数应该约为 1.01，P10、P90 也在 1 附近（只有 7 支大幅下调）。

其中『HOLOLIVE ALTERNATIVE』先导PV 和两部成人向作品（179823、212667）不该上市：v0.6.6 重开时 `server.seed` 没有排除它们，06:30 的日结会按最后成交价收回并下市（`backend/src/daily.rs` 第 0 步，同时发公告）。之后的 seed 会直接跳过它们。

### 5. 发公告，开服

```bash
bin/anistock announce "长篇连载估值修正" docs/revalue-long-runners-announcement.txt
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user start anistock-server
journalctl --user -u anistock-server -n 30 --no-pager
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
```

**一定要取消这个环境变量。**

### 6. 检查

```bash
python3 -c "
import sqlite3,statistics as s
c=sqlite3.connect('file:data/game.db?mode=ro',uri=True)
r=sorted(f/p for f,p in c.execute(\"SELECT fair_value,last_price FROM companies WHERE status='listed' AND fair_value>0\"))
print(len(r),'只  估值/现价 中位',round(s.median(r),3),' 最小',round(r[0],3),' 最大',round(r[-1],3))"
```

## 回滚

`data/backups/game-before-reprice-<时间>.db`，方法同 docs/revalue-v0.6.7.md。
