# 安易 AniEx 公开接口 v1（设计）

状态：**已实现（v0.4.0）**。决定记录见 §8，实现时和设计不同的地方见 §9。对象：网页前端（`web/`）和以后的 QQ 机器人（Koishi 插件）。机器可读的版本由代码生成：服务器上的 `/api/v1/openapi.json`，仓库里的 `web/src/api/openapi.json`（`cd web && npm run gen:api` 重新生成，同时生成 TS 类型 `web/src/api/schema.d.ts`）。评审用的手写 yaml 已删除。

> 命名提醒：本文的 **API v1** 指新的 `/api/v1/*` 接口。它和“v1 Python 服务器”（`server/app.py`，FastAPI）不是一回事。下文把后者称为 **Python 服务器**，把现在 `/api/*` 下逐字节兼容它的 21 个接口称为**旧接口**，`/api/v2/*` 和 `/api/covers` 称为 **v2 只读接口**。

---

## 0. 一页总览

### 0.1 决定

| # | 决定 | 结论 | 理由（一句话） |
|---|---|---|---|
| D1 | **停用 Python 回退路径** | **已确认**停用。不再保证能把数据库交回 `server/app.py` 运行 | V2/V3 迁移（估值状态）、NPC 市场、破产重整（V4）都是 Rust 独有的，Python 服务器已经不能在当前数据库上正确运行；回退路径名存实亡，却让每个接口背着“和 FastAPI 逐字节一致”的包袱 |
| D2 | 金额表示 | **整数分**（JSON number，字段名不加后缀） | 账本本来就是整数分；JS 的整数在 2⁵³ 内精确；前端和机器人只在显示时除以 100 |
| D3 | 版本与路径 | `/api/v1/*`，OpenAPI 里列出的是稳定接口，其余都是内部接口 | 一个干净的起点，以后的不兼容改动走下一个大版本 |
| D4 | 错误格式 | `{"code","message","details"}`，`message` 是可以直接显示的中文 | 前端的 `MESSAGES` 改写表和 422 翻译表随之删除，机器人拿到同样的文案 |
| D5 | 交易日 | 每个按天归属的对象都带 `trading_day`（04:00 换日），时间戳一律 ISO 8601 带 `+08:00` | 夜盘到 04:00，自然日和交易日在 00:00–04:00 不同 |
| D6 | 分页 | 会增长的列表用游标（`before` 往旧翻，`after` 补新的），固定规模的列表不分页 | 断线重连后用 `after` 补齐漏掉的成交、资讯、流水 |
| D7 | 机器人认证 | 服务器级机器人令牌 + `X-Act-As: qq:<QQ号>` 代表已绑定的玩家；v1 只开放只读权限 | 交易权限（`trade`）预留，不在 v1 |
| D8 | 绑定 | 网页生成一次性绑定码，私聊机器人发 `/安易 绑定 <码>`；网页或 `/安易 解绑` 解除 | 证明“这个 QQ 的主人能登录这个账号”，不需要在 QQ 里输密码 |
| D9 | 实时推送 | 以正在开发的 `/api/ws` 为基础，只改一致性所需的地方（§5.5 列出差异）；机器人推送也走 WebSocket | 推送设计已压测验证（`bench/SPEC-v2.md`） |
| D11 | 迁移方式 | **一次切换**：同一个版本里上线 v1、迁完前端、删除旧接口和兼容代码（§7） | 只有自己的前端在用，并存期只有成本 |
| D12 | 资产快照 | 本次就建每日资产快照表（周榜、季榜、资产曲线） | §4.5.10 |
| D10 | 类型 | Rust 端用 utoipa 从处理函数生成 OpenAPI，前端和机器人用 `openapi-typescript` 生成 TS 类型 | 新接口本来就要改用 serde 结构体，一次标注两边受益 |

### 0.2 接口一览

| 分组 | 方法 路径 | 认证 | 取代 |
|---|---|---|---|
| 市场 | `GET /api/v1/market/status` | 无 | `/api/status`（动态部分） |
| | `GET /api/v1/rules` | 无 | `/api/status` 的 `rules` 等静态部分 |
| | `GET /api/v1/market/quotes` | 无 | `/api/market` + `/api/v2/ratios` |
| | `GET /api/v1/market/indices` | 无 | `/api/v2/indices` |
| | `GET /api/v1/market/airings` | 无 | `/api/v2/tonight` |
| | `GET /api/v1/market/macro` | 无 | `/api/macro` + `/api/v2/macro` |
| 股票 | `GET /api/v1/stocks/{id}` | 可选 | `/api/stocks/{id}` + `/api/v2/stocks/{id}/profile` |
| | `GET /api/v1/stocks/search?q=` | 无 | 新（机器人和网页搜索框） |
| | `GET /api/v1/stocks/{id}/book` | 无 | `/api/stocks/{id}` 的 `book`、`auction` |
| | `GET /api/v1/stocks/{id}/trades` | 无 | `/api/stocks/{id}` 的 `trades` |
| | `GET /api/v1/stocks/{id}/candles` | 无 | `/api/stocks/{id}/candles` |
| | `GET /api/v1/stocks/{id}/intraday` | 无 | `/api/stocks/{id}/intraday` |
| | `GET /api/v1/stocks/{id}/cover` | 无 | `/api/covers/{id}` |
| | `GET /api/v1/stocks/{id}/works` | 无 | 新（作品、播出排期、单集讨论、续作） |
| | `GET /api/v1/stocks/{id}/corporate-actions` | 无 | 新（拆股、配股、回购） |
| | `GET /api/v1/stocks/{id}/rights` | 可选 | 新（认购期的配股） |
| | `PUT /api/v1/stocks/{id}/rights/subscription` | 玩家 | 新 |
| | `DELETE /api/v1/stocks/{id}/rights/subscription` | 玩家 | 新 |
| 资讯 | `GET /api/v1/news` | 无 | `/api/news`、`/api/stocks/{id}` 的 `news` |
| | `GET /api/v1/news/{id}` | 无 | 新 |
| 新股 | `GET /api/v1/ipos` | 可选 | `/api/ipos` + `/api/v2/listings` |
| | `GET /api/v1/ipos/{id}` | 可选 | `/api/stocks/{id}` 的 `ipo` |
| | `PUT /api/v1/ipos/{id}/subscription` | 玩家 | `POST /api/ipos/{id}/subscribe` |
| | `DELETE /api/v1/ipos/{id}/subscription` | 玩家 | `DELETE /api/ipos/{id}/subscribe` |
| 账户 | `GET /api/v1/me` | 玩家 / 机器人代表 | `/api/me` |
| | `GET /api/v1/me/portfolio` | 玩家 / 机器人代表 | `/api/portfolio` |
| | `GET /api/v1/me/asset-history` | 玩家 / 机器人代表 | 新（资产曲线） |
| | `GET /api/v1/me/orders` | 玩家 / 机器人代表 | `GET /api/orders` |
| | `GET /api/v1/me/orders/{id}` | 玩家 / 机器人代表 | 新 |
| | `POST /api/v1/me/orders` | 玩家 | `POST /api/orders` |
| | `DELETE /api/v1/me/orders/{id}` | 玩家 | `DELETE /api/orders/{id}` |
| | `GET /api/v1/me/trades` | 玩家 / 机器人代表 | `/api/trades` |
| | `GET /api/v1/me/ledger` | 玩家 / 机器人代表 | `/api/ledger` |
| | `POST /api/v1/me/restructure` | 玩家 | `POST /api/me/restructure`（开发中） |
| | `POST /api/v1/me/agreements` | 玩家 | 新 |
| | `POST /api/v1/me/announcements/{id}/read` | 玩家 | 新 |
| | `DELETE /api/v1/me` | 玩家 | 新（注销账号） |
| | `GET /api/v1/assessments/{risk\|margin-quiz}` | 无 | 新 |
| | `POST /api/v1/me/assessments/{risk\|margin-quiz}` | 玩家 | 新 |
| 排行 | `GET /api/v1/leaderboard?period=all\|week\|season` | 无 | `/api/leaderboard` |
| 认证 | `GET /api/v1/auth/challenge?purpose=register\|login` | 无 | 新 |
| | `POST /api/v1/auth/register` | 无 | `/api/register` |
| | `POST /api/v1/auth/login` | 无 | `/api/login` |
| | `POST /api/v1/auth/logout` | 无 | `/api/logout` |
| 绑定 | `POST /api/v1/me/bind-codes` | 玩家（仅网页） | 新 |
| | `GET /api/v1/me/bindings` | 玩家（仅网页） | 新 |
| | `DELETE /api/v1/me/bindings/{id}` | 玩家（仅网页） | 新 |
| API 密钥 | `GET`、`POST /api/v1/me/api-keys` | 玩家（仅网页） | 新（§3.6） |
| | `DELETE /api/v1/me/api-keys/{id}` | 玩家（仅网页） | 新 |
| 机器人 | `POST /api/v1/bot/bindings` | 机器人 | 新 |
| | `DELETE /api/v1/bot/bindings/{platform}/{external_id}` | 机器人 | 新 |
| 实时 | `GET /api/v1/ws`（WebSocket） | 可选 | `/api/ws`（开发中，同一版本内替换） |
| 元数据 | `GET /api/v1/openapi.json` | 无 | 新 |

---

## 1. 现状问题（这份设计要解决的）

| 问题 | 现在的样子 | v1 怎么处理 |
|---|---|---|
| 重复接口 | `/api/macro`（利率、M2…）和 `/api/v2/macro`（7 日营收、需求指数）都叫宏观，按天各一份 | 合并成 `/market/macro`，一天一行 |
| 金额单位混用 | 旧接口用 `yuan()` 输出 2 位小数的浮点“币”；数据库、引擎、成交价比较都是整数分；前端再乘回去算 | 全部整数分 |
| 比例命名误导 | `return_pct` 里装的是 0.052 这样的比例，不是百分数；`change`、`pnl_pct` 同理 | 比例统一不带 `_pct`，含义写在类型里（§2.5） |
| 时间与日期 | `ts` 是带时区的时间戳，`day` 是交易日但没写；分时的 `minute` 是截断的字符串 `2026-11-06T21:05`；`/api/v2/tonight` 按 06:00 切“播出日”，交易日按 04:00 切 | 时间戳 `*_at` 带时区，交易日叫 `trading_day`，“今晚放送”改按交易日（§4.1.5） |
| 缩写 | `qty`、`ps`、`dps`、`pe_dyn`、`n`、`us/cn/jp` | 全拼，少量金融惯用缩写列入白名单（§2.2） |
| 错误格式 | `{"detail": "中文"}`；参数错误是 pydantic 的 422 `{"detail":[{"type","loc","msg","input"}]}`，前端要自己翻译 | 一种格式 `{code,message,details}` |
| 分页 | `/api/news?limit`（最多 200），`/api/orders`、`/api/trades`、`/api/ledger` 固定前 200 条，没有翻页 | 游标分页 |
| 比率分散 | 列表页的市盈率在 `/api/v2/ratios`，详情页的在 `/api/v2/stocks/{id}/profile`，每股净资产在 `/api/stocks/{id}` | 列表和详情各带一份完整的 `ratios` |
| 一支股票拆成两份 | `/api/stocks/{id}`（行情、盘口、周营收、受众）和 `/api/v2/stocks/{id}/profile`（集数、分红、比率、持有人）各请求一次，持有人两边都有，口径还不同（`company` 账户叫法不一） | 详情一个对象；盘口、成交、K 线这类变化快或数据量大的另开子资源 |
| 静态规则和动态状态混在一起 | `/api/status` 每秒都可能变（`now`、`phase`），却带着手续费、分红税档这些几乎不变的规则 | 拆成 `/market/status` 和 `/rules` |
| 兼容包袱 | `mod.rs` 的 Starlette 路由模拟、`json.rs` 的 Python `json.loads`/`repr` 复刻、`validate.rs` 的 pydantic 报错复刻，合计约 1700 行，只为了和 Python 服务器逐字节一致 | 停用回退路径后，迁移完成时整块删除 |

---

## 2. 约定

### 2.1 基础路径与版本

- 所有接口在 `/api/v1/` 下。WebSocket 在 `/api/v1/ws`。
- **稳定接口** = `GET /api/v1/openapi.json` 里列出、且没有标 `x-internal: true` 的接口。机器人只应该用稳定接口。
- 内部接口（运维、调试、差分测试用的）放在 `/api/internal/` 下，不进 OpenAPI，随时可改。
- **兼容规则**（v1 内）：
  - 可以新增接口、新增响应字段、给**开放枚举**新增取值（§2.7）、放宽请求参数的限制。客户端必须忽略不认识的字段。
  - 删除或改名字段、改类型、改含义、收紧参数、给**封闭枚举**加值，都算不兼容，只能在下一个大版本做。
  - 下一个大版本用 `/api/v2/`。现在的 `/api/v2/*` 只读接口在 v1 上线的同一个版本里删除，路径因此空出来。
- 每个响应都带 `AniEx-Version`（服务器版本，如 `0.4.0`）：前端用它发现服务器已更新并刷新（§7.4），机器人报告问题时附上。

### 2.2 JSON 命名

- 字段名 `snake_case`，与 Rust serde 默认一致，前端现有代码也是这个风格。
- **全拼**：`quantity` 而不是 `qty`，`per_share` 而不是 `ps`，`market` 而不是 `us/cn/jp` 的隐式含义（后者改为 `by_market: {us, cn, jp}`，键是市场代码，见下）。
- **允许的缩写**（白名单，只有这些）：`id`、`ipo`、`vcm`、`pe`、`pb`、`ttm`、`m2`、`url`；市场代码 `us`、`cn`、`jp`（作为对象的键）。
- 时间戳字段以 `_at` 结尾（`created_at`、`airs_at`），日期字段以 `_on` 或 `_day` 结尾（`listed_on`、`trading_day`、`base_day`），布尔字段以 `is_`/`has_`/`can_` 开头（`is_new`、`can_restructure`）。
- 关联对象统一用嵌套的简要对象，而不是平铺 `code`、`title`：
  ```json
  "stock": {"id": 21, "code": "100123", "title": "航海王"}
  ```
  这个 `StockRef` 出现在持仓、委托、成交、流水、资讯里，客户端不用再按 id 去行情表里找名字。

### 2.3 金额：整数分

**规则（已决定）：所有金额字段都是整数，单位是分（1 币 = 100 分），字段名不加 `_cents` 后缀，单位由 `Money` 类型说明。** 价格、成交额、市值、现金、分红、手续费、每股净资产、营收都是金额。在 OpenAPI 里它们都引用同一个类型 `Money`（`type: integer, format: int64, description: 分`），生成的 TS 类型里是 `type Money = number` 的别名，读代码时能看出单位。

为什么选整数分而不是十进制字符串：

| | 整数分 `22045` | 十进制字符串 `"220.45"` | 现在的浮点币 `220.45` |
|---|---|---|---|
| 和服务器内部一致 | 是（引擎、账本、数据库全是分） | 否，每次输出要格式化 | 否 |
| JS 里算总额、盈亏 | 直接加减，精确（2⁵³ ≈ 9×10¹³ 币，远超游戏规模） | 要先解析，解析后还是浮点 | 有舍入误差（前端现在就在做 `×100` 再比较） |
| 显示 | `formatMoney(v / 100)`，一处函数 | 可直接显示但千分位、`万/亿` 仍要处理 | 同左 |
| 机器人（TS） | 同前端 | 同前端 | 同前端 |
| 风险 | 忘了除 100 会显示错 100 倍：靠 `Money` 类型别名和统一的格式化函数防住 | 字符串参与运算会拼接而不是相加 | 精度问题隐蔽 |

**比例不是金额**：`change`、`pe`、`dividend_yield` 等是浮点数（§2.5）。指数点位是浮点数（不是金额）。利率是浮点数（年化比例）。

**请求里的金额也是整数分**：下单 `{"price": 22100}` 表示 221.00 币。旧接口接受 `"221.00"` 字符串或浮点并在服务器端检查“最多两位小数”；v1 只收整数，不再有“价格格式不对”这类错误，前端在输入框里把元转成分（`Math.round(Number(text) * 100)`，并检查输入本身最多两位小数）。

### 2.4 数量

- 股数是整数，字段名 `quantity`（及 `filled_quantity`、`available_quantity` 等）。
- 当前没有零股限制，最小 1 股，单笔上限 10,000,000（沿用 `validate.rs`）。

### 2.5 比例与百分数

- **所有比例都是小数**：`0.0523` 表示 5.23%。不使用 `_pct` 后缀，也不出现已经乘过 100 的数字。
- 服务器把比例四舍五入到 6 位小数（现在 v2 接口是 4 位，`/api/market` 的 `change` 不舍入）；显示精度由客户端决定。
- 倍数（超额认购倍数 `oversubscribed`、市盈率 `pe`、市净率 `pb`）也是浮点数，不是比例，但同样不乘 100。
- 无意义时为 `null`（如亏损股的市盈率、从未分红的股息率），不用 0 冒充。

### 2.6 时间与交易日

- **时间戳**：RFC 3339 / ISO 8601，带偏移，秒精度（唯一例外：行情快照的 `as_of` 带毫秒，用来比较新旧），游戏时区 `Asia/Shanghai`：`"2026-11-07T01:23:45+08:00"`。这就是引擎现在的 `format_ts` 格式，不需要改。WebSocket 的 `stock` 消息里的成交时间也是这个格式。
- **日期**：`YYYY-MM-DD`，没有时区。
- **交易日 `trading_day`**：一个交易日从 08:45 集合竞价开始，连续交易 09:00 到**次日** 04:00，**04:00 换日**（`game.day_rollover`）。所以 2026-11-07 01:30 的成交属于交易日 `2026-11-06`。凡是按天归属的对象都带 `trading_day`：成交、委托、流水、资讯、K 线、分时、宏观数据行。客户端**永远不要**用时间戳自己推算交易日。
- **日结**在 06:30（`game.daily_step_at`），在换日之后。04:00–06:30 之间，`trading_day` 已经是新的一天，但前一天还没日结完。`/market/status` 用 `last_closed_day` 说明最近一次完成日结的交易日。
- **其他日期字段**：`listed_on`、`sub_start`、`sub_end`、`delisted_on` 由日结写入，就是交易日，只是保留原名、不加 `trading_` 前缀；`first_air` 是首播的播出日期（游戏时区，沿用采集器口径），不是交易日。
- **区间参数**：`from`、`to` 都是**包含**端点的交易日。

### 2.7 枚举

- 枚举值一律是小写 `snake_case` 字符串。
- 每个枚举在 OpenAPI 里标明是**封闭**（客户端可以穷举处理）还是**开放**（会加新值，客户端要有兜底）。

