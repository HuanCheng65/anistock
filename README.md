# 安易 AniEx

番剧主题的模拟交易游戏（游戏币与真实货币无关）。仓库与服务的内部名称仍为 anistock。

## 数据采集器

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

### 部署

首次安装（在已经跑着采集器的服务器上）：

```bash
cd ~/anistock && git pull
# 采集数据库要是最新的：从开发机拷贝 data/anistock.db 覆盖，或者在服务器上补跑（约 40 分钟）：
#   uv run collect.py --season 2026-summer && uv run collect.py
bash deploy/install.sh
```

`install.sh` 会依次：检查 uv 和 Node.js（20 以上）→ 检查采集数据库是否最新 → 在 `.env` 里生成 `ANISTOCK_SECRET` 和邀请码（已有就不动）→ 安装依赖、构建前端 → 重建市场序列 → 开服（`data/game.db` 已存在就跳过）→ 安装并启动 systemd 服务 → 打印访问地址和邀请码。重复运行是安全的。

日后更新：`bash deploy/update.sh`（拉代码、装依赖、构建前端、重启；数据库迁移在启动时自动执行，执行前先备份）。

### 升级到 v2（Rust 游戏服务）

v2 把游戏服务从 Python 换成了一个 Rust 程序，规则、数据库、接口、登录状态都和 v1 一样：同一个 `data/game.db`，玩家不用重新登录，前端不用改。服务器上不需要装 Rust：每次在 GitHub 上打 `v*` 标签，Actions 会编译好静态链接的程序挂到对应的 Release 上，`update.sh` 从那里下载。

```bash
cd ~/anistock
export https_proxy=http://代理地址:端口        # 服务器访问 GitHub 需要代理时
bash deploy/update.sh                           # 拉代码 → 下载程序 → 第一次切换时先停服并备份数据库 → 启动 v2
```

- 程序装在 `~/anistock/bin/anistock`；默认下载当前检出的标签对应的版本，没有标签就下载最新发布版。指定版本：`ANISTOCK_VERSION=v2.0.0 bash deploy/update.sh`
- 下载后会校验 sha256，不符就不安装；下载失败时服务不受影响
- 第一次从 v1 切过来时，脚本先停服，把数据库备份到 `data/backups/game-before-v2-时间.db`，再启动 v2。v2 启动时会自动接管数据库迁移记录（v1 的 yoyo 记录会被认出来，不会重复执行）
- 采集器、每日结算用的模型数据（`engine/daily_inputs.py`）仍然是 Python，所以 `uv sync` 和前端构建照旧
- 端口、`.env`、反向代理配置都不变；日志：`journalctl --user -u anistock-server -f`

发布新版本（在开发机上）：`git tag v2.0.1 && git push origin v2.0.1`，等 GitHub Actions 的 backend 工作流跑完（几分钟），再在服务器上运行 `update.sh`。

回滚到 v1：

```bash
ANISTOCK_SERVER=v1 bash deploy/update.sh        # 换回 Python 版的服务定义并重启，数据库直接沿用
```

v2 写过的数据库 v1 可以直接读写（表结构相同）。如果想退回切换前的状态，先停服，再把 `data/backups/game-before-v2-*.db` 拷回 `data/game.db`（会丢掉切换之后的所有交易）。

构建前端需要 Node.js 20+；本机没有 Node 但有 Docker 时，脚本会自动用 `node:24-alpine` 容器构建（可用 `NODE_IMAGE=...` 换镜像、`NPM_REGISTRY=https://registry.npmmirror.com` 换 npm 源）。

想在本机装 Node.js 的话（Ubuntu）：

```bash
curl -fsSL https://fnm.vercel.app/install | bash   # 需要代理时先 export https_proxy=...
fnm install 24 && fnm default 24
npm config set registry https://registry.npmmirror.com   # npm 下载慢时
```

游戏服务默认只监听本机 `127.0.0.1:18620`（端口在 `.env` 的 `ANISTOCK_PORT` 里改），对外通过反向代理绑定域名。国内服务器的域名走 80/443 端口需要先备案。

Caddy（自动申请 HTTPS 证书）：

```
stock.example.com {
    encode zstd gzip
    reverse_proxy 127.0.0.1:18620
}
```

Nginx：

```nginx
server {
    listen 443 ssl http2;
    server_name stock.example.com;
    # ssl_certificate / ssl_certificate_key ...

    location / {
        proxy_pass http://127.0.0.1:18620;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        # WebSocket（实时推送）
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```

反代必须和游戏服务在同一台机器上：服务只信任来自 `127.0.0.1` 的 `X-Forwarded-For`，用它识别玩家的真实 IP。

浏览器打开域名，用邀请码注册（邀请码在 `.env` 的 `ANISTOCK_INVITE`）。

- 每天 06:30 服务端自动结算前一天（采集器 06:00 先跑）；服务停过也会在启动后补上错过的日子
- 每天 08:45–09:00 开盘集合竞价，09:00 至次日 04:00 连续交易（交易日在 04:00 切换），可在 `server/game.toml` 里改；交易规则（冷静期、新股认购等）见设计文档
- 新股在“新股认购”页（`/ipo`）认购
- 新一季的番刚上来时显示的是日文名：在开发机上跑一次 `sim/match.py --search` 并提交，就会换成 Bangumi 的中文名
- 更新代码后：`bash deploy/update.sh`（v2 需要下载对应版本的程序，不要只 `git pull` 后重启）
- 重新开服（**会删除所有玩家**）：`uv run python -m server.seed --force`
- 表结构变化用迁移脚本管理（`server/migrations/`，yoyo-migrations）：服务启动时自动执行未执行过的迁移，执行前先把数据库备份到 `data/backups/`。已发布的迁移文件不要再改，有变化就加新文件
- 每天日结后自动备份一次游戏数据库到 `data/backups/`，保留最近 7 天

### 每季开始时（手动）

新番上来后，跨平台匹配需要联网搜索，而且会改动仓库里的文件，所以不在服务器上自动跑：

```bash
uv run sim/match.py --search      # 在开发机上运行
git add sim/matches.json tracked_extra.json && git commit -m "Match new season" && git push
```

服务器 `git pull` 之后，采集器和游戏都会用上新的匹配结果。

### 本地开发

```bash
ANISTOCK_ALWAYS_OPEN=1 uv run uvicorn server.app:app --port 18620 --reload  # 后端，忽略交易时间
cd web && npm run dev                                                       # 前端，/api 代理到 18620
uv run pytest                                                               # 撮合和分红的单元测试
```
