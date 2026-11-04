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

```bash
sudo useradd -r -m -d /srv/anistock anistock      # 或者用你自己的用户
sudo cp collect.py .env.example /srv/anistock/
sudo cp .env.example /srv/anistock/.env && sudo chmod 600 /srv/anistock/.env
sudo -e /srv/anistock/.env                        # 填入 ANNICT_TOKEN 和代理地址
sudo chown -R anistock: /srv/anistock

# 先手动跑一次，顺便回填 AniList 历史数据（大约 10 多分钟）
sudo -u anistock bash -c 'cd /srv/anistock && set -a && . ./.env && set +a && uv run collect.py --backfill'
sudo -u anistock bash -c 'cd /srv/anistock && uv run collect.py --stats'

# 定时任务：每天 06:00（北京时间）
which uv                                          # 如果不是 /usr/local/bin/uv，修改 service 文件
sudo cp deploy/anistock-collect.{service,timer} /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now anistock-collect.timer
systemctl list-timers anistock-collect.timer      # 查看下次运行时间
journalctl -u anistock-collect -n 50              # 查看日志
```

用 cron 也可以：

```
0 6 * * * cd /srv/anistock && set -a && . ./.env && set +a && uv run collect.py >> collect.log 2>&1
```

（cron 按服务器本地时区执行，要确认服务器时区是 Asia/Shanghai。）

## 代理

- AniList、Annict 在海外，走 `HTTPS_PROXY`；Bangumi 在国内，放进 `NO_PROXY` 直连
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
