# 安易 AniEx v2 架构

v2 把游戏服务器从 Python（FastAPI）换成 Rust，目标是：撮合常驻内存、行情实时推送、能承载几百个 NPC 每秒行动，同时**规则和数据与 v1 完全一致**，可以在同一个数据库上直接替换 v1。

语言选择的依据见 `bench/`（2 核下 Python 约 3 倍负载到顶；Go、Rust 在 10 倍内相当，20 倍以上 Rust 余量约多 1.5 倍、内存约少一半）。

---

## 1. 分工：Python 算数据，Rust 管钱

| 部分 | 语言 | 说明 |
|---|---|---|
| 采集器 `collect.py` | Python | 不变，每天 06:00 运行 |
| 经济模型 `engine/economy.py`、`sim/` | Python | 不变。模型和仿真、校准共用一份代码，不在 Rust 里复制一份（复制后每次重新校准都会不一致） |
| **每日输入** `engine/daily_inputs.py`（新） | Python | 日结时由 Rust 调起，把模型对某一天的输出写成一个 JSON 文件（字段见第 3 节） |
| 开服 `server/seed.py` | Python | 不变。新世界由 Python 生成数据库，Rust 读取 |
| **游戏服务器** `backend/`（新） | Rust | 撮合、账户与账本、NPC、日结中所有资金流动、估值公式、HTTP / WebSocket、认证 |
| 前端 `web/` | Vue | 第 1 步不改；之后把轮询换成 WebSocket 推送 |

Rust 只移植**把模型输出和游戏状态结合起来**的那部分公式，集中在 `engine/valuation.py`：`blended_share`、`expected_share`（预期份额的分母）、`demand_day`/`demand_week`、`margin`、`fair_value_cents`、`fundamental`、`finished`（函数体合计约 80 行）。

## 2. 进程内结构

```
            ┌──────────── tokio（2 个工作线程）────────────┐
 浏览器 ──► │ HTTP / WebSocket（axum） │ 认证 │ 限流      │
 QQ 机器人 ►│ 读接口：从内存快照或 SQLite 只读连接取数    │
            └──────┬─────────────────────────────▲─────────┘
                   │ 命令（下单/撤单/认购…）      │ 行情（合并后的盘口、快照）
            ┌──────▼──────────────┐       ┌──────┴──────┐
            │ 引擎线程（唯一写者） │──────►│ 行情分发     │──► 各连接的发送队列（满 256 条断开）
            │ 订单簿 撮合 竞价 VCM │       └─────────────┘
            │ 账户 账本 持仓批次   │──────► 持久化线程：每 50ms 一批事务写入 SQLite（WAL）
            │ NPC 日结 新股认购    │
            └─────────────────────┘
```

- **引擎线程**独占全部可变状态，没有锁；所有修改都通过命令队列进入，测试可以直接驱动它。
- **持久化**：引擎把每个变化（订单、成交、账本分录、持仓批次…）交给持久化线程，每 50ms 合成一个事务提交。崩溃最多丢最近 50ms 内已经回复给玩家的操作（游戏币可以接受；不能接受的话可改成提交后再回复，延迟加 50ms 以内）。
- **启动**：从 SQLite 把账户、持仓、批次、挂单、公司状态全部读进内存，然后先检查一遍不变量（现金总和为 0 等），不通过就拒绝启动。
- **行情**沿用压测里验证过的做法：每支股票的盘口最多每 100ms 推一次最新状态；全市场快照只发有变化的股票，并分散在一秒内发出；同一连接积压的消息合成一帧（按行分隔的 JSON）。
- **NPC 的动作均匀分散**在时间上，不集中在同一时刻（压测中 10 倍负载 p99 50ms 的主要原因就是集中触发）。

## 3. Python 与 Rust 的接口：每日输入文件

日结 D 日时，Rust 运行 `uv run engine/daily_inputs.py --day D`，读取 `data/derived/inputs/D.json`（`--out` 或环境变量 `ANISTOCK_INPUTS_DIR` 可改位置）。v1 的日结（`server/daily.py`）已经改为只从这个文件取模型数据，两边读的是同一份输入。字段的权威说明在 `engine/daily_inputs.py` 的文档字符串里：

```json
{
  "day": "2026-11-04",
  "window_start": "2026-07-01",
  "cn_titles": {"171018": "中文名", "…": "…"},
  "shows": [
    {
      "id": 171018,
      "title_ja": "…", "title_en": "…", "studio": "…",
      "first_air": "2026-10-03", "episodes": 12, "episodes_known": true, "long_runner": false,
      "popularity_end": 52000,
      "aired": 5, "has_real_data": true,
      "att":      {"us": 0.0, "cn": 0.0, "jp": 0.0},
      "expected": {"us": 0.0, "cn": 0.0, "jp": 0.0}
    }
  ]
}
```

