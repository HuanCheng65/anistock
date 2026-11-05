# AniStock 数据采集器

每天抓取一次本季在播番剧在 **Bangumi / AniList / Annict** 上的数据，原样存成 JSON 放进 SQLite。
指标怎么算以后再定，先把数据攒下来（Bangumi 没有历史接口，漏抓的那天就永久缺失了）。

## 采集内容

| 来源 | 发现方式 | 每部番每天存什么 |
|---|---|---|
| Bangumi | `/calendar` 在播列表 | 作品详情（想看/在看/看过/搁置/抛弃、评分分布、tags、infobox）+ 最新 100 集的单集吐槽数 |
| AniList | 本季全部 + 所有连载中（人气 > 1000） | 作品详情（状态分布、评分分布、trending、制作公司、MAL ID）+ 最近 10 天的每日 trends |
| Annict | 本季 + 上季（watchers ≥ 30） | 作品详情（watchers、满意度、MAL ID）+ 最新 100 集的单集观看数和满意度 |

- 番剧从在播列表里消失后（已完结），还会**继续追踪 180 天**，用来记录完结后的长尾数据
- 同一天重复运行会覆盖当天的数据，不会产生重复行
- “天”按 **Asia/Shanghai** 时区划分

## 部署（Ubuntu + uv）

用自己的账户部署即可，全程不需要 sudo。

```bash
git clone https://github.com/HuanCheng65/anistock.git ~/anistock
cd ~/anistock
cp .env.example .env && chmod 600 .env
nano .env                                         # 填入 ANNICT_TOKEN 和代理地址

# 先手动跑一次，顺便回填 AniList 历史数据（大约 40 分钟，建议在 tmux 里跑）
uv run collect.py --backfill                      # 脚本会自动读取同目录下的 .env
uv run collect.py --stats
```

### 定时任务：每天 06:00（北京时间）

**方式 A：systemd 用户定时器（推荐，错过的运行会在开机后补跑）**

```bash
which uv                                          # 如果不是 ~/.local/bin/uv，修改 deploy/anistock-collect.service
mkdir -p ~/.config/systemd/user
cp deploy/anistock-collect.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now anistock-collect.timer
loginctl enable-linger "$USER"                    # 让定时器在你没登录 SSH 时也照常运行
systemctl --user list-timers                      # 查看下次运行时间
journalctl --user -u anistock-collect -n 50       # 查看日志
```

如果 `loginctl enable-linger` 提示权限不足，就让管理员执行一次 `sudo loginctl enable-linger 你的用户名`，或者改用方式 B。

**方式 B：cron（最简单）**

`crontab -e`，加入一行（`uv` 换成 `which uv` 的完整路径）：

```
0 6 * * * cd $HOME/anistock && $HOME/.local/bin/uv run collect.py >> collect.log 2>&1
```

cron 按服务器本地时区执行，先用 `timedatectl` 确认时区是 Asia/Shanghai，不是的话换算一下时间。

## 代理

- 三个来源都走 `HTTPS_PROXY`（国内服务器直连 api.bgm.tv 也不稳定）。`.env` 会被脚本自动读取，fish、bash 等 shell 都能直接 `uv run collect.py`
- 第一次运行 uv 需要从 PyPI 下载 httpx。如果很慢，在 `.env` 里设置 `UV_INDEX_URL` 使用清华镜像
- 代理是 socks5 也可以（已包含 `httpx[socks]`），例如 `HTTPS_PROXY=socks5://127.0.0.1:7891`

## 命令

```
uv run collect.py                    # 全部来源
uv run collect.py --source bgm       # 只抓一个来源：bgm / anilist / annict
uv run collect.py --backfill         # 额外回填 AniList 的每日历史（从上一季开始）
uv run collect.py --stats            # 查看数据量和最近几次运行情况
```

任一来源失败时，退出码为 1，systemd 会把这次运行标记为 failed。没设置 `ANNICT_TOKEN` 时，会直接跳过 Annict，不算失败。

## 数据表

- `works`：追踪中的作品（来源、ID、标题、MAL ID、首次发现日期、最后一次出现在在播列表的日期）
- `snapshots`：每天的原始 JSON（`source, kind, external_id, day` 唯一）
- `anilist_trends`：AniList 每日 trends（`media_id, date_epoch` 唯一）
- `runs`：每次运行的记录

查询示例（SQLite 自带 JSON 函数）：