| 枚举 | 取值 | 类型 |
|---|---|---|
| `Phase` 市场阶段 | `auction`（集合竞价 08:45–09:00）、`continuous`（连续交易 09:00–次日 04:00）、`closed` | 封闭 |
| `OrderSide` | `buy`、`sell` | 封闭（卖空上线时进下一个大版本或新增独立字段） |
| `OrderType` | `limit`、`market` | 开放 |
| `OrderStatus` | `open`、`filled`、`cancelled` | 开放（以后可能有 `expired`） |
| `StockStatus` | `subscribing`、`listed`、`delisted` | 开放 |
| `IpoStatus` | `upcoming`（认购未开始）、`open`（认购中）、`allotted`（已配售、未上市）、`listed`（已上市）、`cancelled`（取消发行） | 开放 |
| `NewsKind` | `system`、`ipo`、`allotment`、`premiere`、`finale`、`dividend`、`rate`、`delisting`、`warning`、`vcm`、`flash`、`sequel`、`delay`、`hiatus`、`split`、`rights`、`buyback` | 开放 |
| `NewsCategory` | `flash`（快讯）、`notice`（公告）、`airing`（放送）、`close`（日结）、`media`（媒体，外部新闻，docs/design/news.md） | 封闭 |
| `LedgerReason` | 见 §4.5.8 | 开放 |
| `HolderKind` | `player`、`npc`、`market_maker`、`committee`、`unissued` | 开放 |
| `NpcStyle` | `value`（价值型）、`trend`（趋势型）、`news`（消息型）、`retail`（散户型） | 开放 |
| `IndexKey` | `all`（番剧综合指数）、`season`（本季新番）、`long`（长篇连载） | 开放 |
| `LeaderboardPeriod` | `all`（总榜）、`week`（周榜）、`season`（季榜） | 开放 |
| `CandleSource` | `live`、`replay`（开市前用真实热度回放生成的走势） | 封闭 |

`NewsCategory` 是从 `kind` 推出来的分组，今天写在前端 `format.ts` 的 `NEWS_GROUP` 里；移到服务器端，前端和机器人用同一套分组：

| category | 标签 | kinds |
|---|---|---|
| `flash` | 快讯 | `flash`、`vcm` |
| `notice` | 公告 | `system`、`ipo`、`allotment`、`delisting`、`warning`、`sequel`、`split`、`rights`、`buyback`，以及任何未知的新 kind |
| `airing` | 放送 | `premiere`、`finale`、`delay`、`hiatus` |
| `close` | 日结 | `dividend`、`rate` |
| `media` | 媒体 | `media`（外部新闻，docs/design/news.md） |

### 2.8 空值

- **字段永远存在**。“不知道”或“不适用”用 `null`，不省略键。（旧接口里 `position`、`my_orders`、`available_cash` 只在登录时出现，v1 不再这样。）
- **数组永远不是 `null`**，没有内容就是 `[]`。
- 每个 schema 在 §4 里标出可空字段；没标的都不可空。
- 需要登录才有的内容放在 `viewer` 字段下（如新股的“我的认购”），未登录时 `viewer: null`。

### 2.9 错误

所有非 2xx 响应（WebSocket 握手失败也一样）的 body：

```json
{
  "code": "insufficient_cash",
  "message": "可用资金不足：认购 1,000 股需要冻结 5,011.25 星辉",
  "details": {"required": 501125, "available": 230000}
}
```

- `code`：稳定的机器可读代码（下表），客户端按它分支。
- `message`：可以直接显示给玩家的中文，遵守 `docs/copywriting.md`（不用“请”“您”，用“无法”不用“失败”，全角标点，中英文之间半角空格）。服务器统一文案后，前端的 `MESSAGES` 改写表和 `translateValidation` 删除。
- `details`：可选对象，结构由 `code` 决定；没有时为 `{}`。
- 响应头带 `AniEx-Request-Id`，日志里同一个 id，方便玩家报问题。

**参数校验错误**统一 400 `invalid_request`，不再有 422：

```json
{
  "code": "invalid_request",
  "message": "数量需大于 0",
  "details": {"fields": [{"field": "quantity", "reason": "too_small", "message": "数量需大于 0"}]}
}
```

`reason` 取值：`missing`、`wrong_type`、`too_small`、`too_large`、`too_short`、`too_long`、`not_allowed`（不在枚举里）、`bad_format`。顶层 `message` 是第一个字段的 `message`。

**错误代码目录**（中文文案来自现在前端改写后的版本，或沿用服务器的原句）：

| HTTP | code | message（示例） | details |
|---|---|---|---|
| 400 | `invalid_request` | 见上 | `fields` |
| 400 | `invalid_json` | 请求内容不是有效的 JSON | — |
| 401 | `unauthenticated` | 登录后可用 | — |
| 401 | `bad_credentials` | 用户名或密码不正确 | — |
| 403 | `forbidden` | 没有权限 / 这个账号现在不能用 API 交易 | `required_scope`；API 交易条件不满足时另有 `required`（§3.6） |
| 403 | `not_bound` | 这个 QQ 还没有绑定账号 | — |
| 404 | `not_found` | 没有这个接口 | — |
| 404 | `stock_not_found` | 股票不存在 | `id` |
| 404 | `order_not_found` | 委托不存在 | `id` |
| 404 | `ipo_not_found` | 新股不存在 | `id` |
| 404 | `news_not_found` | 资讯不存在 | `id` |
| 404 | `cover_not_found` | 没有封面 | — |
| 404 | `binding_not_found` | 绑定不存在 | — |
| 404 | `media_news_not_found` | 没有这条新闻 | — |
| 405 | `method_not_allowed` | 不支持这个操作 | `allow` |
| 409 | `username_taken` | 用户名已有人使用 | — |
| 409 | `order_finished` | 委托已结束，无法撤单 | `status` |
| 409 | `already_bound` | 这个 QQ 已经绑定了另一个账号 / 这个账号已经绑定了其他 QQ，先解除绑定再绑定 | `side`: `qq` \| `account` |
| 422 | `market_closed` | 休市中（集合竞价 08:45–09:00，连续交易 09:00–次日 04:00） | `phase`、`next_open_at` |
| 422 | `auction_limit_only` | 集合竞价时段只接受限价委托 | — |
| 422 | `price_required` | 限价委托需要填写价格 | — |
| 422 | `price_out_of_range` | 价格超出范围：不能超过昨收的 20 倍（4,409.00 星辉） | `max`、`min` |
| 422 | `insufficient_cash` | 可用资金不足 | `required`、`available` |
| 422 | `insufficient_shares` | 可卖数量不足 | `requested`、`available` |
| 422 | `stock_not_tradable` | 这支股票已退市 | `status` |
| 422 | `ipo_not_open` | 不在认购期（认购时间是 2026-11-06 至 2026-11-09） | `sub_start`、`sub_end` |
| 422 | `ipo_closed` | 认购已截止，无法撤回 | — |
| 422 | `bad_invite` | 邀请码不正确，可以向社团管理员索取 | — |
| 403 | `registration_closed` | 暂未开放注册 | — |
| 403 | `agreement_required` | 签署风险揭示书后才能交易 | — |
| 403 | `verification_required` | 完成身份验证后才能认购新股 | — |
| 422 | `assessment_limit` | 每天最多测评 3 次，明天再试 | — |
| 422 | `reset_code_invalid` | 验证码不正确或已过期，重新获取后再试 | — |
| 422 | `close_not_allowed` | 还清融资融券负债后才能注销 | — |
| 422 | `ipo_quota_exceeded` | 认购金额超过认购额度 1,234.00 星辉（前 5 个交易日的日均持仓市值） | `quota`、`max_quantity` |
| 409 | `document_outdated` | 文件已更新，刷新页面后重新阅读 | — |
| 422 | `pow_required` | 需要先完成验证，重新提交即可 | — |
| 422 | `pow_invalid` | 验证已失效，重新提交即可 | — |
| 503 | `busy` | 当前登录的人较多，稍后重试 | `retry_after`（秒） |
| 422 | `restructure_not_eligible` | 总资产低于 10,000 星辉才能申请重整 | `reason`: `not_poor` \| `this_season` \| `market_closed` \| `margin_debt`，`threshold` |
| 409 | `credit_open` | 信用账户已经开通 | — |
| 409 | `too_many_open_orders` | 最多同时挂 100 笔委托，先撤掉一些 | `scope`（`account` / `stock`）、`max` |
| 422 | `credit_not_eligible` | 注册满 7 天后才能开通 / 净资产不低于 50,000 星辉才能开通 / 穿仓后需要先申请重整 | — |
| 422 | `credit_not_open` | 尚未开通信用账户 | — |
| 422 | `credit_defaulted` | 信用账户已穿仓 | — |
| 422 | `credit_limit_only` | 信用委托只接受限价 | — |
| 422 | `credit_continuous_only` | 信用委托只在连续交易时段接受 | — |
| 422 | `margin_call_active` | 追保或强制平仓期间不能新增融资融券 | — |
| 422 | `not_marginable` | 这支股票不是融资融券标的 | — |
| 422 | `insufficient_margin` | 可用保证金不足 | — |
| 422 | `financing_cap` | 这支股票的融资额度已满 | — |
| 422 | `lending_pool_empty` | 可融券数量不足：最多 120 股 | — |
| 422 | `short_price_rule` | 融券卖出价格不能低于最新价 5.21 | — |
| 422 | `concentration` | 单支股票的融资融券不能超过净资产 | — |
| 422 | `holds_long` | 持有这支股票时不能融券卖出 | — |
| 422 | `has_short` | 欠这支股票的券时，买入请用买券还券 | — |
| 422 | `no_debt` | 没有融资欠款 | — |
| 422 | `no_short` | 没有这支股票的融券负债 | — |
| 422 | `return_too_many` | 还券数量超过欠券：最多 80 股 | — |
| 422 | `liquidating` | 强制平仓期间不能买入 | — |
| 422 | `bind_code_invalid` | 绑定码不正确或已过期 | — |
| 429 | `rate_limited` | 操作太频繁，稍后重试 | `retry_after`（秒） |
| 429 | `daily_order_limit` | 今天的委托已达上限（5000 笔），下一个交易日再来 | `max` |
| 500 | `internal` | 服务器暂时无法响应，稍后重试 | — |
| 503 | `unavailable` | 服务器正在维护，稍后重试 | — |

状态码约定：语法或类型错 → 400；身份 → 401/403；资源不存在 → 404；和当前状态冲突（重名、已结束）→ 409；请求本身合法但游戏规则不允许（休市、资金不足）→ 422。客户端只要显示 `message` 就够了，状态码给日志和重试逻辑用（只有 429、503 和网络错误值得重试）。

### 2.10 分页

**会增长的列表**（资讯、个股成交、我的委托历史、我的成交、我的流水）用游标分页，按 id 从新到旧：

```
GET /api/v1/news?limit=50                 第一页（最新的 50 条）
GET /api/v1/news?limit=50&before=c_8812   更旧的一页
GET /api/v1/news?after=c_8890             比 8890 更新的全部（最多 limit 条，从旧到新）
```

响应：
```json
{"items": [ ... ], "next_cursor": "c_8762", "has_more": true}
```

- `limit` 默认 50，最大 200。
- 游标是不透明字符串（实现上就是行 id，加前缀防止客户端当数字用）；`next_cursor` 在没有更多时为 `null`。
- `after` 专门用于**断线补齐**：WebSocket 重连后，客户端拿自己见过的最新 id 去补漏掉的资讯、成交、流水（§5.4）。`after` 的结果从旧到新排列，方便依次追加。`before` 和 `after` 不能同时用。
- **固定规模的列表**（行情约 150 支、指数 3 个、新股、排行榜约 40 人、持仓）不分页，返回全部，外层同样是 `{"items": [...]}` 加必要的元数据。
- **时间序列**（K 线、分时、宏观、指数走势）用 `from`/`to` 区间，不用游标。

### 2.11 缓存与 ETag

| 类别 | 例子 | 响应头 |
|---|---|---|
| 账户数据 | `/me/*` | `Cache-Control: no-store` |
| 实时行情 | `/market/status`、`/market/quotes`、`/stocks/{id}/book` | `Cache-Control: no-cache` + `ETag`（内容哈希）；客户端带 `If-None-Match` 时没变就回 304 |
| 日内不变的数据 | `/rules`、`/stocks/{id}/candles?to=<昨天>`、`/market/macro`、指数历史 | `Cache-Control: public, max-age=60` + `ETag`；在 06:30 日结后内容才变 |
| 封面 | `/stocks/{id}/cover`，静态路径 `/covers/{id}.jpg` | `Cache-Control: public, max-age=604800, immutable`（沿用）；静态路径不经过 API 层、不计入限流，Cloudflare 按扩展名默认缓存 |
| OpenAPI | `/openapi.json` | `ETag`，按服务器版本 |

ETag 主要是给机器人和轮询的客户端省流量；网页连上 WebSocket 后基本不轮询。

### 2.12 限流

令牌桶，按主体计数。数字写在 `game.toml` 的 `[api.rate_limit]` 里，可以不重启以外的方式调整（改配置后重启生效）：

| 主体 | 限额 | 配置键 |
|---|---|---|
| 已登录玩家（按账号，读写合计） | **10 次/秒**（桶容量 20，允许短暂突发） | `player_rps`、`player_burst` |
| 机器人令牌（按令牌，代表的所有玩家合计） | **30 次/秒**（桶容量 60） | `bot_rps`、`bot_burst` |
| 未登录（按 IP） | 10 次/秒（桶容量 20） | `anon_rps`、`anon_burst` |
| 登录 / 注册（按 IP） | 10 次/分 | `auth_per_min` |
| 注册（按 IP，另外计数；只有注册成功才计入） | 20 次/天 | `register_per_ip_day` |
| 绑定码兑换（按 QQ） | 5 次/小时 | `bind_per_hour` |
| 下单、撤单（按账号，网页和全部 API 密钥合计，同时也算在上面“已登录玩家”里） | **5 次/秒**（桶容量 10） | `order_rps`、`order_burst` |
| 每日委托（按账号，只计接受的委托） | 5,000 笔/交易日 | `orders_per_day` |
| 未成交委托（按账号） | 同时 100 笔，单支股票 20 笔 | `max_open_orders`、`max_open_orders_per_stock` |

- 每日委托的计数在内存里，服务重启后从 0 开始。挂单超限返回 409 `too_many_open_orders`，每日超限返回 429 `daily_order_limit`。
- 响应头采用 IETF 草案 `RateLimit-Limit`、`RateLimit-Remaining`、`RateLimit-Reset`；超限返回 429 `rate_limited` 和 `Retry-After`。
- 按 IP 计数时，IPv6 地址按所在的 /64 合并（一个用户通常有整个 /64，可以随意换地址）；IPv4 映射地址按 IPv4 计。
- WebSocket 不按消息计数，只限制每个 IP 的并发连接（默认 64，`ws_per_ip`；校园网等共用出口的用户多）、全站并发连接（默认 5000，`ws_total`）、每个账号用 API 密钥打开的连接（默认 3，`ws_per_account`，浏览器不计）和每条连接的订阅数（§5.2）。
- API 密钥和网页共用账号的桶（“已登录玩家”那一行），不按密钥分别计数。
- 密码哈希（scrypt，每次约 50 ms）同时最多算 `hash_concurrency` 个（默认 2）。登录、注册最多等 5 秒，等不到返回 503 `busy`，撮合和推送不受影响。

### 2.13 其他

- 请求体只接受 `Content-Type: application/json`。写操作要求这个头，这同时让跨站表单无法直接提交（浏览器会先发预检），和 `SameSite=Lax` 一起防 CSRF。
- 成功的创建返回 201 并带上新对象；成功的删除返回 200 并带上删除后的对象（如撤单后的委托），没有内容可返回时用 204。
- 所有响应都是对象，不直接返回数组，这样以后可以加元数据。

---

## 3. 认证与绑定

### 3.1 浏览器：会话 cookie

保持现在的实现不变：

- 密码 scrypt（N=2¹⁴, r=8, p=1，`盐hex:哈希hex`），兼容老账号。
- cookie `session`：HMAC 签名的账号 id 和过期时间，`HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000`（30 天）。
- 只有接口路径和响应格式变（§3.4）。服务器端会话（可以“退出所有设备”）是架构文档第 3 步的事，接口不受影响：cookie 的内容对客户端不透明。

### 3.2 机器人：服务器级令牌

- 机器人用 `Authorization: Bearer <token>` 认证。令牌在 `.env`（`ANISTOCK_BOT_TOKENS`，可多个，格式 `name:token`），或者以后放进数据库表 `bot_clients(name, token_hash, scopes, created_at)`。只存哈希，比较用常数时间。
- 令牌本身的权限：所有公开读接口 + 绑定兑换 / 解除接口 + `/api/v1/ws` 的公开主题和 `events`。
- **同一个请求不能同时用 cookie 和机器人令牌**；带了 `Authorization` 就忽略 cookie。
- 令牌泄露的影响：能读所有已绑定玩家的账户数据（只读），不能交易。所以 v1 只开放只读权限是安全上的底线，不只是功能取舍。

### 3.3 代表已绑定的玩家

机器人在请求里加：

```
Authorization: Bearer <bot token>
X-Act-As: qq:123456789
```

- 服务器查绑定表，找到 `(platform=qq, external_id=123456789)` 对应的账号，以这个账号的身份执行请求，权限是**绑定时授予的 scope 和令牌 scope 的交集**。
- 没有绑定 → 403 `not_bound`（机器人据此提示“先在网页上生成绑定码”）。权限不够 → 403 `forbidden`，`details.required_scope`。
- `platform` 现在只有 `qq`，留着是为了以后接别的平台不改格式。

**权限（scope）**：

| scope | 内容 | v1 |
|---|---|---|
| `read:public` | 行情、资讯、新股、排行 | 令牌自带 |
| `read:account` | `/me`、`/me/portfolio`、`/me/orders`（只读）、`/me/trades`、`/me/ledger` | 绑定时授予 |
| `trade` | 下单、撤单、新股认购 | **预留**，v1 不能授予；服务器拒绝在绑定码里写入 |
| `notify` | 机器人私聊推送这个玩家的成交、中签 | **预留** |

`POST /me/restructure`、绑定管理、注册登录永远不能由机器人代表执行（只在网页上做）。

### 3.4 认证接口

#### `GET /api/v1/auth/challenge?purpose=register|login`

注册模式和工作量证明的题目（设计见 `docs/design/account.md` §3.2）。不缓存。

```json
{"registration": "open",
 "challenge": {"algorithm": "PBKDF2-SHA-256", "purpose": "register", "salt": "9f2c…", "cost": 5000, "max": 1000,
               "target": "3a7e…", "expires": 1793000000, "signature": "c41b…"}}
```
- `registration`：`open` 开放注册；`invite` 需要邀请码；`closed` 暂停注册。由 `game.toml` 的 `[account] registration` 或环境变量 `ANISTOCK_REGISTRATION` 决定。
- `challenge`：不需要验证时为 `null`（注册已关闭；登录且不在高压状态）。
- 解法：找 `[0, max)` 里的整数 `n`，使 PBKDF2-SHA-256(password = `n` 的十进制字符串, salt = `salt` 的 UTF-8 字节, iterations = `cost`, 输出 32 字节) 的十六进制等于 `target`。提交 `{"challenge": <原样>, "number": n}`。
- 每道题只能提交一次（答错也算），有效期 `expires_secs`（默认 5 分钟）。
- 注册一直需要验证；登录只在全站每分钟的登录和注册请求超过 `surge_per_min` 时需要（之后持续 `cooldown_secs`），这段时间注册的 `max` 也乘以 `surge_factor`。参数在 `[api.pow]`。