- `shows` 是**数组，顺序有意义**：新股按这个顺序创建，股票代码按创建顺序编号。包含模型范围内已播出的番和 `ipo_subscription_days + 1` 天内要上市的番。
- `att`：D 日实际热度（营收份额、需求指数）；`expected`：预期热度（新股定价、估值和预期份额的分母）。
- `episodes`：估值用的集数，未知时为默认集数（`episodes_known = false`，公司表里存 null）；只有长篇且无集数时为 null。`aired`：截至 D 日已播集数。`popularity_end`：上市门槛 `min_listing_popularity` 用。
- `cn_titles`：所有匹配到的中文名（不只是 `shows` 里的），日结用它给任何同 id 的公司改名（含已退市的）；新公司取 `cn_titles[id]`，没有则用 `title_ja`。
- **不在文件里**：需求指数（由*已上市*公司的 `att.us` 之和与游戏状态里的 `a_ref` 算出，`demand_day = base/7 * x` 与 `demand_week = base * x` 是两个公式）、市场权重（配置）、利率、价格。
- Rust 要移植的纯公式都在 `engine/valuation.py`（无 I/O）。浮点求和顺序必须一致：营收的 `totals` 按 `SELECT * FROM companies WHERE status='listed'` 的行序求和；预期份额的分母按同样的已上市顺序求和，未上市的番最后加入。
- 文件确定性：同样的采集数据、市场序列和参数得到相同字节；留在 `data/` 下（不进仓库），同时是日结的复盘记录。
- 差分对比工具：`uv run python -m server.tools.compare_close A.db B.db`。

## 4. 数据库与迁移

- **沿用 v1 的 SQLite 表结构**，v2 直接使用同一个 `data/game.db`。
- 迁移改用 Rust 库 `refinery`（rusqlite，内置 SQLite，不依赖服务器上的 libsqlite）。第一次启动时，如果发现 v1 的迁移记录表（`_yoyo_migration`），就把 `001_initial` 标记为已执行，然后从 `002` 开始执行新迁移。迁移前照旧自动备份。
- 以后的表结构变化（会话、API 令牌、机器人绑定等）都写成 `002+` 迁移。

## 5. 与 v1 的兼容（替换当天不能出问题的地方）

- **密码**：v1 用 scrypt（N=2¹⁴, r=8, p=1，`盐hex:哈希hex`）。Rust 必须能验证同样的格式，否则所有老玩家都登不上。用 Python 生成的哈希做测试。
- **登录状态**：第 1 步继续识别 v1 的签名 cookie（同一个 `ANISTOCK_SECRET`），替换后玩家不用重新登录。第 3 步再换成服务器端会话。
- **接口**：第 1 步保持所有 `/api/*` 路径和返回的 JSON 完全一致，现在的前端不用改就能用。
- 端口（18620）、`.env`、反向代理配置都不变。

## 6. 测试

1. **移植现有 15 个测试**，作为 Rust 单元测试。
2. **差分测试（重点）**：同一串订单序列分别喂给 v1 的 `server/exchange.py` 和 Rust 引擎，比较成交、余额、持仓批次、VCM 状态是否完全一致；日结同理：在同一个数据库副本上分别跑 v1 和 v2 的日结，账本分录必须一致。这一步能抓出取整、先进先出顺序、VCM 基准价这类细节差异。
3. **压测**：用 `bench/loadgen` 对整个服务器做验收。

## 7. 部署

服务器上不装 Rust。用 GitHub Actions 编译静态链接的 Linux 程序（`x86_64-unknown-linux-musl`），发布到 GitHub Releases；`deploy/update.sh` 下载对应版本，替换后重启。Python 部分（采集器、每日输入）和前端构建照旧。

## 8. 分步

| 步骤 | 内容 | 交付物 |
|---|---|---|
| 1 | 引擎、账本、日结、存储、迁移、接口兼容 | 可以直接替换 v1，行为一致 |
| 2 | 多主体 NPC 市场、盘中消息（播出时间、热度快讯） | 市场有动静，离线仿真校准 |
| 3 | 服务器端会话、API 令牌、开放注册、风控 | 去掉邀请码 |
| 4 | QQ 机器人（Koishi 插件、绑定） | 群内查行情、下单 |
| 5 | 前端改用 WebSocket 推送；整机压测；上线 | v2 正式替换 |

第 1 步完成后就可以先上线（玩家无感知），后面每一步都单独上线。

## 9. 不变的约定

- 时区 Asia/Shanghai；时钟可以在测试里注入。保留 `ANISTOCK_ALWAYS_OPEN`、`ANISTOCK_NO_LOOPS` 这类开发开关。
- 日结按 `last_closed` 幂等，停机后自动补跑；每天备份，保留 7 份。
- Annict 令牌只放在 `.env`；`data/` 和数据库永远不提交。
- 游戏币和真实货币没有任何对应关系。