```sql
SELECT day, json_extract(data,'$.collection.doing') AS doing,
            json_extract(data,'$.collection.dropped') AS dropped
FROM snapshots WHERE source='bgm' AND kind='subject' AND external_id='418229' ORDER BY day;
```

## ⚠️ 数据不要公开

Annict API 条款第 6 条：**不能把抓到的原始数据放在公开仓库或公开链接里**。`data/` 和 `*.db` 已经写进 `.gitignore`。
游戏网站展示算出来的统计结果（营收、股价）没问题；不要提供原始数据下载。

## 仿真（sim/）

用采集到的数据回放一个番剧股票经济：三个市场的营收、分红估值、股价、税收、央行利率。参数集中在 `sim/params.toml`。

```bash
uv run sim/match.py --search             # 跨平台作品匹配 -> sim/matches.json（手工修正写在 manual 里）
uv run sim/reconstruct.py airtimes       # Annict 每集播出时间（约 10 次请求）
uv run sim/reconstruct.py calibrate      # 累积曲线（一次性，约 160 次请求，需要 ANNICT_TOKEN）
uv run sim/reconstruct.py build          # 重建中国/日本市场的每日序列
uv run sim/simulate.py                   # -> sim/out/result.json
uv run sim/report.py                     # -> sim/out/report.html
```

- 换参数跑对照方案：`uv run sim/simulate.py --set central_bank.max_step=0.01 --out sim/out/result_fastcb.json`
- `data/derived/` 和 `sim/out/` 是本地生成的，不进 git
- `sim/reconstruct.py check` 会把重建结果和 AniList 做交叉验证

## 游戏网站（server/ + web/）

规则和设计依据见 [`docs/game-design.md`](docs/game-design.md)。

### 部署（在服务器上按顺序做，每一步都不能跳过）

```bash
cd ~/anistock
git pull

# 1. 采集数据库要是最新的：夏季番回填、补充的 Bangumi 条目、海贼王新篇章都在里面。
#    二选一：从开发机拷贝 data/anistock.db 过来覆盖；或者在服务器上补跑（约 40 分钟）：
#    uv run collect.py --season 2026-summer && uv run collect.py

# 2. .env 里填上 ANISTOCK_SECRET（随机串）和 ANISTOCK_INVITE（社团邀请码），见 .env.example

# 3. 安装依赖、重建市场序列（只发约 10 次 Annict 请求）
uv sync
uv run sim/reconstruct.py airtimes
uv run sim/reconstruct.py build

# 4. 构建前端（需要 Node 20+；npm 慢的话：npm config set registry https://registry.npmmirror.com）
cd web && npm ci && npm run build && cd ..

# 5. 开服：用 7 月以来的数据回放生成历史行情（会创建 data/game.db）
uv run python -m server.seed

# 6. 常驻运行（systemd 用户服务，端口 8080）
cp deploy/anistock-server.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now anistock-server
journalctl --user -u anistock-server -n 50 --no-pager
```

浏览器打开 `http://服务器地址:8080`，用邀请码注册。

- 每天 06:30 服务端自动结算前一天（采集器 06:00 先跑）；服务停过也会在启动后补上错过的日子
- 交易时间 09:00–23:00，可在 `server/game.toml` 里改
- 更新代码后：`git pull && uv sync && (cd web && npm ci && npm run build) && systemctl --user restart anistock-server`
- 重新开服（**会删除所有玩家**）：`uv run python -m server.seed --force`
- 目前没有数据库迁移：如果某次更新改了游戏数据库的表结构（提交说明里会写），旧的 `data/game.db` 用不了，需要重新开服。开服后真正有人玩了，再加迁移脚本

### 每季开始时（手动）

新番上来后，跨平台匹配需要联网搜索，而且会改动仓库里的文件，所以不在服务器上自动跑：

```bash
uv run sim/match.py --search      # 在开发机上运行
git add sim/matches.json tracked_extra.json && git commit -m "Match new season" && git push
```

服务器 `git pull` 之后，采集器和游戏都会用上新的匹配结果。

### 本地开发

```bash
ANISTOCK_ALWAYS_OPEN=1 uv run uvicorn server.app:app --port 8080 --reload   # 后端，忽略交易时间
cd web && npm run dev                                                       # 前端，/api 代理到 8080
uv run pytest                                                               # 撮合和分红的单元测试
```