#### `POST /api/v1/auth/register`

```json
{"username": "alice", "password": "secret123", "invite": "CLUB2026", "pow": {"challenge": {…}, "number": 417}}
```
- `username`：去掉首尾空白后 2–20 个字符；`password`：至少 6 位；`invite`：邀请注册模式下必填；`pow`：题目不为 `null` 时必填；`agree`：必须为 `true`（同意《用户协议》和《隐私政策》，记为当前版本的签署记录）。
- 201，设置 cookie，返回 `Me`（§4.5.1）。
- 错误：`invalid_request`、`registration_closed`、`bad_invite`、`pow_required`、`pow_invalid`、`username_taken`、`busy`、`rate_limited`。

#### `POST /api/v1/me/agreements`

同意或签署一份文件（`docs/design/account.md` §7）：

```json
{"doc": "disclosure", "version": 1, "signature": "alice"}
```
- `doc`：`terms` 用户协议、`privacy` 隐私政策、`disclosure` 风险揭示书；`version` 必须是当前版本（`game.toml` 的 `[account.documents]`），否则 409 `document_outdated`。
- 风险揭示书要用自己的用户名签署（去掉首尾空白后一致）；其他文件不需要 `signature`。
- 200，返回 `Me`。`Me.verification` 是身份验证状态（`verified`、`method`：目前只有 `qq`，即绑定 QQ）；`Me.eligibility` 列出财政补贴、新股认购（含认购额度 `quota`）、排行榜三项资格，不符合时 `reason` 是可以直接展示的原因。门槛只看身份验证状态，不直接看绑定（`backend/src/kyc.rs`）。排行榜只列出符合排行榜资格的玩家。`Me.pending_documents` 列出还没有同意或签署的当前版本文件；不为空时，下单、新股认购、配股认购、开通信用账户返回 403 `agreement_required`。撤单、还款、重整不受影响。

#### `POST /api/v1/me/announcements/{id}/read`

运营公告的已读标记。`Me.announcement` 是登录后要弹出的公告：开户之后发布、还没有标记已读的最新一条（`id`、`title`、`body` 纯文本空行分段、`published_at`），没有时为 `null`。标记已读后返回 `Me`，下一条未读的公告（如果有）出现在 `Me.announcement`。重复标记不报错；公告不存在时 404 `not_found`。

公告由运营者在服务器上发布：`anistock announce <标题> <正文文件> [--no-popup]`（服务运行时也可以执行；`--no-popup` 只记录、不弹出）。公告和已读记录是数据库的旁表（`announcements`、`announcement_reads`，迁移 V14），不在引擎状态里。

#### 风险测评与知识测试

`GET /api/v1/assessments/risk` 返回 8 道题；`GET /api/v1/assessments/margin-quiz` 返回 10 道题和及格题数 `pass_score`（8）。题目在服务器端（`backend/src/assessment.rs`），由服务器评分。

`POST /api/v1/me/assessments/{kind}`，`{"answers": [3, 1, …]}`（每题所选选项的序号）：风险测评返回 `risk`（C1–C5、名称、说明、有效期，`[account.eligibility] assessment_valid_days`）；知识测试返回 `quiz`（得分、是否通过、每题的正确答案和解释）。每种每天最多 `assessment_per_day` 次。

开通信用账户（`POST /api/v1/me/credit`）在原有条件（注册天数、净资产）之外，还要求：风险测评有效且不低于 C4、通过知识测试、签署当前版本的《融资融券风险揭示书》（`doc: "margin_disclosure"`，用用户名签署）。`Me.credit_steps` 列出这三项的状态，未满足时返回 422 `credit_not_eligible`。

#### `DELETE /api/v1/me`

`{"password": "…", "confirm": true}`。撤销全部委托和认购，持仓按最新价转给 world 账户，现金转给 world 账户，账号变为 `closed`、不再有用户名和密码；绑定、签署记录、测评、登录记录删除，账本和成交记录以匿名形式保留。有融资融券负债时返回 422 `close_not_allowed`。204，清除 cookie。

#### `POST /api/v1/auth/login`

```json
{"username": "alice", "password": "secret123", "pow": null}
```
200，设置 cookie，返回 `Me`。错误：`invalid_request`、`pow_required`、`pow_invalid`、`bad_credentials`、`busy`、`rate_limited`。收到 `pow_required` 或 `pow_invalid` 时，重新取题再提交。

#### `POST /api/v1/auth/logout`

204，清除 cookie。未登录时也是 204。

> 和旧接口相比：登录、注册直接返回 `Me`，前端省一次 `/me`；重名从 400 改为 409。

### 3.5 QQ 绑定

流程：

```
网页（已登录）                 服务器                          群里 / Koishi
POST /me/bind-codes ───────►  生成 8 位码，10 分钟有效
          ◄── {code:"K7Q3-9MZD", expires_at}
玩家私聊机器人 “/安易 绑定 K7Q39MZD” ───────────────────────────►  机器人收到
                                ◄── POST /bot/bindings {code, platform:"qq", external_id:"<openid>"}
                               校验码 → 写绑定，作废码
                                ──► 201 {binding, account:{id, username}}
                                                               回复“已绑定 alice”
```

- 码由网页生成，这样“谁的账号”由登录会话证明，“谁的 QQ”由可信的机器人证明，两边都不需要在群里暴露密码。
- 码：8 个字符，Crockford Base32（去掉易混的 I L O U），显示时中间加连字符，兑换时忽略连字符和大小写；一次性；10 分钟过期；同一账号同时只有一个有效码（再生成会作废旧码）。
- **只在私聊中兑换（已决定）**：绑定码在群里发出会被别人抢先兑换；绑定的 QQ 又是找回密码的凭据，所以绑定、解绑、找回密码都只在私聊中处理（`docs/qq-bot.md`）。`external_id` 是官方机器人私聊事件里发送者的 openid（字母、数字、`_`、`-`，最长 64 位）。
- **一对一（已决定）**：一个账号只能绑一个 QQ，一个 QQ 只能绑一个账号。换绑须先解绑（网页上解绑，或者私聊机器人 `/安易 解绑`），再生成新码。

#### `POST /api/v1/me/bind-codes`（仅网页 cookie）

请求：`{"scopes": ["read:account"]}`（可省略，默认就是这个；v1 只接受这一个值）。

账号已经绑定了 QQ 号时返回 409 `already_bound`（`details.side: "account"`，文案“这个账号已经绑定了 QQ 号，先解绑再绑定”），不发码。

201：
```json
{"code": "K7Q3-9MZD", "expires_at": "2026-11-06T21:40:00+08:00", "scopes": ["read:account"]}
```

#### `GET /api/v1/me/bindings`（仅网页 cookie）

```json
{"items": [{"id": 3, "platform": "qq", "external_id": "123456789", "display_name": "123****789",
            "scopes": ["read:account"], "created_at": "2026-11-06T21:32:10+08:00", "last_used_at": "2026-11-06T22:01:44+08:00"}]}
```
`display_name` 是打码后的 QQ 号，避免截图泄露；完整号码只在绑定者本人的网页上显示（就是 `external_id`）。一对一之下 `items` 最多一项（每个平台一项）；保留数组是为了以后接别的平台。

#### `DELETE /api/v1/me/bindings/{id}`（仅网页 cookie）

204。错误：`binding_not_found`。

#### `POST /api/v1/bot/bindings`（机器人令牌）

```json
{"code": "K7Q39MZD", "platform": "qq", "external_id": "123456789"}
```
201：
```json
{"binding": {"id": 3, "platform": "qq", "external_id": "123456789", "scopes": ["read:account"],
             "created_at": "2026-11-06T21:32:10+08:00", "last_used_at": null},
 "account": {"id": 4, "username": "alice"}}
```
错误：`bind_code_invalid`、`already_bound`（`details.side: "qq"`：这个 QQ 号已经绑定了别的账号；`"account"`：码生成之后账号在别处绑定了）、`rate_limited`（同一 QQ 号每小时 5 次，防止猜码）。

#### `DELETE /api/v1/bot/bindings/{platform}/{external_id}`（机器人令牌）

私聊 `/安易 解绑`。204。错误：`binding_not_found`。

#### 修改和找回密码

- `POST /api/v1/me/password`（玩家）：`{"current_password", "new_password"}`。200，返回 `Me`，换发 cookie。错误：`invalid_request`（`current_password` 不正确、新密码不足 6 位）。
- `POST /api/v1/bot/password-resets`（机器人令牌）：`{"platform": "qq", "external_id": "<openid>"}`。201，`{"code": "K7Q3-9MZD", "expires_at", "account": {"id", "username"}}`。错误：`not_bound`、`rate_limited`（同一 QQ 每小时 5 次）。
- `POST /api/v1/bot/news`（机器人令牌）：news-feed 股市扩展推送一条判断过的外部新闻。格式、合并和三档模式见 `docs/design/news.md` §2–3。
- `POST /api/v1/auth/password-reset`（无需登录）：`{"username", "code", "new_password"}`。200，返回 `Me`，设置 cookie。错误：422 `reset_code_invalid`（验证码不正确、过期、用过或输错 5 次）。按 IP 计入登录限额。
- 改密码或重设后，签发时间早于这次修改的 cookie 不再有效（`docs/design/account.md` §13）。

**需要的新表**（迁移 V5，示意）：

```sql
CREATE TABLE bind_codes (code TEXT PRIMARY KEY, account_id INTEGER NOT NULL, scopes TEXT NOT NULL,
                         expires_at TEXT NOT NULL, used_at TEXT);
CREATE TABLE bindings (id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, platform TEXT NOT NULL,
                       external_id TEXT NOT NULL, scopes TEXT NOT NULL, created_at TEXT NOT NULL,
                       last_used_at TEXT, UNIQUE(platform, external_id), UNIQUE(account_id, platform));
```

### 3.6 API 密钥（脚本）

玩家自己的密钥，给脚本用。设计和分阶段计划见 `docs/design/api-access.md`。

- 在网页设置里生成（`POST /api/v1/me/api-keys`，只接受 cookie）：`{"name": "看盘脚本", "expires_in_days": 90}`，有效期 30、90（默认）或 365 天。201 返回 `{"key": ApiKey, "token": "anx_…"}`，明文只返回这一次，服务器只存 SHA-256。每个账号同时有效的密钥最多 5 个，超出返回 409 `too_many_api_keys`。
- 请求时带 `Authorization: Bearer anx_…`。前缀 `anx_` 区分 API 密钥和机器人令牌。密钥无效、已吊销或已过期返回 401 `unauthenticated`。
- 权限（生成时在 `scopes` 里选，之后不能改）：
  - `read:account`（默认）：和机器人代表玩家时相同，公开读接口加 `/me/*` 的读接口，WebSocket 可以订阅 `me`。
  - `trade`：另外可以下单、撤单（`POST`/`DELETE /me/orders`，含信用委托）、认购和撤销新股与配股、还融资款。返回的 `scopes` 为 `["read:account", "trade"]`。只在 `game.toml` 的 `[api] trading = true` 时能生成（否则 400 `invalid_request`），而且账号要完成身份验证、签署当前版本的《程序化交易风险揭示书》（`POST /me/agreements`，`doc: "api_disclosure"`，签名为用户名）。这两个条件每次请求都检查：不满足时返回 403 `forbidden`，`details.required` 列出缺少的条件（`api_trading`、`verification`、`api_disclosure`），满足后自动恢复，不用重新生成。`GET /me` 的 `api_trading` 给出这些状态。
  - 只读密钥调用交易接口返回 403 `forbidden`，`details.required_scope` 为 `"trade"`；其余写接口和只在网页上做的操作（登录、改密码、签署、开通信用、重整、注销、管理密钥）返回 403，`required_scope` 为 `"web"`。
  - 用 `trade` 密钥下的单记在 `api_orders` 表里（哪个密钥、哪个交易日），排行榜据此显示“程序化”标记（§4.6）。脚本下单建议带 `Idempotency-Key`，网络重试不会重复下单。
- `GET /api/v1/me/api-keys` 列出没有吊销的密钥（含已过期的），带 `prefix`（明文前 8 位）、`last_used_at`、`last_used_ip`；`DELETE /api/v1/me/api-keys/{id}` 吊销，立即生效，用它打开的 WebSocket 以 4001 关闭。
- 找回密码时全部密钥吊销，注销账号时全部删除。主动修改密码不影响密钥。

---

## 4. 资源与接口

先定义共用的对象，接口再引用它们。示例数据取自本地 `data/game.db`（航海王 id 21，代码 100123，昨收 220.45 币）。

### 4.0 共用对象

#### `StockRef`
```json
{"id": 21, "code": "100123", "title": "航海王"}
```
`id` 是 AniList 作品 id（整数，也是公司 id）；`code` 是 6 位显示代码（字符串，按上市顺序编号）。两者都是 6 位左右的数字，**不能混用**：路径参数一律是 `id`。

#### `Quote`（`/market/quotes` 的一行，也嵌在 `StockDetail.quote` 里）

| 字段 | 类型 | 可空 | 说明（旧字段） |
|---|---|---|---|
| `stock` | StockRef | | （`id`、`code`、`title`） |
| `title_ja` | string | 是 | 原名 |
| `title_en` | string | 是 | |
| `price` | Money | | 最新价（`price`，旧为浮点币） |
| `prev_close` | Money | | 昨收 |
| `change` | number | | 比例，`price / prev_close - 1`；昨收为 0 时为 0 |
| `open` | Money | 是 | 今开（本交易日第一笔成交）；无成交为 null |
| `high` / `low` | Money | 是 | 同上 |
| `volume` | integer | | 本交易日成交股数 |
| `turnover` | Money | | 本交易日成交额 |
| `market_cap` | Money | | `price × shares` |
| `week_revenue` | Money | | 截至最近一次日结的 7 日营收 |
| `week_revenue_change` | number | 是 | 比例，比前 7 日（`revenue_change`） |
| `episodes_aired` | integer | | 已播集数（`aired`） |
| `episodes` | integer | 是 | 总集数，未知或长篇为 null |
| `is_long_runner` | boolean | | （`long_runner`） |
| `first_air` | date | | |
| `listed_on` | date | | |
| `is_new` | boolean | | 上市不足 5 个交易日（`new`） |
| `ipo_remaining` | integer | | 首日发行未卖完的股数，通常为 0（`ipo_left`） |
| `ipo_price` | Money | | |
| `vcm_active` | boolean | | 正在冷静期（`cooling`） |
| `sparkline` | Money[] | | 最近 20 个交易日收盘价，旧到新（`spark`） |
| `ratios` | Ratios | | 见下（原 `/api/v2/ratios`） |
| `cover_url` | string | | `/covers/21.jpg` |

#### `Ratios`

| 字段 | 可空 | 定义（沿用 `v2.rs` 的口径） |
|---|---|---|
| `pe_ttm` | 是 | 市盈率（TTM）= 价格 / EPS，EPS = 近 365 天营收 ×（1 − 运营成本率）×（1 − 企业所得税）/ 股本（`pe`） |
| `pe_dynamic` | 是 | 市盈率（动）：近 28 天营收年化（`pe_dyn`）。番剧的收入集中在播出的三个月，年化会把播出期的高峰当成全年，严重低估市盈率；界面不再展示，列表和行情条改用 `pe_ttm` |
| `dividend_yield_ttm` | 是 | 股息率（TTM）= 近 365 天每股分红 / 价格（`dividend_yield`）；拆股、配股前的分红按复权因子换算成现在的每股（v0.6.0） |
| `dividend_yield_dynamic` | 是 | 股息率（动）= 近 28 天已派分红的均值 × 52 / 价格（`dividend_yield_dyn`）。同上，季播作品会严重夸大，界面不再展示 |
| `pb` | 是 | 市净率 = 价格 / 每股净资产 |
| `book_value_per_share` | 是 | Money，公司现金 / 股本（原在 `/api/stocks/{id}` 的 `book_value_ps`） |

列表和详情返回完全相同的 `ratios`。

#### `Vcm`
```json
{"active": true, "until": "2026-11-06T21:35:00+08:00", "low": 19841, "high": 24250, "count_today": 1}
```
`until`、`low`、`high` 在 `active: false` 时为 null。`count_today` 是本交易日触发的次数（第一次按 5 分钟前价格 ±10%，之后按今开 ±20%、±30%…）。

#### `BookLevel`
```json
{"price": 22050, "quantity": 120, "orders": 3}
```

#### `PublicTrade`
```json
{"id": 88213, "price": 22045, "quantity": 20, "at": "2026-11-06T21:04:13+08:00", "trading_day": "2026-11-06"}
```
不含买卖双方。`id` 单调递增，可用于去重和 `after` 补齐。

#### `NewsItem`
```json
{"id": 8890, "at": "2026-11-07T01:12:40+08:00", "trading_day": "2026-11-06",
 "kind": "flash", "category": "flash",
 "title": "热度快讯：《航海王》第 1180 集讨论量高于预期 35%",
 "body": "播出 3 小时后的 Bangumi 单集吐槽数，与前几集同一时间相比。",
 "stock": {"id": 21, "code": "100123", "title": "航海王"}}
```
`body`、`stock` 可空（`system` 类资讯没有关联股票）。`body` 是展示用的中文文本，里面的金额已经格式化，客户端不解析。

#### `Order`

| 字段 | 类型 | 可空 | 说明（旧字段） |
|---|---|---|---|
| `id` | integer | | |
| `stock` | StockRef | | （`company_id`、`code`、`title`） |
| `side` | OrderSide | | |
| `type` | OrderType | | （`kind`） |
| `price` | Money | 是 | 限价；市价单为 null |
| `quantity` | integer | | （`qty`） |
| `filled_quantity` | integer | | （`filled`） |
| `average_fill_price` | Money | 是 | 成交均价，未成交为 null（新：由该委托的成交算出） |
| `frozen_cash` | Money | | 买单仍冻结的资金，卖单为 0 |
| `status` | OrderStatus | | |
| `created_at` / `updated_at` | timestamp | | |
| `trading_day` | date | | 下单时的交易日；委托当日有效，日结时撤销 |

#### `Ipo`

| 字段 | 类型 | 可空 | 说明（旧字段） |
|---|---|---|---|
| `stock` | StockRef | | |
| `title_ja` | string | 是 | |
| `status` | IpoStatus | | 新增 `upcoming`、`listed`、`cancelled`（旧接口只有 `open`/`allotted`，`/api/v2/listings` 另列已上市和即将认购的） |
| `price` | Money | | 发行价（`ipo_price`） |
| `sub_start` / `sub_end` | date | | 认购期（含两端） |
| `listed_on` | date | | |
| `first_air` | date | | |
| `float_quantity` | integer | | 流通盘（`float_qty`） |
| `base_public_quantity` | integer | | 公开发售基础数量（`base_public_qty`） |
| `applied_quantity` | integer | | 认购总股数（`applied_qty`） |
| `applicants` | integer | | 认购人数 |
| `result` | object | 是 | 配售后才有：`{"oversubscribed": 18.4, "public_quantity": 875, "allotment_ratio": 0.0475}` |
| `first_day` | object | 是 | 上市后才有：`{"close": 2361, "change": 0.159}`（原 `/api/v2/listings` 的 `first_close`、`first_change`） |
| `viewer` | object | 是 | 登录后：`{"quantity": 200, "frozen": 40740, "allotted": null}`；没有认购也是对象，`quantity: 0`。未登录为 null（`my`） |

#### `Position`

| 字段 | 类型 | 可空 | 说明（旧字段） |
|---|---|---|---|
| `stock` | StockRef | | |
| `quantity` | integer | | |
| `available_quantity` | integer | | 去掉卖单冻结（`available`） |
| `average_cost` | Money | | 持仓批次的加权成本（`avg_cost`） |
| `price` | Money | | |
| `value` | Money | | |
| `pnl` | Money | | 浮动盈亏 = 市值 − 成本 |
| `pnl_ratio` | number | 是 | （`pnl_pct`） |
| `day_change` | number | | 比例，价格相对昨收 |
| `dividends` | Money | | 这支股票累计到手的分红（税前，与旧接口一致） |
| `oldest_lot_on` | date | 是 | 最早一批的买入交易日，用来提示分红税档（`oldest_lot`） |
| `newest_lot_on` | date | 是 | 最近一批的买入交易日。分红税按批先进先出，这一批持有满期后全部按最低税率，用来提示“再持有几天分红免税” |

---

### 4.1 市场

#### 4.1.1 `GET /api/v1/market/status`

小而频繁变化，网页连上 WebSocket 后由 `time` 消息代替轮询（§5.3）。

```json
{
  "now": "2026-11-07T01:30:02+08:00",
  "trading_day": "2026-11-06",
  "phase": "continuous",
  "phase_ends_at": "2026-11-07T04:00:00+08:00",
  "next_phase": "closed",
  "next_open_at": "2026-11-07T08:45:00+08:00",
  "last_closed_day": "2026-11-05",
  "launched_on": "2026-11-04",
  "policy_rate": 0.03,
  "deposit_rate": 0.02,
  "sentiment": "neutral"
}
```
- `sentiment`：当天的市场情绪档位（封闭枚举）`cold` 冷淡 / `neutral` 平稳 / `hot` 火热，交易日内不变；NPC 市场或情绪层未开启时为 null。只给档位，不给数值（docs/design/sentiment.md）。
- `phase_ends_at`、`next_phase`：当前阶段何时结束、之后是什么。休市时 `phase_ends_at` = `next_open_at`。开发开关 `ANISTOCK_ALWAYS_OPEN` 下两者为 null。
- `next_open_at`：下一次集合竞价开始的时间（交易中时也给出，方便显示“下次开盘”）。
- 旧字段 `trading`（布尔）删除，用 `phase != "closed"`；`day` 改名 `trading_day`；`auction_start`/`open`/`close`/`rollover`/`rules` 移到 `/rules`。

#### 4.1.2 `GET /api/v1/rules`

几乎不变的游戏规则。前端规则页、下单框的手续费预估、机器人的 `/规则` 都读这里。

```json
{
  "timezone": "Asia/Shanghai",
  "session": {"auction_start": "08:45", "open": "09:00", "close": "04:00", "close_is_next_day": true,
              "day_rollover": "04:00", "daily_close": "06:30"},
  "fees": {"commission": 0.00025, "stamp_duty": 0.0005, "stamp_duty_on": "sell"},
  "dividend_tax": [{"max_holding_days": 0, "rate": 0.2}, {"max_holding_days": 27, "rate": 0.1},
                   {"max_holding_days": null, "rate": 0.0}],
  "vcm": {"threshold": 0.1, "window_minutes": 5, "cool_minutes": 5, "later_bands": [0.2, 0.3]},
  "price_limit_multiple": 20,
  "initial_cash": 10000000,
  "restructure": {"threshold_ratio": 0.1, "per_season": 1},
  "ipo": {"subscription_days": 3, "public_base": 0.5,
          "clawback": [{"oversubscribed": 15, "public_share": 0.7}, {"oversubscribed": 50, "public_share": 0.8},
                       {"oversubscribed": 100, "public_share": 0.9}]},
  "order_quantity_max": 10000000,
  "heat_index": {"version": "v1", "weights": {"us": 0.3, "cn": 0.5, "jp": 0.2},
                 "platform": {"us": 116352.0, "cn": 1772.0, "jp": 1649.0}, "scale": 14437.0}
}
```
- `heat_index`：安易热度指数的编制方法（docs/design/futures.md §3.7）。一天的指数 = `scale` × Σ `weights[m]` × 当天市场 m 的热度 ÷ `platform[m]`；当天热度见 `/stocks/{id}/heat` 的 `us`、`cn`、`jp`（没有数据来源的市场不计）。
- 分红税档：`max_holding_days` 含端点，按游戏时间的持有天数；最后一档为 null（无上限），代替配置里的 `100000`。
- `initial_cash` 是金额（分）：100,000 币 = `10000000`。
- `price_limit_multiple`、`stamp_duty_on` 等字段以实现时的真实配置为准；这里列的是字段形状。

#### 4.1.3 `GET /api/v1/market/quotes`

全部上市股票的行情，**比率合并在每一行里**。

参数：`ids`（可选，逗号分隔的 id，机器人查少数几支时用）。

```json
{
  "trading_day": "2026-11-06",
  "as_of": "2026-11-06T21:05:00+08:00",
  "items": [
    {
      "stock": {"id": 21, "code": "100123", "title": "航海王"},
      "title_ja": "ONE PIECE", "title_en": "ONE PIECE",
      "price": 22045, "prev_close": 22045, "change": 0.0,
      "open": null, "high": null, "low": null, "volume": 0, "turnover": 0,
      "market_cap": 220450000,
      "week_revenue": 1825000, "week_revenue_change": -0.042,
      "episodes_aired": 1150, "episodes": null, "is_long_runner": true,
      "first_air": "1999-10-20", "listed_on": "2026-07-01",
      "is_new": false, "ipo_remaining": 0, "ipo_price": 30379,
      "vcm_active": false,
      "sparkline": [21803, 21950, 22045],
      "ratios": {"pe_ttm": 31.2, "pe_dynamic": 24.8, "dividend_yield_ttm": 0.021,
                 "dividend_yield_dynamic": 0.034, "pb": 2.7, "book_value_per_share": 8165},
      "cover_url": "/covers/21.jpg"
    }
  ]
}
```
- `as_of` 和 ETag 只随数据变化，不随请求时间变化：`as_of` 是最后一次成交或报价变化的时间，ETag 只对 `items` 取哈希，所以没变化时能返回 304。
- 顺序：按 `code` 升序（与旧接口相同）。排序、筛选在客户端做（全量约 150 行）。
- 退市和认购中的股票不在这里：退市的查 `/stocks/{id}`，认购中的查 `/ipos`。
- 错误：`invalid_request`（`ids` 格式不对）。不存在的 id 直接不出现在结果里，不报错。

#### 4.1.4 `GET /api/v1/market/indices`

```json
{
  "trading_day": "2026-11-06",
  "season_start": "2026-10-01",
  "season_from": "2026-09-17",
  "basis": "total_return",
  "breadth": {"up": 61, "flat": 22, "down": 58},
  "items": [
    {"key": "all", "name": "番剧综合指数", "members": 141, "base_day": "2026-07-01",
     "value": 1043.27, "prev_close": 1038.9, "change": 0.004206,
     "series": [{"trading_day": "2026-07-01", "value": 1000.0}, {"trading_day": "2026-11-06", "value": 1043.27}]}
  ]
}
```
- 市值加权（前一天的市值）、逐日链接；今天还没有 K 线时，最后一点用最新价对昨收算出（`is_live` 由 `series` 最后一项的 `trading_day == 本交易日` 判断）。
- **成分按当天**（v0.6.0 起）：每天计入当天和前一天都有收盘价的股票。退市的股票保留退市前的涨跌，退市后不再计入；新股从上市第二天起计入。`members` 是今天在市的成分数。
- **口径** `basis`：默认 `total_return`（全收益，主指数）：分红在当天加回（当天收盘价 + 每股分红），退市当天按返还股东的每股现金计；`basis=price` 返回价格指数（只看价格）。两个口径的基日和基点相同（1000）。
- **复权**：拆股、配股按复权因子换算（和 K 线前复权一致），除权本身不改变指数。权重用前复权收盘价 × 现在的流通股数（不含制作委员会和公司自己持有的股份，即按流通市值加权）。
- **本季新番** `season`：首播在 `season_from` 及以后的非长篇。`season_from` 是季度第一天（`season_start`）前 14 天：10 月季的新番常在 9 月最后一周开播，算本季（和估值的季度划分一致）。
- 参数 `series=false` 去掉走势（机器人只要点位时用）。
- 参数 `set`：`market`（默认）只返回行情页的 3 个；`all` 再加上指数基金跟踪的 6 个（docs/v3-funds.md §3）：`top20` 安易 20（每月第一个交易日按前一天的流通市值取前 20，月中不变，退市即移出）、`manga` 漫改、`light_novel` 轻改（AniList 原作类型）、`isekai` 异世界、`romance` 恋爱、`action` 热血（AniList 类型和标签）。
- **权重上限**：`long` 和基金跟踪的 6 个指数，单只股票的权重不超过 25%，超出的部分按市值比例分给其他成分股（`all`、`season` 不设上限）。规则在 backend/src/indices.rs，基金调仓用的是同一份。
- 旧字段 `prev` 改 `prev_close`；`series` 从 `[day, value]` 二元数组改成对象（体积大约翻倍，3 条 × 约 130 点，可以接受；换来自描述）。

#### 4.1.5 `GET /api/v1/market/airings`

本交易日要播的集数（“今晚放送”）。

参数：`trading_day`（可选，默认本交易日；只支持本交易日和下一个交易日，因为数据来自 AniList 的“下一集”快照）。

```json
{
  "trading_day": "2026-11-06",
  "from": "2026-11-06T04:00:00+08:00",
  "to": "2026-11-07T04:00:00+08:00",
  "items": [
    {"stock": {"id": 191656, "code": "100051", "title": "学生会也有洞！"}, "episode": 6,
     "airs_at": "2026-11-06T23:30:00+08:00", "has_aired": false}
  ]
}
```
- **行为变化**：现在的 `/api/v2/tonight` 按 06:00–次日 06:00 的“播出日”切分。v1 改为按交易日（04:00–次日 04:00），全站只有一种“天”。（已决定，Q6）
- 包括认购中的股票（首播前就在认购）。

#### 4.1.6 `GET /api/v1/market/macro`

合并 `/api/macro`（日结写入的 `macro_days`）和 `/api/v2/macro`（按天聚合的营收），一天一行。

参数：`from`、`to`（交易日，可选；默认最近 90 天）。

```json
{
  "items": [
    {"trading_day": "2026-11-04", "policy_rate": 0.03, "m2": 412500000, "m2_growth": 0.0031,
     "injected": 2500000, "exported": 1830000, "taxes": 412000, "interest": 31000,
     "revenue_7d": 249500000, "demand_index": 0.998}
  ]
}
```
- `m2`、`injected`、`exported`、`taxes`、`interest`、`revenue_7d` 是金额（分）；`m2_growth`、`policy_rate` 是比例；`demand_index` 是倍数（1.00 = 经济的基准周需求）。
- 某天只有一边的数据时，另一边的字段为 null（如开市前的回放日没有 `macro_days`）。
- `interest` 是 `macro_days` 里本来就有但旧接口没输出的列。

---

### 4.2 股票

#### 4.2.0 `GET /api/v1/stocks/search`

按名字或代码找股票。机器人的 `/行情 芙莉莲`、网页的搜索框都用它。

参数：
- `q`：必填，1–50 个字符。
- `limit`：默认 5，最大 20。
- `status`：默认 `listed,subscribing`；加上 `delisted` 可以搜到已退市的。

```json
{
  "query": "海贼",
  "items": [
    {"stock": {"id": 21, "code": "100123", "title": "航海王"}, "status": "listed", "score": 0.92,
     "matched": {"field": "alias", "text": "海贼王"},
     "price": 22045, "change": 0.0}
  ]
}
```
- `score`：0–1，越大越相关，按它降序。`matched` 说明命中的是哪个名字（`field`：`code`、`title`、`title_ja`、`title_romaji`、`title_en`、`alias`），机器人可以回复“按别名‘海贼王’找到《航海王》”。
- 带上 `price`、`change`，机器人查一支股票不用再请求一次行情。
- 没有结果时 `items` 为 `[]`，不报错。错误：`invalid_request`（`q` 为空或太长）。

**匹配规则**（按优先级，分数由高到低）：
1. `q` 是 6 位数字且等于某个 `code`：精确命中，分数 1.0。
2. 名字（下表全部来源）规范化后与 `q` 完全相等：0.95。
3. 名字以 `q` 开头：0.85；包含 `q`：0.75（再按 `q` 占名字长度的比例微调，越接近整名越高）。
4. 模糊：字符 bigram 的相似度（中日文按字，拉丁字母按词内字母）不低于 0.3 时，分数 = 0.7 × 相似度。

规范化：全角转半角、繁体转简体（OpenCC 的字表）、去掉空格和标点（`・`、`：`、`!`、`～` 等）、拉丁字母转小写、日文片假名转平假名。同一支股票多个名字都命中时取最高分。

**名字的来源**：

| 字段 | 来源 |
|---|---|
| `title` | 公司表（Bangumi `name_cn` 或手工覆盖） |
| `title_ja`、`title_en` | 公司表 |
| `title_romaji` | 采集器 AniList 快照的 `title.romaji` |
| `alias` | 采集器 Bangumi 条目快照 `infobox` 里的“别名”（如《哆啦A梦》的“哆啦A梦 水田版”“Doraemon (2005)”）；AniList 快照的 `synonyms` |

- 名字表在启动时和每次日结后（采集器 06:00 刚更新快照）从游戏库和采集器库（`collector_db`，只读）重建一次，放在内存里；约 150–300 个作品，每个几到十几个名字，直接线性扫描即可，不需要索引。
- 只读采集器已经存下的快照，不额外请求 Bangumi 或 AniList；按 Annict 的条款，Annict 的数据不用于这里。

#### 4.2.1 `GET /api/v1/stocks/{id}`

一支股票的完整资料：行情、比率、经营数据、集数、分红、持有人、受众数据。**合并了 `/api/stocks/{id}` 和 `/api/v2/stocks/{id}/profile`**。盘口、成交、资讯、K 线、个人持仓和委托不在这里，各有接口（它们变化快或数据量大，网页连上 WebSocket 后由 `stock:<id>` 推送）。

退市和认购中的股票也能查（`status` 区分）。

```json
{
  "stock": {"id": 21, "code": "100123", "title": "航海王"},
  "title_ja": "ONE PIECE", "title_en": "ONE PIECE", "studio": "Toei Animation",
  "status": "listed",
  "trading_day": "2026-11-06",
  "shares": 10000,
  "first_air": "1999-10-20", "listed_on": "2026-07-01", "delisted_on": null,
  "episodes": null, "episodes_aired": 1150, "is_long_runner": true,
  "ipo_price": 30379,
  "quote": {"price": 22045, "prev_close": 22045, "change": 0.0, "open": null, "high": null, "low": null,
            "open_reference": 22045, "volume": 0, "turnover": 0, "market_cap": 220450000, "ipo_remaining": 0},
  "vcm": {"active": false, "until": null, "low": null, "high": null, "count_today": 0},
  "ratios": {"pe_ttm": 31.2, "pe_dynamic": 24.8, "dividend_yield_ttm": 0.021, "dividend_yield_dynamic": 0.034,
             "pb": 2.7, "book_value_per_share": 8165},
  "revenue_weeks": [
    {"week_start": "2026-10-26", "revenue": 1905000, "by_market": {"us": 820000, "cn": 610000, "jp": 475000},
     "dividend_per_share": 160, "earnings_per_share": 179, "episodes_aired": 1149}
  ],
  "trailing_4w": {"earnings_per_share": 702, "dividends_per_share": 640},
  "episode_days": [{"episode": 1149, "trading_day": "2026-11-02"}],
  "next_episode": {"episode": 1151, "airs_at": "2026-11-08T09:30:00+08:00"},
  "dividends": [{"trading_day": "2026-11-02", "per_share": 160}],
  "holders": {
    "player": 812, "committee": 5300, "unissued": 0, "market_maker": 1400, "other": 0,
    "npc": {"total": 2488, "by_style": {"value": 1320, "trend": 410, "news": 198, "retail": 560}}
  },
  "audiences": {
    "anilist": {"popularity": 312000, "score": 88, "genres": ["Action", "Adventure", "Comedy", "Drama"]},
    "bangumi": {"id": 975, "collection": {"wish": 1520, "doing": 30110, "collect": 5220, "on_hold": 8410, "dropped": 2150},
                "score": 8.7},
    "annict": {"id": 2534, "watchers": 4210}
  },
  "ipo": null,
  "cover_url": "/covers/21.jpg"
}
```

字段说明和变化：

- `quote.open_reference`：今日开盘参考价（集合竞价价或昨收），VCM 后续档位的锚（旧 `open_ref`）。
- `revenue_weeks`：按周（周一起）汇总，`week_start` 是该周第一天（旧 `weeks[].week`，`us/cn/jp` 收进 `by_market`，`dividend_ps` 改 `dividend_per_share`，`aired` 改 `episodes_aired`）。
- `revenue_weeks[].earnings_per_share`：这一周的每股收益 = 营收 ×（1 − 运营成本率）×（1 − 企业所得税）/ 股本，四舍五入到分；运营成本率和税率取服务器配置（`company.operating_cost_ratio`、`company.corp_tax`），和 `Ratios.pe_ttm` 同一口径。股本用当前股数（拆股后的历史周按新股数计，和前复权 K 线一致）。
- `trailing_4w`：近 28 天（与 `Ratios` 同样按 `day > 今天 − 28` 切）的实际合计：每股收益、每股分红。不年化。每股分红和 `revenue_weeks` 的 `dividend_per_share` 都按复权因子换算成现在的每股（与 K 线前复权一致）。
- `episode_days`：每集首次出现在数据里的交易日（旧 profile 的 `episodes: [{n, day}]`，改名避免和总集数 `episodes` 冲突）。
- `holders`：**口径统一**。旧 `/api/stocks/{id}` 按账户类型原样输出（`company`、`mm`、`npc`…），profile 把 `company` 叫 `unissued`、其余全算 `npc`。v1 固定为：`player`（玩家）、`committee`（制作委员会）、`unissued`（公司账户里未发行的）、`market_maker`（做市商 `mm`）、`npc`（NPC 市场的四类主体，`by_style` 按名字前缀的主体类型分；v1 NPC 的 `noise` 账户计入 `npc.total` 但不进 `by_style`）、`other`（世界账户、央行、政府：破产重整和下架收购会把股票卖给世界账户）。各项之和等于 `shares`。NPC 分类型的持股由正在开发的 NPC 市场提供；在那之前 `by_style` 为 null。
- `audiences`：对应来源没有数据时该键为 null（不再省略）。旧的 `anilist.next_episode`（含 Unix 秒 `airingAt`）删除，用顶层 `next_episode`；`anilist.cover` 删除，用 `cover_url`。`bangumi.name_cn` 删除（就是 `title`）。
- `ipo`：有认购历史的股票给出 `Ipo` 对象（含 `viewer`），否则 null。
- 不再包含：`book`、`trades`、`news`、`auction`（见下面的子资源和 `/news?stock_id=`）、`position`、`my_orders`、`available_cash`（用 `/me/portfolio` 和 `/me/orders?stock_id=`）。

错误：`stock_not_found`、`invalid_request`（id 不是整数）。路由上 `/stocks/search` 是静态路径，优先于 `/stocks/{id}` 匹配。

> 为什么个人持仓不放进详情：机器人查行情不需要；网页股票页本来就有 `/me` 的现金和 `/me/orders` 的委托，WebSocket 的 `me` 主题会实时更新它们。把它们放进详情会让这个接口按人变化，没法用 ETag。

#### 4.2.2 `GET /api/v1/stocks/{id}/book`

```json
{
  "trading_day": "2026-11-06", "as_of": "2026-11-06T21:05:00.300+08:00",
  "bids": [{"price": 22000, "quantity": 120, "orders": 3}],
  "asks": [{"price": 22100, "quantity": 80, "orders": 2}],
  "auction": null
}
```
- 各 5 档。参数 `depth`（1–20，默认 5）。
- `auction`：集合竞价阶段的参考成交价和量 `{"price": 22080, "quantity": 340}`，`price` 可空（无法撮合时）；其他阶段为 null。
- `as_of` 带毫秒，用来和 WebSocket 推送比较新旧（§5.4）。

#### 4.2.3 `GET /api/v1/stocks/{id}/trades`

公开成交，游标分页（§2.10），`items` 为 `PublicTrade`。旧接口只有最近 30 条嵌在详情里。

#### 4.2.4 `GET /api/v1/stocks/{id}/candles`

参数：`from`、`to`（交易日，可选，默认全部）；`adjust`：`forward`（默认，前复权）或 `none`（不复权）。

```json
{
  "items": [
    {"trading_day": "2026-11-05", "open": 21803, "high": 22300, "low": 21700, "close": 22045, "volume": 460,
     "source": "live", "is_partial": false},
    {"trading_day": "2026-11-06", "open": 22045, "high": 22200, "low": 21990, "close": 22100, "volume": 85,
     "source": "live", "is_partial": true}
  ],
  "adjust": "forward",
  "ex_days": [{"trading_day": "2026-10-20", "kind": "split", "price_factor": 0.5}]
}
```
- 前复权（docs/design/corporate.md §5）：每个除权除息日之前的开高低收 × `price_factor`，成交量按拆股比例相乘；最新价格不变。除权日是拆股、配股后按新股数交易的第一天；除息日是分红日（当天日结时派发）之后的第一个交易日，因子 = 1 − 每股分红 / 分红日收盘价（`kind: "dividend"`）。分红日之后还没有日结过的交易日时，本交易日未完成的那一根就是除息日。`ex_days` 是区间内的除权除息日。（2026-11-08 起含分红；之前分红不复权。）
- 本交易日有成交而还没日结时，最后一根是 `is_partial: true`（旧 `live: true`）。旧字段 `time` 改 `trading_day`，`replay` 布尔改成 `source` 枚举。
- 只有日线。参数 `interval` 预留，v1 只接受 `1d`。

#### 4.2.5 `GET /api/v1/stocks/{id}/intraday`

参数：`trading_day`（可选，默认本交易日）。

```json
{
  "trading_day": "2026-11-06",
  "prev_close": 22045,
  "items": [{"minute": "2026-11-06T21:05:00+08:00", "price": 22100, "volume": 20}]
}
```
- `minute` 是分钟起点的完整时间戳（旧值 `2026-11-06T21:05` 没有秒和时区）。夜盘跨过午夜时，同一个交易日里会出现第二天日期的分钟，这正是要带时区和交易日的原因。
- `prev_close` 让分时图不用再请求一次行情就能画基准线。
- 只有有成交的分钟出现。

#### 4.2.7 `GET /api/v1/stocks/{id}/corporate-actions`

公司资本政策（docs/design/corporate.md §3）的每次行动，新的在前。

```json
{"items": [
  {"id": 3, "kind": "rights", "status": "pending", "announced": "2026-11-05", "effective": "2026-11-08",
   "split_ratio": null, "rights_ratio": 0.3, "rights_price": 800, "rights_budget": 500000,
   "rights_terp": null, "rights_issued": null, "rights_subscribed": null,
   "buyback_budget": null, "buyback_cap": null, "buyback_spent": null, "buyback_bought": null}
]}
```
- `kind`：`split`（拆股）、`rights`（配股）、`buyback`（回购）。`status`：`pending`（已公告、未生效）、`active`（回购进行中）、`done`、`cancelled`。
- `effective`：拆股是按新股数交易的第一天；配股是认购截止、登记、缴款、发股的交易日（它的日结时完成）；回购是最后一天。
- 只适用于某一种的字段，其他种类为 `null`；配股的结果（除权参考价、发行股数、玩家认购股数）在完成后才有值。

#### 4.2.8 `GET /api/v1/stocks/{id}/rights`

认购期的配股：`{"action": CorporateAction | null, "entitlement": 30, "subscribed": 30, "frozen": 24000}`。没有登录时 `entitlement`、`subscribed` 为 0。`entitlement` 按当前持仓计算，登记时按登记时的持仓重新计算，认购超出的部分退回。

#### 4.2.9 `PUT /api/v1/stocks/{id}/rights/subscription`、`DELETE …`

请求 `{"quantity": 30}`：设定认购数量（幂等，0 等于撤回），冻结 数量 × 配股价。返回同 4.2.8。错误：`rights_not_open`（没有认购期的配股，422）、`rights_rejected`（超过配股权或资金不足，422）。`DELETE` 撤回，没有配股时也返回 200。

#### 4.2.10 `GET /api/v1/stocks/{id}/works`

股票名下的作品（docs/design/series.md §3：一个系列一家公司）、各作品的播出排期和单集讨论，以及续作。个股页“作品”栏用；变化以天计，客户端几分钟刷新一次即可。

```json
{"works": [
  {"id": 195516, "title": "药屋少女的呢喃 第三季", "first_air": "2026-10-01", "episodes": 12, "episodes_aired": 5,
   "is_long_runner": false, "status": "airing",
   "schedule": [{"episode": 5, "date": "2026-10-29", "aired": true, "delayed": false},
                {"episode": 6, "date": "2026-11-06", "aired": false, "delayed": true},
                {"episode": 12, "date": null, "aired": false, "delayed": false}],
   "discussion": [{"episode": 1, "comments": 89}, {"episode": 5, "comments": 55}]}
 ],
 "sequel": {"status": "announced", "title": "無職転生 IV", "premiere": "2027-07", "announced": "2026-10-20"}}
```
- `works`：第一部（`id` 等于股票 id）在前，之后是并入的作品（`company_works`）。`title`：第一部是股票名，其余取 Bangumi 中文名，没有时取 AniList 原名。
- `status`：`upcoming`（未开播）、`airing`、`finished`、`halted`（AniList HIATUS，暂停播出）。长篇连载不会是 `finished`（两个篇章之间的空档不算完结）。
- `schedule`：一季的每一集；长篇只有最近 8 集和已公布的下一集。已播的 `date` 是实际播出的交易日；未播的 `date` 只来自公开排期（Bangumi 各集日期，和 AniList 下一集的时间不一致时按 AniList 顺延），**没有公开日期时为 null**——估值内部对未定日期的每周外推、对停播的恢复日假设都不在这里出现。`delayed`：晚于 Bangumi 原定日期超过 1 天（深夜档的日期写法差一天不算），或有这一集的延期公告。
- `discussion`：Bangumi 单集吐槽数（采集器最新快照，中国市场营收用的就是这项公开数据），已播各集，最多最近 26 集；长篇按 Bangumi 的总集数编号。
- `sequel.status`：`unknown`（采集器还没有这部作品的关系数据，无法判断）、`none`（未公布）、`announced`（已公布，未定档）、`dated`（定档）。`premiere` 是公布的开播时间，精度按公布的写（`2027-01-10`、`2027-01`、`2027`）；只有完整日期才是 `dated`。并入公司、尚未开播的新一季也算 `dated`（`premiere` 为它的首播日）。不给出估值用的续作概率或预计日期。

#### 4.2.6 `GET /api/v1/stocks/{id}/cover`

作品封面图片（Bangumi 200px），首次请求时下载并缓存在磁盘（沿用 `v2.rs` 的实现）。成功返回图片本身（`image/jpeg` 等）；没有封面返回 404 `cover_not_found`（JSON），`Cache-Control: public, max-age=3600`。

同一张图也在 `/covers/{id}.jpg`（`cover_url` 给的就是它）：不在 `/api` 下，不计入限流、不带 API 响应头，浏览器和 Cloudflare 边缘都按 `Cache-Control` 缓存。网页用这个路径；`/stocks/{id}/cover` 保留给已有的调用方。

---

### 4.3 资讯

#### 4.3.1 `GET /api/v1/news`

参数（都可选，可组合）：
- `stock_id`：某支股票的资讯（代替旧详情里的 `news`）
- `category`：`flash` / `notice` / `airing` / `close`，可逗号分隔多个
- `kind`：具体类型，可逗号分隔
- `trading_day`：某个交易日
- `limit`、`before`、`after`：游标分页

```json
{"items": [ <NewsItem>, ... ], "next_cursor": "c_8840", "has_more": true}
```
错误：`invalid_request`（未知的 `category`；未知的 `kind` 不报错，因为 kind 是开放枚举）。

#### 4.3.2 `GET /api/v1/news/{id}`

一条 `NewsItem`。错误：`news_not_found`。给机器人推送后“查看详情”用，网页暂时用不到。

---

### 4.4 新股

#### 4.4.1 `GET /api/v1/ipos`

合并 `/api/ipos`（认购中和近两周已配售的）和 `/api/v2/listings`（近 60 天上市的、即将开始认购的）。

参数：`status`（可逗号分隔，默认 `upcoming,open,allotted`）；`listed_since`（交易日，`status` 含 `listed` 时有效，默认 60 天前）。

```json
{
  "trading_day": "2026-11-06",
  "items": [
    {"stock": {"id": 206949, "code": "100105", "title": "乱世千金倪亚‧利斯顿转生为娇弱千金的弑神武人华丽无双录"},
     "title_ja": "凶乱令嬢ニア・リストン", "status": "listed", "price": 2037,
     "sub_start": "2026-09-10", "sub_end": "2026-09-12", "listed_on": "2026-09-14", "first_air": "2026-10-05",
     "float_quantity": 2500, "base_public_quantity": 1250, "applied_quantity": 23000, "applicants": 31,
     "result": {"oversubscribed": 18.4, "public_quantity": 1750, "allotment_ratio": 0.0761},
     "first_day": {"close": 2361, "change": 0.159},
     "viewer": {"quantity": 200, "frozen": 0, "allotted": 15}}
  ]
}
```
排序：`open` 在前（按截止日），然后 `upcoming`（按开始日），然后 `allotted`、`listed`（按上市日倒序）。

#### 4.4.2 `GET /api/v1/ipos/{id}`

一个 `Ipo`。错误：`ipo_not_found`。

#### 4.4.3 `PUT /api/v1/ipos/{id}/subscription`

设定我的认购数量（新建或改量，幂等）。

请求：`{"quantity": 200}`（大于 0；撤回用 DELETE）。

200：
```json
{"ipo": <Ipo，含更新后的 viewer>, "frozen": 407400}
```
错误：`unauthenticated`、`ipo_not_found`、`ipo_not_open`、`insufficient_cash`、`invalid_request`。

> 旧接口是 `POST .../subscribe`，`qty` 可以是 0。改成 PUT 是因为它本来就是“设为这个数”，重复提交结果一样。

#### 4.4.4 `DELETE /api/v1/ipos/{id}/subscription`

撤回认购，200 返回更新后的 `{"ipo": <Ipo>}`。没有认购时也是 200（幂等）。错误：`ipo_closed`、`ipo_not_found`。

---

### 4.5 账户

`/me/*` 的读接口接受网页 cookie，也接受机器人令牌 + `X-Act-As`（需要 `read:account`）。写接口 v1 只接受 cookie。

#### 4.5.1 `GET /api/v1/me` → `Me`

```json
{
  "id": 4, "username": "alice",
  "trading_day": "2026-11-06",
  "cash": 6230000, "frozen_cash": 407400, "available_cash": 5822600,
  "stock_value": 4410000, "total_assets": 10640000,
  "initial_cash": 10000000,
  "total_return": 0.064,
  "dividends": 12800,
  "deposit_rate": 0.02,
  "restructure": {"count": 0, "eligible": false, "reason": "not_poor",
                  "message": "总资产低于 10,000 星辉才能申请重整"},
  "liabilities": 0, "credit_status": null
}
```
- `total_assets` 是**净资产**：现金 + 持仓市值 − `liabilities`（融资融券负债，§4.5.11）。没有开通信用账户时负债为 0，和以前的总资产相同。排行榜、资产曲线、重整门槛都用它。
- `credit_status`：信用账户状态 `normal` / `call`（追保）/ `liquidating`（强制平仓中）/ `defaulted`（已穿仓）；未开通为 null。
- `total_return`：**链接收益率**（时间加权），玩家没重整过时就是 `总资产 / 初始资金 − 1`（旧 `return_pct`）。
- `restructure`：来自开发中的破产重整（`restructures`、`can_restructure`、`restructure_reason` 三个旧字段收成一个对象）。`reason` 是 `not_poor` / `this_season` / `market_closed` / `margin_debt`（有融资融券负债）/ null（可以申请时），`message` 是对应文案。穿仓后不受每季一次的限制。
- 字段改名：`frozen` → `frozen_cash`，`available` → `available_cash`，`total` → `total_assets`，`initial` → `initial_cash`（且从浮点币改为分）。

#### 4.5.2 `GET /api/v1/me/portfolio`

```json
{
  "trading_day": "2026-11-06",
  "summary": {"cash": 6230000, "frozen_cash": 407400, "available_cash": 5822600, "stock_value": 4410000,
              "total_assets": 10640000},
  "items": [
    {"stock": {"id": 21, "code": "100123", "title": "航海王"}, "quantity": 200, "available_quantity": 200,
     "average_cost": 21500, "price": 22045, "value": 4409000, "pnl": 109000, "pnl_ratio": 0.0253,
     "day_change": 0.0, "dividends": 6400, "oldest_lot_on": "2026-11-02"}
  ]
}
```
按 `code` 排序，只含数量大于 0 的持仓（融券借来、还没卖出的股票不算持仓）。`summary.liabilities` 是融资融券负债，`summary.total_assets` 是净资产。

#### 4.5.3 `GET /api/v1/me/orders`

参数：`status`（`open`（默认）/ `closed` / `all`）、`stock_id`、`limit`、`before`、`after`。`items` 为 `Order`，按 id 倒序。

`status=open` 一般不超过几十条，仍然用同一个分页格式。

#### 4.5.4 `GET /api/v1/me/orders/{id}`

一个 `Order`。错误：`order_not_found`（不是自己的也是这个）。

#### 4.5.5 `POST /api/v1/me/orders`

```json
{"stock_id": 21, "side": "buy", "type": "limit", "price": 22100, "quantity": 100}
```
- `price`：限价单必填（整数分，大于 0）；市价单必须省略或为 null。
- 可选请求头 `Idempotency-Key`（任意 1–64 字符）：同一账号 24 小时内重复的 key 返回第一次的结果，不重复下单。网页双击和机器人重试都用得上；不带就不去重。

201：
```json
{
  "order": {"id": 5521, "stock": {"id": 21, "code": "100123", "title": "航海王"}, "side": "buy", "type": "limit",
            "price": 22100, "quantity": 100, "filled_quantity": 60, "average_fill_price": 22050, "frozen_cash": 884000,
            "status": "open", "created_at": "2026-11-06T21:05:12+08:00", "updated_at": "2026-11-06T21:05:12+08:00",
            "trading_day": "2026-11-06"},
  "fills": [{"trade_id": 88214, "price": 22050, "quantity": 60}]
}
```
`fills` 是这次下单当场撮合的成交（集合竞价阶段为空）。旧接口返回 `{order_id, status, filled, avg_price}`，前端要再请求委托列表；现在直接给出完整委托。

**信用委托**（融资融券，docs/design/margin.md）：加 `"credit": "<类型>"`，只接受限价、只在连续交易时段：

| `credit` | `side` | 说明 |
|---|---|---|
| `margin_buy` | buy | 融资买入：下单时借出 `价格 × 数量 + 佣金 + 每股 1 分`，冻结在委托里；没用完的部分在委托结束时还回 |
| `short_sell` | sell | 融券卖出：价格不低于最新价；下单时从制作委员会借券，成交所得冻结；没卖出的股票在委托结束时还回 |
| `sell_repay` | sell | 卖券还款：卖出持仓，所得先还这支股票的融资欠款（先利息后本金），再还其他 |
| `buy_return` | buy | 买券还券：先用冻结的融券卖出所得，不够再用可用资金；买到的股票直接还给出借人 |

`Order.credit` 是委托的类型，普通委托为 null。信用委托另外可能返回的错误见 §2.9（`credit_*`、`insufficient_margin` 等）。

错误：`unauthenticated`、`invalid_request`、`stock_not_found`、`stock_not_tradable`、`market_closed`、`auction_limit_only`、`price_required`、`price_out_of_range`、`insufficient_cash`、`insufficient_shares`、`rate_limited`。

#### 4.5.6 `DELETE /api/v1/me/orders/{id}`

撤单，200 返回撤单后的 `{"order": <Order>}`。休市时也可以撤单（与现在的引擎一致）。错误：`order_not_found`、`order_finished`。

#### 4.5.7 `GET /api/v1/me/trades`

参数：`stock_id`、`limit`、`before`、`after`。

```json
{"items": [
  {"id": 88214, "order_id": 5521, "stock": {"id": 21, "code": "100123", "title": "航海王"}, "side": "buy",
   "price": 22050, "quantity": 60, "amount": 1323000, "commission": 331, "stamp_duty": 0,
   "at": "2026-11-06T21:05:12+08:00", "trading_day": "2026-11-06"}
], "next_cursor": null, "has_more": false}
```
- `amount` = 价格 × 数量；`commission`、`stamp_duty` 来自账本里 `ref` 指向这笔成交的分录（旧接口没有，玩家要去流水里对）。
- `order_id` 是我这一方的委托（旧接口没有）。

#### 4.5.8 `GET /api/v1/me/ledger`

资金流水，不含成交本身（成交在 `/me/trades`）。

参数：`reason`（可逗号分隔）、`stock_id`、`trading_day`、`limit`、`before`、`after`。

```json
{"items": [
  {"id": 120331, "at": "2026-11-05T06:30:04+08:00", "trading_day": "2026-11-04", "reason": "dividend",
   "reason_label": "分红", "amount": 6400, "stock": {"id": 21, "code": "100123", "title": "航海王"}, "ref": null}
], "next_cursor": "c_120290", "has_more": true}
```
- `amount` 有正负：收入为正，支出为负（与旧接口相同）。
- `reason`（开放枚举），玩家可能看到的：`grant`（初始资金）、`dividend`（分红）、`dividend_tax`（股息税）、`commission`（佣金）、`stamp_duty`（印花税）、`interest`（存款利息）、`transfer`（财政补贴）、`delisting`（退市返还）、`ipo`（新股认购缴款）、`ipo_refund`（认购退款）、`restructure`（破产重整，开发中，名称以实现为准）、`margin_loan`（融资借款）、`margin_repay`（融资还款）、`margin_interest`（融资利息）、`lending_fee`（融券费用）、`short_dividend`（分红补偿）、`short_settle`（融券了结）。括号里就是服务器给出的 `reason_label`（已决定，Q8）：显示用的中文标签，机器人和网页共用一份文案，前端的 `LEDGER_REASON` 删除。客户端按 `reason` 分支，按 `reason_label` 显示；遇到不认识的 `reason` 也能显示标签。
- `npc_*`、`revenue`、`operating_cost`、`corp_tax`、`production_fee` 只发生在非玩家账户之间，玩家的流水里不会出现。

#### 4.5.9 `POST /api/v1/me/restructure`

破产重整（开发中，`backend/src/restructure.rs`）：撤销全部委托和认购，持仓按最新价卖给世界账户，现金补到初始资金；总资产须低于初始资金的 `rules.restructure.threshold_ratio`，每季一次，只能在连续交易时段申请。

请求：`{"confirm": true}`（防止误触；网页的确认对话框之后才发）。这个字段和 §4.5.5 的 `Idempotency-Key` 都是现有接口没有的小补充，不需要可以直接删掉。

200：`{"me": <Me>}`。错误：`unauthenticated`、`restructure_not_eligible`（`details.reason`）。

#### 4.5.10 `GET /api/v1/me/asset-history`

资产曲线：每个交易日日结后的总资产。

参数：`from`、`to`（交易日，可选，默认全部）。

```json
{
  "items": [
    {"trading_day": "2026-11-04", "cash": 6100000, "stock_value": 4300000, "total_assets": 10400000,
     "return_index": 1.04, "total_return": 0.04, "restructured": false, "is_partial": false},
    {"trading_day": "2026-11-06", "cash": 6230000, "stock_value": 4410000, "total_assets": 10640000,
     "return_index": 1.064, "total_return": 0.064, "restructured": false, "is_partial": true}
  ]
}
```
- 每项是该交易日**日结后**的状态（股票按收盘价计）。本交易日还没日结时，最后追加一项 `is_partial: true`，按最新价计；其余各项为 `false`。
- `return_index`：链接收益指数，从 1 开始，每次破产重整处断开再接上（= 重整前各段的期末/期初之积 × 当前段的 总资产/段初资产）。`total_return = return_index − 1`，就是 `Me.total_return`。周榜、季榜都由它算。
- `restructured`：这一天申请过重整。曲线在这一天总资产会跳回初始资金，`return_index` 不跳。

**快照表**（迁移 V6，已决定本次就建，Q5）：

```sql
CREATE TABLE player_days (
    account_id    INTEGER NOT NULL,
    day           TEXT NOT NULL,          -- trading day
    cash          INTEGER NOT NULL,       -- cents, after the close
    stock_value   INTEGER NOT NULL,       -- holdings at the day's close prices
    return_index  REAL NOT NULL,          -- chain-linked, 1.0 at registration
    restructured  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, day)
);
```
- **写入**：日结的最后一步（价格、分红、利息、转移支付都已入账之后），每个玩家一行，和日结同一个事务。日结补跑时按 `last_closed` 幂等。
- **回填**：迁移时从开服日起逐日重放每个玩家的成交和流水，用当天的收盘价（`candles`）计算持仓市值，写出历史各行；最后一天的结果必须等于当前账户状态，否则回填中止并只保留从迁移当天开始的快照（日志说明原因）。重整开始之前（V4 之前）的历史没有断点，`return_index = 总资产 / 初始资金`。
- 新注册的玩家从注册当天的日结开始有记录。
- V7 增加 `liabilities` 列（日结时的融资融券负债）；`AssetDay.liabilities`，`total_assets = cash + stock_value − liabilities`。

#### 4.5.11 融资融券

规则见 `docs/design/margin.md`。

- `GET /api/v1/me/credit` → `{"credit": <CreditAccount>}`：是否开通（`opened`）、未开通时能否开通（`can_open`、`open_message`）、状态、追保日、净资产、毛资产、负债及其构成（`debt`、`interest`、`short_value`、`fees`、`bad_debt`）、冻结的融券卖出所得（`locked`）、可用保证金、维持担保比例（没有负债时为 null）、融资利率、融券费率、各条线（`margin_ratio`、`call_ratio`、`liquidate_ratio`、`target_ratio`）、`debts`（每支股票的融资本金和未付利息）、`shorts`（每支股票的欠券数量、市值、冻结所得、未付费用）。
- `POST /api/v1/me/credit`：开通，200 返回 `{"credit": ...}`。错误：`credit_open`（409）、`credit_not_eligible`。
- `POST /api/v1/me/credit/repay`：直接还款，请求 `{"amount": 500000}`（省略则用可用资金还清全部），先还利息再还本金。200：`{"paid": 500000, "credit": ...}`。错误：`no_debt`、`insufficient_cash`、`market_closed`。
- `GET /api/v1/stocks/{id}/margin` → `StockMargin`：是否标的（`eligible`，否则 `reason`：`new_listing` / `penny` / `delisting` / `not_listed`）、折算率、可融券数量（`lendable`）、剩余融资额度、融资余额（`financed`，全部账户未还的融资本金）、融券余量（`shorted`，全部账户的欠券股数）、融资利率、融券费率。公开接口。
- WebSocket `me` 主题的 `credit` 消息（§5.3.5）：状态变化时推送。

---

### 4.5a 热度期货（docs/design/futures.md；`[futures] enabled` 打开前，列表为空，下单返回 `futures_closed`）

价格的单位是 0.01 指数点（`IndexPrice`，整数；90.00 点 = 9000）。合约的 `tick_value` 是一手每 0.01 点的价值（分）。模型对合约期平均值的预测和估值一样不公开。

| 接口 | 说明 |
|---|---|
| `GET /futures` | 合约列表：作品、期数、合约期、最新价、前结算价、成交量、持仓量、一手的初始保证金、合约期内已公布的天数和平均指数 |
| `GET /futures/{id}` | 一个合约：同上，加盘口（10 档）、每日结算（OHLC、结算价及其来源、持仓量）、合约期内已公布的指数 |
| `GET /stocks/{id}/heat` | 公司各部作品已公布的热度指数（冻结在游戏库里，只增不改） |
| `GET /me/futures` | 我的期货：持仓（方向用正负号表示；开仓均价 `average_price`，平仓按比例扣除、日结不改变；持仓以来的盈亏 `pnl`；自上次结算以来的盈亏 `unsettled_pnl`；占用的保证金）、未成交委托、保证金汇总和维持担保比例 |
| `POST /me/futures/orders` | 下单：`contract_id`、`side`（buy/sell）、`type`（limit/market）、`quantity`（手）、`price`（限价，0.01 点）。需要信用账户。成交后返回 201 和成交数量、均价 |
| `DELETE /me/futures/orders/{id}` | 撤单，返回最新的“我的期货” |

错误码：`futures_closed`、`futures_not_eligible`（还没开通期货：信用账户、风险测评、知识测试、风险揭示书，见 `Me.futures_steps`；只平仓的委托不检查）、`contract_not_found`（404）、`contract_not_tradable`、`price_out_of_range`、`price_required`、`auction_limit_only`、`insufficient_margin`、`credit_not_open`、`margin_call_active`、`credit_defaulted`、`position_cap`、`margin_cap`、`order_not_found`（404）。

### 4.5b 指数基金（docs/design/funds.md；`[funds] enabled` 打开前，这些接口返回 404 `funds_closed`）

单位净值 `nav` 以星辉为单位（4 位小数，从 1.0000 起），每天收盘后更新。份额以 0.01 份为单位（整数；`units: 120355` 即 1,203.55 份）。申购和赎回在提交后的下一个交易时段成交（08:50 前提交的当天成交），收盘后按当天的净值确认。

| 接口 | 说明 |
|---|---|
| `GET /funds` | 基金列表（按 `[funds] keys` 的顺序）：名称、跟踪指数的全称、分组（market/source/theme）、净值及其日期、今日估算 `estimate`（跟踪指数今天的涨跌）、近一月 `month_change`（成立不满一个月为 null）、成立以来、成分股数、单只权重上限、规模、总份额 |
| `GET /funds/{key}` | 一只基金：同上，加每日净值、前十大持仓（占基金资产和在指数里的权重）、现金比例、费率 |
| `GET /me/funds` | 我的基金：持有份额、按最新净值的市值、持有成本（赎回按份额比例扣除）、持有不满 7 天的份额和全部免赎回费的日期；未结束的申购赎回（pending 可撤，executing 成交中）和最近 30 笔已结束的；未到账的赎回款 |
| `POST /me/fund-requests` | 申购：`fund`、`kind: "subscribe"`、`amount`（分，至少 `min_subscribe`）；赎回：`fund`、`kind: "redeem"`、`units`（0.01 份）。返回 201 和这笔申请、最新的“我的基金”。申购的资金立即转出，赎回的份额立即扣除 |
| `DELETE /me/fund-requests/{id}` | 撤销还没开始成交的申请（资金或份额退回），返回最新的“我的基金” |

`Me.fund_value`：份额按最新净值，加上未确认的申购、赎回和未到账的赎回款；计入 `total_assets` 和排行。持有份额或有未结束的申请时，破产重整返回 `restructure_not_eligible`（`reason: fund_units`），注销返回 `close_not_allowed`。

错误码：`funds_closed`（404）、`fund_not_found`（404）、`insufficient_cash`、`insufficient_units`、`margin_debt`（有融资融券负债时不能申购）、`request_not_cancellable`（404）、`agreement_required`。

### 4.6 排行榜

#### `GET /api/v1/leaderboard`

参数：`period`：`all`（总榜，默认）、`week`（周榜）、`season`（季榜）。

```json
{
  "period": "week",
  "from_day": "2026-11-02", "to_day": "2026-11-06",
  "as_of": "2026-11-06T21:05:00+08:00",
  "items": [
    {"rank": 1, "user": {"id": 4, "username": "alice"}, "total_assets": 10640000, "return": 0.031,
     "restructures": 0, "program": false}
  ],
  "viewer": {"rank": 1, "return": 0.031}
}
```
- `all`：按链接收益率排名（开发中的重整改动已改为这个口径；没重整过的玩家和旧接口按总资产排名的结果一致）。`from_day` = 开服日。
- `week`：本周（周一开始的交易日）的收益率 = 当前 `return_index` / 上周最后一个交易日日结后的 `return_index` − 1。`season`：本季（1/4/7/10 月 1 日起）同理，以上季最后一个交易日为起点。都来自资产快照表（§4.5.10），重整不影响口径。本周或本季才注册的玩家以注册时为起点。
- `return` 是比例（旧 `return_pct`）。排名相同时按 `user.id` 升序，名次不并列。
- `viewer`：登录或机器人代表时给出自己的名次，否则 null。
- `program`：这个周期内（总榜为开户以来）用 API 密钥下过单，页面显示“程序化”。只读密钥不触发。
- 约 40 人，不分页。

---

### 4.7 元数据

- `GET /api/v1/openapi.json`：本接口的 OpenAPI 3.1 文档（由代码生成，§6）。
- `GET /api/internal/health`：`{"ok": true, "version": "0.4.0", "engine_lag_ms": 3}`，给部署脚本和监控用，不稳定。

---

## 5. 实时推送 `/api/v1/ws`

**基础**：另一个分支正在实现的 `/api/ws`（`backend/src/api/ws.rs`、`backend/src/live.rs`，按 `bench/SPEC-v2.md`）。它的结构不动：引擎线程上的变更流扇出、每支股票 100ms 合并、全市场快照每秒一次并按连接 id 分散到 10 个 tick、股票页初始快照分散发送、NDJSON 批量帧、每连接 256 条消息的队列、溢出即断开。本节先记录它现在的消息，再给出 v1 的形式，最后在 §5.5 列出全部差异。

### 5.1 连接

- 地址 `wss://<host>/api/v1/ws`。开发中的 `/api/ws` 在同一个版本里直接改成这个路径，不留别名。
- 认证在握手时完成：
  - 浏览器：`session` cookie（可选）。有效时可订阅 `me`。
  - 机器人：`Authorization: Bearer <token>`（Node 的 `ws` 库可以设请求头；浏览器不能，所以浏览器只用 cookie）。可订阅公开主题和 `events`。
  - 都没有：匿名，只能订阅公开主题。
  - 令牌无效时握手返回 401 和 §2.9 的错误 JSON（不升级）。
- 服务器→客户端：文本帧，内容是**一条或多条 JSON，以 `\n` 分隔**（NDJSON）。客户端按行拆开逐条处理。
- 客户端→服务器：每帧一个 JSON 对象，最大 16 KB。

### 5.2 订阅

```json
{"sub": ["quotes", "stock:21", "news", "me"]}
{"unsub": ["stock:21"]}
{"sub": ["events"], "since": {"events": 10233, "news": 8890}}
{"resync": ["quotes"]}
{"ping": 1730900000}
```

| 主题 | 内容 | 权限 | 合并 |
|---|---|---|---|
| `quotes` | 全市场行情 | 公开 | 每秒最多一次，只发变化的行 |
| `stock:<id>` | 一支股票的盘口、最新价、VCM、新成交 | 公开，每连接最多 16 个 | 每 100ms 最多一次，最新状态 |
| `futures:<id>` | 一个期货合约的快照：合约字段（同 `GET /futures/{id}` 的 `contract`）和盘口各 10 档（`t: "futures"`） | 公开，每连接最多 16 个 | 有变化时每 100ms 最多一条，每条都是完整快照（没有增量，不需要重新同步） |
| `news` | 新资讯 | 公开 | 不合并 |
| `events`、`events:<kind>` | 机器人关心的事件（§5.3.7），`events` 是全部 | 公开 | 不合并 |
| `me` | 自己的委托、成交、现金；期货委托或持仓变化时发 `futures_mine`（只是提示，重新读取 `GET /me/futures`） | 浏览器 cookie 或 API 密钥（§3.6） | 不合并 |

- 每条订阅消息最多 32 个主题，主题名最长 40 字符（沿用实现）。
- 服务器对每条 `sub` 回一条确认（新增）：
  ```json
  {"t": "subscribed", "topics": ["quotes", "stock:21"], "rejected": [{"topic": "me", "code": "unauthenticated"}]}
  ```
  `code` 取值：`unknown_topic`、`unauthenticated`、`forbidden`、`too_many_stocks`、`stock_not_found`。现在的实现静默忽略无效主题，客户端无从知道。
- `since`：断线重连时带上各主题最后见到的 `seq`，服务器尽量补发（§5.4）。
- `resync`：要求服务器对这些状态主题重发全量（客户端发现 `seq` 断档时用）。
- `ping`：应用层心跳，服务器回 `{"t":"pong","ping":<原值>}`。

### 5.3 消息

所有消息都有 `t`（类型）。v1 起所有主题消息都有 `seq`（§5.4）。金额一律整数分。

#### 5.3.1 `hello`（连接后第一条）

现在：`{"t":"hello","me":true}`

v1：
```json
{"t": "hello", "v": 1, "server_version": "0.4.0", "me": true, "bot": false, "now": "2026-11-06T21:05:00+08:00",
 "trading_day": "2026-11-06", "phase": "continuous", "limits": {"stocks": 16, "queue": 256}}
```

#### 5.3.2 `quotes`

现在（金额是浮点币）：
```json
{"t":"quotes","full":true,"s":[[21,220.45,220.45,0,0.0,false]]}
```

v1：
```json
{"t": "quotes", "seq": 5120, "full": true, "trading_day": "2026-11-06",
 "cols": ["id", "price", "prev_close", "volume", "turnover", "vcm_active"],
 "rows": [[21, 22045, 22045, 0, 0, false]]}
```
- 保留按位置的数组：这是压测过的热路径，150 行 × 每秒 × 每个连接，省掉字段名可以少一大半流量。
- 列顺序固定为上面的 `cols`，只在 `full: true` 时附带（客户端校验用）；以后要加列只能加在末尾（兼容的新增）。
- `full: true`：订阅后第一条、`resync` 之后、换交易日之后（昨收全部变化）。`full: false`：只含上一条以来变化的行。
- 不在推送里的列（市值、比率、走势等）由 REST `/market/quotes` 提供，日内只随价格变（客户端可以自己用 `price × shares` 更新市值）。

#### 5.3.3 `stock`

现在：
```json
{"t":"stock","id":21,"init":true,"price":220.45,"prev_close":220.45,"open":null,"high":null,"low":null,
 "volume":0,"turnover":0.0,"open_ref":220.45,"bids":[[220.0,120,3]],"asks":[[221.0,80,2]],
 "vcm":null,"trades":[[88213,220.45,20,"2026-11-06T21:04:13+08:00"]]}
```

v1：
```json
{"t": "stock", "seq": 77012, "id": 21, "full": true, "trading_day": "2026-11-06",
 "as_of": "2026-11-06T21:05:00.300+08:00",
 "price": 22045, "prev_close": 22045, "open": null, "high": null, "low": null,
 "volume": 0, "turnover": 0, "open_reference": 22045,
 "bids": [[22000, 120, 3]], "asks": [[22100, 80, 2]],
 "vcm": {"active": false, "until": null, "low": null, "high": null, "count_today": 0},
 "auction": null,
 "trades": [[88213, 22045, 20, "2026-11-06T21:04:13+08:00"]]}
```
- 档位 `[price, quantity, orders]`，成交 `[id, price, quantity, at]`，都是分。
- `full: true`（原 `init`）：订阅后的第一条，`trades` 是最近 30 笔；之后的消息 `trades` 只含上一条以来的新成交（连接存续期间不会漏）。
- `vcm` 始终是对象（与 REST 的 `Vcm` 相同），不再用 null 表示未触发。
- `auction`：集合竞价阶段的参考价和量 `{"price": 22080, "quantity": 340}`，其他阶段 null（新增，原来只有 REST 详情里有）。

#### 5.3.4 `news`

现在：`{"t":"news","item":{"id","day","ts","kind","title","body","company_id","code"}}`

v1：`{"t": "news", "seq": 8890, "item": <NewsItem>}`，`seq` 就是资讯 id。

#### 5.3.5 `me`：`order`、`fill`、`cash`

现在：
```json
{"t":"order","order":{"id":5521,"company_id":21,"side":"buy","kind":"limit","price":221.0,"qty":100,"filled":60,
 "status":"open","created_at":"…","updated_at":"…","title":"航海王","code":"100123"},"open":true}
{"t":"fill","side":"buy","company_id":21,"title":"航海王","code":"100123","price":220.5,"qty":60,"order_id":5521,"ts":"…"}
{"t":"cash","cash":62300.0,"frozen":4074.0,"available":58226.0}
```

v1（三种消息共用 `me` 主题的 `seq`）：
```json
{"t": "order", "seq": 412, "order": <Order>}
{"t": "fill", "seq": 413, "trade": <与 /me/trades 的一项相同>}
{"t": "cash", "seq": 414, "cash": 6230000, "frozen_cash": 884000, "available_cash": 5346000}
```
- 融资融券（§4.5.11）：信用账户状态变化（追保、强制平仓、穿仓、恢复正常、重整后关闭）时推 `{"t": "credit", "seq": 415, "status": "call", "call_day": "2026-11-06"}`，`status` 为 null 表示信用账户已关闭。
- `order` 去掉多余的 `open` 布尔（看 `status`）。`fill` 的内容改成和 REST 成交记录同一个对象，客户端可以直接插进成交列表。
- 持仓数量的变化不单独推送：客户端收到 `fill` 后自己更新，或者重新请求 `/me/portfolio`。

#### 5.3.6 `time`（新增，心跳兼市场状态）

```json
{"t": "time", "now": "2026-11-07T04:00:00+08:00", "trading_day": "2026-11-07", "phase": "closed",
 "phase_ends_at": "2026-11-07T08:45:00+08:00", "last_closed_day": "2026-11-05"}
```
每 20 秒一次，另外在阶段切换、04:00 换日、日结完成时立即发送。所有连接都会收到，不需要订阅。浏览器的 JS 看不到协议层的 ping（现在的实现每 25 秒发一次 ping，保留，用来穿过反向代理），所以需要这条应用层消息判断连接是否还活着：40 秒收不到任何消息就重连。它同时代替轮询 `/market/status`。

#### 5.3.7 `event`（新增，给机器人）

```json
{"t": "event", "seq": 10234, "kind": "big_move", "at": "2026-11-06T23:41:07+08:00", "trading_day": "2026-11-06",
 "stock": {"id": 191656, "code": "100051", "title": "学生会也有洞！"},
 "data": {"rule": "daily", "change": 0.104, "threshold": 0.1, "window_minutes": null,
          "price": 4358, "reference_price": 3946, "prev_close": 3946},
 "news_id": null}
```

| kind | 什么时候 | `data` | 关联资讯 |
|---|---|---|---|
| `flash` | 热度快讯发布 | `{"episode": 1180, "surprise": 0.35}` | 有 |
| `big_move` | 两条规则（已决定，Q11），见表后说明 | `{"rule", "change", "threshold", "window_minutes", "price", "reference_price", "prev_close"}` | 无 |
| `vcm` | 触发冷静期 | `{"low", "high", "until", "count_today"}` | 有（`vcm` 资讯） |
| `ipo_open` | 新股开始认购（日结时） | `{"ipo": <Ipo，无 viewer>}` | 有 |
| `ipo_allotted` | 配售结果公布 | `{"ipo": <Ipo>}` | 有 |
| `listed` | 新股上市首日开盘 | `{"price", "ipo_price"}` | 无 |
| `delisting_warning` | 退市预警 | `{"weeks_left": 1}` | 有 |
| `delisted` | 退市 | `{"last_price"}` | 有 |
| `daily_close` | 日结完成 | 见下 | 无 |
| `leaderboard_weekly` | 周日的日结之后（周榜结算；市场每天都交易，一周按周一到周日的交易日算） | `{"from_day", "to_day", "top": [<排行榜的前 10 项>]}` | 无 |

`daily_close` 的 `data`：
```json
{"trading_day": "2026-11-05",
 "indices": [{"key": "all", "value": 1038.9, "change": -0.0042}],
 "breadth": {"up": 58, "flat": 20, "down": 63},
 "top_gainers": [{"stock": {"id": 191656, "code": "100051", "title": "学生会也有洞！"}, "change": 0.231}],
 "top_losers": [],
 "dividends_paid": 1824000, "policy_rate": 0.03}
```
（`top_gainers`、`top_losers` 各 3 支。）

`big_move` 的两条规则（阈值在 `game.toml` 的 `[api.events]` 里可调，触发时写进 `data`，机器人不用自己知道配置）：

| `rule` | 条件 | `reference_price` | 去重 |
|---|---|---|---|
| `daily` | 相对昨收的日涨跌幅 \|change\| ≥ 10%（`big_move_daily = 0.10`） | 昨收 | 每支股票每个方向每个交易日一次；之后每再多 10%（20%、30%…）再发一次，`threshold` 随之变 |
| `window` | 相对 30 分钟前的价格涨跌 ≥ 5%（`big_move_window = 0.05`、`big_move_window_min = 30`） | 30 分钟前的最近成交价 | 同一支股票同一方向，上次触发后 30 分钟内不再发 |

- `change` 是按 `reference_price` 算的比例（带符号）；`threshold` 是越过的那一档；`window_minutes` 在 `daily` 规则下为 null。
- 两条规则可以同时触发，各发一条。服务器只负责按规则发事件；群里的推送频率（比如每个群每支股票每小时最多一条、夜间合并）由机器人插件控制。

- `stock` 在 `daily_close`、`leaderboard_weekly` 里为 null。
- `big_move` 和 `listed`、`daily_close`、`leaderboard_weekly` 是新的派生事件，其余对应已有的资讯。阈值写进 `game.toml`。
- 机器人订阅 `events:flash`、`events:big_move` 这样的单个类型，或者 `events` 全部。

### 5.4 顺序、合并与重新同步

**`seq`**：每个主题一个单调递增的计数器（`quotes` 全局一个，`stock:<id>` 每支一个，`news` 用资讯 id，`events` 全局一个，`me` 每个账号一个）。同一主题的消息在一条连接上按 `seq` 顺序到达。

**保证**：

| 主题 | 类型 | 保证 |
|---|---|---|
| `quotes` | 状态 | 每条连接每秒至多一条；每条包含上一条以来所有变化的行的**最新值**；`seq` 连续（每个快照编码一次，每个连接都收到） |
| `stock:<id>` | 状态 | 每 100ms 至多一条；字段是最新状态；`trades` 包含上一条以来的全部新成交；`seq` 连续 |
| `futures:<id>` | 状态 | 每 100ms 至多一条；每条是完整快照，`seq` 每个合约一个，漏掉一条不影响下一条 |
| `news`、`events`、`me` | 增量 | 不合并，按发生顺序；连接存续期间不丢 |
| 任何主题 | — | 队列满 256 条时服务器关闭连接（关闭码 4008），不会静默丢消息 |

**客户端的同步步骤**（网页和机器人都一样）：

1. 连接，发 `sub`。在 REST 结果回来之前，把收到的消息缓存起来。
2. 请求对应的 REST 快照（`/market/quotes`、`/stocks/{id}/book`、`/me/orders?status=open`…）。
3. 把缓存的消息应用到快照上，丢掉比快照旧的：状态消息比 `as_of`，成交、资讯按 id，委托按 `id` + `updated_at`。合并都是幂等的（按 id 覆盖），重复应用没有副作用。
4. 之后：状态主题的 `seq` 断档 → 发 `resync`；增量主题的 `seq` 断档 → 用 REST 的 `after` 补齐。

**断线重连**：

- 按 1、2、5、10 秒（加随机抖动，上限 10 秒）退避重连。
- 重连后带上 `since`。服务器为 `news` 和 `events` 保留最近 24 小时（最多 2000 条）的消息：`since` 在范围内就按顺序补发，之后照常推送；超出范围，或者要补发的超过 128 条（发送队列的一半），回 `{"t": "reset", "topic": "events"}`，客户端用 REST `?after=` 补齐。
- `me` 不保留历史（每个账号单独的缓冲不值得），重连后直接重新请求 `/me`、`/me/orders?status=open`、`/me/trades?after=<最后见到的成交 id>`。
- 状态主题重新订阅时自然先收到 `full`。

**关闭码**：

| 码 | 含义 | 客户端 |
|---|---|---|
| 1001 / 1012 | 服务器重启 | 退避后重连 |
| 4001 | 会话失效（退出登录、cookie 过期） | 不带 `me` 重连，或者回登录页 |
| 4008 | 太慢，队列满 | 重连，重新同步 |
| 4029 | 同一 IP 连接太多 | 不自动重连 |

### 5.5 相对于开发中实现的改动（差异清单）

只列需要改的地方；推送结构、合并节奏、批量帧、队列和断开策略都不变。

| # | 改动 | 原因 |
|---|---|---|
| W1 | 所有金额从 `yuan()` 浮点币改为整数分（`quotes` 的 price/prev_close/turnover，`stock` 的价格、档位、成交、VCM 边界，`order`、`fill`、`cash`） | §2.3 |
| W2 | 每条主题消息加 `seq`；`stock` 加 `as_of`；`quotes`、`stock` 加 `trading_day` | 断档检测、和 REST 快照比较新旧 |
| W3 | `quotes.s` 改名 `rows`，`full` 时附 `cols` 列名 | 列顺序自描述，以后能在末尾加列 |
| W4 | `stock.init` 改名 `full`；`open_ref` 改 `open_reference`；`vcm` 未触发时为对象（`active: false`）而不是 null；加 `auction` | 与 REST `Vcm`、`StockDetail.quote` 一致 |
| W5 | `stock.full` 时 `trades` 给最近 30 笔 | 股票页不用再请求 REST 成交就能显示成交明细（实现时确认现在的 init 是否已经这样） |
| W6 | `news.item` 改用 `NewsItem`（`at`、`trading_day`、`category`、嵌套 `stock`） | §2.2、§2.7 |
| W7 | `order` 消息用 REST 的 `Order`（`type`、`quantity`、`filled_quantity`、`stock`），去掉 `open` | 同一对象两处共用 |
| W8 | `fill` 消息改为 `{"t":"fill","trade":<MyTrade>}` | 同上 |
| W9 | `cash` 字段改为 `cash`、`frozen_cash`、`available_cash` | 与 `/me` 同名 |
| W10 | 新增 `subscribed` 确认、`pong`、`time` 心跳、`reset` | 客户端能知道订阅结果、连接存活、补发失败 |
| W11 | 新增 `events` 主题与 `event` 消息（§5.3.7）；`news`/`events` 保留 24 小时的补发缓冲，支持 `since` | 机器人推送；断线补齐 |
| W12 | 握手接受 `Authorization: Bearer`（机器人）；机器人不能订阅 `me` | §3.2 |
| W13 | 路径改为 `/api/v1/ws`，不留别名 | §7 一次切换 |
| W14 | 关闭时带关闭码（4001/4008/4029），握手失败返回 §2.9 的错误 JSON | 客户端区分是否该重连 |
| W16 | `hello` 加 `server_version` | 前端发现服务器更新后刷新（§7.4） |
| W15 | 退出登录或会话过期时，关闭该会话的连接（4001）——当前实现在连接建立时读一次 cookie，之后不再检查 | 退出后不再收到 `me` |

W1–W9 是同一个改动（编码函数换成 v1 的 serde 结构体）。开发中的推送如果先于 v1 上线，前端先按它现在的格式接入，v1 发布时随整个前端一起切到 v1 格式（§7.2 第 5、7 步）。

### 5.6 机器人推送走 WebSocket（已决定，Q2）

Koishi 本身是常驻进程，主动连出去不需要公网地址或回调鉴权，复用同一套推送和 `since` 补发；服务器不用实现出站请求的重试队列。不提供 webhook。

---

## 6. 类型从哪里来

| 方案 | 怎么做 | 优点 | 缺点 |
|---|---|---|---|
| **A. utoipa（推荐）** | v1 处理函数的请求和响应都是 serde 结构体，加 `#[derive(ToSchema)]` 和 `#[utoipa::path]`；构建时输出 `openapi.json`；前端和机器人用 `openapi-typescript` 生成 `.d.ts`，配 `openapi-fetch`（约 2 KB）做有类型的请求 | 代码就是规范，不会走样；OpenAPI 对机器人作者和其他语言都可读 | 宏标注有些啰嗦；WebSocket 消息不在 OpenAPI 的路径模型里 |
| B. ts-rs / specta | 结构体直接导出 TS 类型 | 最简单，WS 消息也能直接导出 | 没有接口描述（路径、参数、错误），机器人之外的人看不到规范 |
| C. 先写 YAML | 手写 OpenAPI，Rust 端照着实现，前端生成 | 设计评审方便 | Rust 端没有好用的服务器代码生成，规范和实现靠测试对齐，迟早走样 |

做法细节（方案 A）：

- 新接口本来就要从旧的 `J`/`obj!` 手工拼 JSON 改成 serde 结构体（`v2.rs` 已经在用 `serde_json`），加 utoipa 标注的增量成本很小。
- `Money`、`Ratio`、`TradingDay`、`Timestamp` 定义成 newtype（`struct Money(i64)`，`#[schema(value_type = i64, format = "cents")]`），生成的 TS 里是带注释的别名。
- WebSocket 消息定义成一个 `#[serde(tag = "t")]` 的 Rust 枚举，同样 `derive(ToSchema)`，在 OpenAPI 的 `components.schemas` 里登记为 `WsServerMessage`、`WsClientMessage`（不挂路径），前端和机器人从同一个文件生成类型。
- 生成的 `web/src/api/schema.d.ts` 提交进仓库；CI 重新生成并 `git diff --exit-code`，接口变了而类型没更新就失败。
- 机器人插件引用同一个生成文件（发布成一个只有类型的小包，或者直接复制，接口稳定后再考虑）。
- `docs/api-v1.openapi.yaml` 是这份设计的手写版本，用于评审；已由生成的 `openapi.json` 取代并删除。CI（`.github/workflows/backend.yml`）重新生成 `openapi.json` 并和仓库里的比较。

---

## 7. 迁移计划：一次切换

**已决定（Q12）**：不设并存期。目前只有我们自己的前端在用接口，机器人还没有，所以在**同一个分支、同一个版本**里实现 `/api/v1`、迁完整个前端、删除旧接口、`/api/v2/*`、`/api/covers`、`/api/ws` 和全部兼容代码。没有别名，没有过渡路由。

### 7.1 决定 D1：停用 Python 回退路径（已确认）

**停用的**：在当前数据库上运行 `server/app.py`（FastAPI 游戏服务器）作为回退手段，以及为此存在的一切兼容约束：

- 旧接口和 Python 服务器逐字节一致（`api_diff.py` 检查的内容）；
- `api/mod.rs` 的 Starlette 路由模拟（声明顺序匹配、405/307 行为、`{"detail": ...}`）；
- `api/json.rs` 对 Python `json.dumps`/`json.loads` 的复刻（浮点 `repr`、int/float 区分、错误位置）；
- `api/validate.rs` 对 pydantic 422 报错的复刻；
- 只为兼容输出存在的 `pyfmt` 部分。

**不受影响的**：`collect.py` 采集器、`engine/`（经济模型）、`engine/daily_inputs.py`（日结输入）、`server/seed.py`（开服建库）、`sim/`。它们不经过 HTTP 接口。

**为什么可以停**：数据库已经过 V2/V3 迁移（估值状态），即将有 V4（破产重整）、V5/V6（绑定、资产快照）；NPC 市场、热度快讯、夜盘、重整都只在 Rust 里。Python 服务器在这个数据库上要么拒绝启动，要么跑出错误的日结。真要回退，唯一可行的是恢复备份并换回上一个版本的 Rust 程序，这条路不受影响（见 7.5）。

### 7.2 分支内的工作顺序

一个分支（如 `api-v1`），按下面的顺序提交；每一步结束时分支都能编译、测试通过，但只有最后一步之后才发布。

| 顺序 | 内容 | 依赖 |
|---|---|---|
| 1 | **地基**：`/api/v1` 用 axum 自己的 `Router`；错误类型和代码目录（§2.9）；认证提取器（cookie / 机器人令牌 / `X-Act-As`）；分页工具；`Money` 等 newtype；utoipa 与 `/api/v1/openapi.json`；限流（§2.12）；ETag；每个响应带 `AniEx-Version` | — |
| 2 | **迁移文件**：V5 绑定（`bind_codes`、`bindings`），V6 资产快照（`player_days`，§4.5.10），连同快照的一次性回填和日结写入 | 重整（V4）已合并 |
| 3 | **公开读接口**：market（status、quotes、indices、airings、macro）、rules、stocks（详情、搜索、book、trades、candles、intraday、cover）、news、ipos、leaderboard（三个 period） | 1、2 |
| 4 | **账户接口**：auth、me、portfolio、asset-history、orders 读写、trades、ledger（含 `reason_label`）、ipo 认购、restructure、绑定 | 1、2 |
| 5 | **WebSocket v1**：§5.5 的 W1–W16，直接替换开发中的 `/api/ws` 的编码和路径；`events` 主题和补发缓冲 | 1；开发中的推送已合并 |
| 6 | **等价核对**（一次性，§7.3）：旧接口和 v1 同时还在，跑一遍 | 3、4 |
| 7 | **前端整体迁移**：生成类型；`api.ts` 换成 `openapi-fetch`；按 store 改：`market`、`stock`、`ipo`、资讯页、`account`、`session`、排行页；推送客户端换成 v1 格式；版本检测（§7.4）。删除 `MESSAGES`、`translateValidation`、`NEWS_GROUP`、`LEDGER_REASON`、所有 `×100` 换算 | 3、4、5 |
| 8 | **删除**：21 个旧路由、`/api/v2/*`、`/api/covers`、`/api/ws`、Starlette 兼容分发器、`json.rs` 的 Python 兼容部分、`validate.rs`、只为兼容存在的 `pyfmt` 函数；`api_diff.py`、`api_setup.py`、`v1_frozen.py`。SPA 兜底保留：未知的 `/api/*` 返回 404 `not_found`（新格式） | 6、7 |
| 9 | **验收**：契约测试全绿（§7.3）；`grep -rn "/api/[a-z]" web/src` 只剩 `/api/v1`；本地按 §7.6 的清单手动过一遍全部页面；压测（`bench/loadgen`）的 WS 部分改用 v1 格式后重跑 | 8 |

第 3、4、5 步互不依赖，可以并行开发。

### 7.3 怎么验证

**契约测试**（长期保留，替代 `api_diff.py`）：

1. **快照测试**：沿用 `api_diff.py` 的场景和冻结时钟（open、auction、closed、openfail、nodist），在同一个种子数据库上依次请求每个 v1 接口，响应录成快照文件（`insta` 一类），之后每次改动比对。时间戳由冻结时钟决定，快照稳定。
2. **结构校验**：每个快照用生成的 `openapi.json` 校验（字段齐全、类型对、可空性对、枚举值在范围内）。CI 里同时检查生成的 TS 类型文件没有过期。
3. **错误路径**：每个错误代码至少一个用例，检查状态码、`code`、`message` 文案。
4. **WebSocket**：脚本化客户端：订阅后先收到 `full`；下单后收到 `order`、`fill`、`cash` 且 `seq` 连续；断线重连带 `since` 能补发 `news`、`events`；超出缓冲收到 `reset`；慢客户端被 4008 断开。

**新旧等价核对**（第 6 步跑一次，删除旧接口前的最后一道检查，不进 CI）：

- 同一个冻结时钟的数据库副本，同一组场景，每个旧接口和它在 §7.7 对照表里的 v1 接口各请求一次。
- 一个脚本把旧响应按对照表转换：金额 ×100 取整、字段改名、`StockRef` 展开、拆分的接口合起来比、合并的接口分开比；然后和 v1 的值逐项比较。
- 允许的差异写在脚本里并逐条说明（新增字段、`holders` 口径、`airings` 的切分时间、`return` 的链接口径）。其余任何差异都要先查清楚再继续。
- 写操作同样比较：在两份数据库副本上分别用旧接口和 v1 下同样的委托、认购、撤单，比较之后的数据库表（复用 `api_diff.py` 比较数据库的那部分代码）。
- 结果（通过 / 允许的差异清单）贴在发布的 PR 描述里。之后脚本和旧接口一起删除。

**引擎差分测试**（`backend/src/bin/difftest.rs` 和 `difftest/driver.py` 等）不经过 HTTP，不受这次迁移影响，暂时保留。当某条规则在 Rust 里有意改变时，把 Python 一侧的输出录成 golden 文件，之后只和文件比；全部分叉后整套换成 golden 测试。

### 7.4 打开着的旧页面

发布时还开着的浏览器标签页加载的是旧前端，它会请求已经删除的路由，得到 404（新格式的 `not_found`），页面会显示“无法完成操作”。处理：

- **检测版本变化**：每个 HTTP 响应带 `AniEx-Version`，WebSocket 的 `hello` 带 `server_version`（§5.3.1）。前端构建时把自己的版本写进代码；发现服务器版本和自己不同时：
  - 没有正在编辑的表单（下单框为空、没有打开的对话框）时直接 `location.reload()`；
  - 否则显示一条横幅“有新版本，刷新后可用”加“刷新”按钮。
- **这次的旧前端没有这个检测**，所以这一次靠两点：旧前端的轮询请求会在发布后几秒内全部 404，WebSocket 会被服务器重启断开、重连到 `/api/ws` 时得到 404；以及**发布公告**。
- **公告**：发布前在群里通知时间（选在休市时段，例如 04:30–06:00 日结之前，或者 06:40 日结之后到 08:45 集合竞价之前），并说明“更新后刷新页面”。发布后再在资讯里发一条 `system` 资讯。
- 以后的版本都带检测，不再需要靠公告。

### 7.5 发布

- 作为**一个版本**发布（如 v0.4.0）：一个 tag，CI 编译，`deploy/update.sh` 替换程序并重启，前端 `web/dist` 同时更新。
- 启动时自动执行 V5、V6 迁移（迁移前照常自动备份）。V6 的回填在迁移里完成，日志打印回填的天数和玩家数。
- 发布时间选在休市时段（见 7.4），不和 06:30 日结重叠。
- **回退**：恢复迁移前的自动备份，换回上一个版本的程序和 `web/dist`。新表在旧版本里不会被读到；但发布后产生的成交和流水会随回退丢失，所以回退只在发布后很短时间内有意义。决定要回退就尽快。
- 发布后检查：`/api/internal/health`；打开市场、股票、我的三页；下一笔小委托再撤单；看 WS 的 `hello` 里 `server_version` 正确；`journalctl` 里没有 500。

### 7.6 手动检查清单（第 9 步）

市场页（指数、行情表排序筛选、今晚放送、宏观）、股票页（行情条、盘口推送、成交明细、K 线、分时、比率、持有人、受众、新股信息）、搜索、新股页（认购、改量、撤回、已上市）、资讯页（四类筛选、翻页）、我的（总览、持仓、委托、成交、流水翻页、资产曲线、重整按钮的状态、绑定码生成和解绑）、排行（三个标签）、登录注册退出、错误提示（休市下单、资金不足、价格超限）。手机宽度和桌面宽度各一遍。

### 7.7 新旧对照表

| 旧接口 | v1 | 主要变化 |
|---|---|---|
| `GET /api/status` | `GET /api/v1/market/status` + `GET /api/v1/rules` | 拆分；`day` → `trading_day`；删 `trading`；加 `phase_ends_at`、`next_open_at`；分红税档变对象数组 |
| `GET /api/market` | `GET /api/v1/market/quotes` | 合并比率；`StockRef`；分；字段改名（`aired` → `episodes_aired`、`new` → `is_new`、`cooling` → `vcm_active`、`spark` → `sparkline`、`ipo_left` → `ipo_remaining`）；加 `open/high/low`、`cover_url` |
| `GET /api/stocks/{id}` | `GET /api/v1/stocks/{id}` | 合并 profile；盘口、成交、资讯、个人数据拆出；持有人口径统一 |
| 　（其中 `book`、`auction`） | `GET /api/v1/stocks/{id}/book` | 深度可调 |
| 　（其中 `trades`） | `GET /api/v1/stocks/{id}/trades` | 分页 |
| 　（其中 `news`） | `GET /api/v1/news?stock_id=` | 分页 |
| 　（其中 `position`、`my_orders`、`available_cash`） | `/api/v1/me/portfolio`、`/api/v1/me/orders?stock_id=`、`/api/v1/me` | — |
| `GET /api/stocks/{id}/candles` | `GET /api/v1/stocks/{id}/candles` | `time` → `trading_day`；`replay` → `source`；`live` → `is_partial`；区间参数 |
| `GET /api/stocks/{id}/intraday` | `GET /api/v1/stocks/{id}/intraday` | `minute` 带时区；可指定交易日；加 `prev_close` |
| `POST /api/orders` | `POST /api/v1/me/orders` | `company_id` → `stock_id`、`kind` → `type`、`qty` → `quantity`、价格为整数分；返回完整委托和当场成交；`Idempotency-Key` |
| `DELETE /api/orders/{id}` | `DELETE /api/v1/me/orders/{id}` | 返回撤单后的委托 |
| `GET /api/orders?status=` | `GET /api/v1/me/orders?status=` | 分页；`closed`/`all`；`stock_id` 筛选 |
| — | `GET /api/v1/me/orders/{id}` | 新 |
| `GET /api/trades` | `GET /api/v1/me/trades` | 分页；加 `order_id`、`amount`、费用 |
| `GET /api/ledger` | `GET /api/v1/me/ledger` | 分页；筛选；加 `id`、`trading_day`、`ref`、`StockRef` |
| `GET /api/portfolio` | `GET /api/v1/me/portfolio` | 汇总放进 `summary`，持仓放进 `items` |
| `GET /api/me` | `GET /api/v1/me` | 字段改名；`restructure` 对象 |
| `POST /api/login` | `POST /api/v1/auth/login` | 返回 `Me` |
| `POST /api/logout` | `POST /api/v1/auth/logout` | 204 |
| `POST /api/register` | `POST /api/v1/auth/register` | 返回 `Me`；重名 409 |
| `GET /api/leaderboard` | `GET /api/v1/leaderboard?period=all` | 链接收益率；`user` 对象；`viewer` |
| `GET /api/ipos` | `GET /api/v1/ipos` | 合并 listings；`status` 筛选；`my` → `viewer` |
| `POST /api/ipos/{id}/subscribe` | `PUT /api/v1/ipos/{id}/subscription` | PUT 幂等；数量须大于 0 |
| `DELETE /api/ipos/{id}/subscribe` | `DELETE /api/v1/ipos/{id}/subscription` | 幂等 |
| `GET /api/news?limit=` | `GET /api/v1/news` | 游标分页；`category`；`StockRef` |
| `GET /api/macro` | `GET /api/v1/market/macro` | 与 v2 宏观合并 |
| `GET /api/v2/macro` | `GET /api/v1/market/macro` | 同上 |
| `GET /api/v2/indices` | `GET /api/v1/market/indices` | `series` 对象化；`prev` → `prev_close` |
| `GET /api/v2/tonight` | `GET /api/v1/market/airings` | 按交易日切分；`airing_at` → `airs_at`；`aired` → `has_aired` |
| `GET /api/v2/stocks/{id}/profile` | `GET /api/v1/stocks/{id}` | 合并进详情 |
| `GET /api/v2/listings` | `GET /api/v1/ipos?status=listed,upcoming` | 合并进新股 |
| `GET /api/v2/ratios` | `GET /api/v1/market/quotes`（`ratios`） | 合并进行情 |
| `GET /api/covers/{id}` | `GET /api/v1/stocks/{id}/cover` | 路径归到股票下；404 新错误格式 |
| `GET /api/ws`（开发中） | `GET /api/v1/ws` | §5.5；同一版本内替换 |
| `POST /api/me/restructure`（开发中） | `POST /api/v1/me/restructure` | 需 `confirm: true`；返回 `{me}` |
| — | `/stocks/search`、`/me/asset-history`、绑定、机器人接口、`/news/{id}`、`/ipos/{id}`、`/openapi.json` | 新 |

---

## 8. 决定记录

开放问题已于 2026-11-06 全部决定：

| # | 问题 | 决定 | 落在 |
|---|---|---|---|
| Q1 | 停用 Python 回退路径 | 停用（D1 确认） | §7.1 |
| Q2 | 机器人推送用 WebSocket 还是 webhook | WebSocket，不做 webhook | §5.6 |
| Q3 | 金额字段加不加 `_cents` 后缀 | 不加；`Money` 类型写明单位 | §2.3 |
| Q4 | 绑定的对应关系 | 一个账号对一个 QQ 号；换绑须先解绑 | §3.5 |
| Q5 | 每日资产快照表 | 本次就建（周榜、季榜、资产曲线） | §4.5.10、§4.6 |
| Q6 | “今晚放送”的切分 | 按交易日（04:00） | §4.1.5 |
| Q7 | 限流 | 玩家 10 次/秒，机器人令牌 30 次/秒，可配置 | §2.12 |
| Q8 | 流水标签 | 服务器给 `reason_label` | §4.5.8 |
| Q9 | 服务器端会话 | 不在这次 | §3.1 |
| Q10 | 机器人怎么找股票 | 加搜索接口，按中文名、原名、罗马音、英文名、别名和代码模糊匹配，返回带分数的前 N 个 | §4.2.0 |
| Q11 | 大幅波动事件 | 日涨跌 ≥ 10%，或 30 分钟内 ≥ 5%；阈值写进事件；群推送频率由机器人控制 | §5.3.7 |
| Q12 | 迁移方式 | 一次切换，不设并存期 | §7 |

**实现时核对的细节**（不需要再决定）：

- 开发中的推送 `stock` 的初始消息是否已经带最近成交（W5）。
- `/rules` 里各项的实际值以 `game.toml` 为准（价格上限是昨收的 20 倍）。
- `holders.npc.by_style` 在 NPC 市场能按主体类型给出持股之前为 null。
- 破产重整的流水 `reason` 名称以重整分支的实现为准。
- 资产快照回填能否对上当前账户状态（§4.5.10）；对不上时只从迁移当天开始记录。

---

## 9. 实现记录（v0.4.0）

实现按 §7.2 的顺序在一个分支里完成。下面是和上文设计不同、或者设计没有写到而实现时定下来的地方；上文相应位置已经同步修改的不再重复。

### 9.1 与设计不同的地方

| # | 位置 | 设计 | 实现 | 原因 |
|---|---|---|---|---|
| I1 | §5.3.7 | 阈值在 `game.toml` 的 `[events]` | `[api.events]`（`big_move_daily`、`big_move_window`、`big_move_window_min`）；限流在 `[api.rate_limit]` | 和 NPC 市场的 `[market]`、引擎事件分开；所有 API 配置在一个 `[api]` 表下 |
| I2 | §5.3.7 | `leaderboard_weekly` 在“每周最后一个交易日” | 周日的日结之后 | 市场每天交易，没有“周五收盘”；周榜的一周 = 周一到周日的交易日，和 `period=week` 的起点一致 |
| I3 | §4.2.1 | `holders.npc.by_style` 在 NPC 市场给出前为 null | 有 NPC 市场账户时按账户名前缀（价值型、趋势型、消息型、散户型）给出，否则 null | NPC 市场已经上线，账户名就带主体类型 |
| I4 | §4.2.1 | `holders` 各项之和等于 `shares` | 持有记录之和不足 `shares` 时，差额计入 `unissued` | 从未发出的股份不在任何账户里 |
| I5 | §5.3.7 | `flash` 的 `data: {episode, surprise}` | 从资讯标题解析，`surprise` 精确到 1%（标题上的精度） | NPC 市场发快讯时不另存这两个数；不为一个事件改动 `market/` |
| I6 | §2.9 | 错误目录 | 加 `order_rejected`（422）：引擎拒绝了委托、但不属于目录里任何一种时的兜底，`message` 是引擎原文 | 正常情况下不出现（下单前已经逐项检查）；客户端照常显示 `message` |
| I7 | §4.5.9 | `restructure_not_eligible` 的文案只给了 `not_poor` | `this_season`：“本季已经重整过一次”；`market_closed`：“连续交易时段才能申请重整” | 补全 |
| I8 | §4.5.5 | `stock_not_tradable` 文案“这支股票已退市” | 认购中的股票：“这支股票还没有上市”，`details.status` 区分 | 认购中的股票不能下单，但没有退市 |
| I9 | §4.5.5 | `Idempotency-Key` 24 小时去重 | 记在内存里，服务器重启后清空；同一个 key 的并发请求等第一个的结果，被拒绝的委托不记（可以用同一个 key 重试） | 重启时客户端的请求本来就会失败重试；不值得为它建表 |
| I10 | §3.3 | 没有绑定 → 403 `not_bound` | 只要带了 `X-Act-As` 而查不到绑定，任何接口都返回 `not_bound`（公开接口也一样） | 机器人带错头部时尽早发现 |
| I11 | §3.4 | 密码至少 6 位 | 另加上限 128 位（`too_long`） | scrypt 的输入不设上限没有意义 |
| I12 | §4.4 | `IpoStatus` 有 `cancelled` | 不会出现 | 游戏里没有取消发行的机制；值保留在开放枚举里 |
| I13 | §5.2 | `ping` 原样回 | `pong.ping` 在 OpenAPI 里是整数 | 规范示例就是 Unix 秒 |
| I14 | §5.3.1 | 连接后第一条是 `hello` | `hello` 之后紧跟一条 `time` | 客户端马上知道阶段和最近日结，不用等 20 秒 |
| I15 | §2.12 | 限流按 IP | 反向代理后面按 `X-Real-IP`，没有时取 `X-Forwarded-For` 的**最后一项**（代理追加的那一项），只在对端是本机时信任这两个头 | 第一项可以由客户端伪造 |
| I16 | §4.5.10 | 回填重放成交和流水 | 持仓重放：成交、新股中签（按缴款流水的日子）、破产重整和退市（按对应流水的日子清零）；现金重放全部流水。只要有一个玩家对不上就整体放弃回填（日志写明哪个账号、哪一项） | 没有成交记录的股份变动只有这几种 |

### 9.2 设计没写、实现时定下来的

- **开发开关** `ANISTOCK_NO_RATE_LIMIT=1`：关掉所有限流，只给本地测试和自动化检查用（和 `ANISTOCK_ALWAYS_OPEN` 同类）。
- **机器人令牌**：`.env` 的 `ANISTOCK_BOT_TOKENS=name:token[,name:token]`，令牌至少 16 个字符；只保存 SHA-256，常数时间比较。
- **绑定表的写入**：`bind_codes`、`bindings` 不是游戏状态，由 API 用一条单独的连接写（`busy_timeout` 5 秒），不经过引擎线程。
- **行情 `full` 的 `seq`**：订阅或 `resync` 时单发给这条连接的全量快照带的是最近一次增量的 `seq`，之后这条连接跳过这一号增量、从下一号开始收。这样 `seq` 对每条连接仍然连续，也不会把较旧的增量盖到较新的全量上。
- **个股 `full`**：连接在全量快照发出时才进入这支股票的推送名单，所以不会先收到增量再收到全量。`trades` 是本交易日最近 30 笔（W5：开发中的实现原来不带，已补上）。
- **`events` 的 `seq`**：启动时从 Unix 秒 × 1000 开始，重启后仍然递增；重启前的序号都在新进程的补发缓冲之外，带 `since` 重连会收到 `reset`。
- **`/me/orders` 里的 `average_fill_price`** 从这笔委托的成交算；WebSocket 的 `order` 消息里，只有服务器从启动起看到了这笔委托的全部成交时才给出，否则为 null（客户端以 REST 为准）。
- **CI**：重新生成 `openapi.json` 并和仓库里的比较；`schema.d.ts` 由前端的 `npm run gen:api` 生成，CI 的后端任务里没有 Node，不检查。
- **压测**：`bench/loadgen` 压的是 `bench/` 下的原型服务器（`/ws?sym=`），不连游戏服务器，所以不需要改成 v1 格式；v1 推送的行为用脚本化客户端检查（§7.3 第 4 项）。

### 9.3 新旧等价核对的结果（§7.3，第 6 步，一次性）

在 API 差分测试的种子数据库上，冻结时钟的四个场景（`open` 10:00 常开、`auction` 08:50、`closed` 23:30（夜盘中）、`closed0500` 05:00 休市），同一个服务器程序的两份实例：一份只收旧接口的请求，一份只收 v1 的请求。读接口按 §7.7 换算后逐值比较，写操作（限价买卖、成交、市价单、撤单、新股认购改量撤回、注册）分别走两套接口后比较全部表。

- 4468 个值相等，**0 处差异**；四个场景的数据库全部表一致。
- 允许的差异（662 处，三类）：比例按 6 位小数舍入（旧 v2 只读接口是 4 位）；`IpoStatus` 区分 `upcoming`/`listed`（旧接口只有 `open`/`allotted`）；受众数据的形状（删掉的字段、`collection` 补全为五项，以及采集快照里 `"genres": null` 时旧接口整块丢掉 AniList 数据、v1 当作没有类型）。
- 脚本 `backend/difftest/v1_equiv.py` 在核对后随旧接口一起删除（提交历史里保留）。
