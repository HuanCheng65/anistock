# 用脚本访问安易

给写脚本的玩家：怎么拿到密钥、怎么调用接口、有哪些限制。完整的接口定义在站点的 `/api/v1/openapi.json`，字段和错误码的约定在 `docs/api-v1.md`。

## 1. 密钥

在网页的“设置 → API 密钥”里生成，明文只显示一次。

| 权限 | 能做什么 | 条件 |
|---|---|---|
| 只读 | 行情、资讯、自己的账户、持仓、委托、成交、流水；WebSocket 订阅 `me` | 无 |
| 交易 | 另外可以下单、撤单（含融资融券委托）、认购和撤销新股与配股、偿还融资款 | 完成身份验证，签署《程序化交易风险揭示书》 |

- 每个账号同时最多 5 个有效密钥，有效期 30、90 或 365 天。
- 登录、改密码、签署文件、开通融资融券、破产重整、注销、管理密钥只能在网页上做。
- 找回密码时全部密钥吊销。失去身份验证期间交易密钥暂停使用，重新验证后恢复。
- 用交易密钥下过单的账号，在排行榜上显示“程序化”。只读密钥不会。

请求时带请求头：

```
Authorization: Bearer anx_…
```

## 2. 约定

- 金额一律是**整数分**：`22100` 表示 221.00 星辉。请求里的价格也是整数分。
- 时间是带时区的 ISO 8601（`+08:00`），交易日是 `YYYY-MM-DD`。
- 错误统一返回 `{"code", "message", "details"}`，按 `code` 判断，不要按 `message`。

## 3. 限制

| 项 | 限额 |
|---|---|
| 所有请求（按账号，网页和全部密钥合计） | 10 次/秒，允许突发 20 次 |
| 下单和撤单 | 5 次/秒，允许突发 10 次 |
| 每个交易日的委托 | 5,000 笔 |
| 未成交委托 | 同时 100 笔，单支股票 20 笔 |
| 用密钥打开的 WebSocket | 每个账号 3 条 |

超限时：

| 状态 | `code` | 处理 |
|---|---|---|
| 429 | `rate_limited` | 等 `Retry-After` 秒后重试 |
| 429 | `daily_order_limit` | 下一个交易日再下单 |
| 409 | `too_many_open_orders` | 先撤掉一些；`details.scope` 是 `account` 或 `stock` |
| 403 | `forbidden` | `details.required` 列出缺少的条件：`verification`（身份验证）、`api_disclosure`（揭示书）、`api_trading`（本站暂停了 API 交易） |

下单时带 `Idempotency-Key`（任意字符串，最长 64），网络重试时用同一个 key：24 小时内同一个 key 只会下一笔单，重复的请求返回第一次的结果。

## 4. 交易时段

| 时段 | 时间 | 能做什么 |
|---|---|---|
| 集合竞价 | 08:45–09:00 | 只接受限价委托 |
| 连续交易 | 09:00–次日 04:00 | 限价、市价委托；融资融券委托只在这一段 |
| 休市 | 04:00–08:45 | 可以撤单，不能下单（`market_closed`） |

当前时段见 `GET /api/v1/market/status`，或 WebSocket 的 `time` 消息。

## 5. 常用接口

| 用途 | 接口 |
|---|---|
| 全市场行情 | `GET /api/v1/market/quotes` |
| 一支股票的盘口 | `GET /api/v1/stocks/{id}/book` |
| 账户 | `GET /api/v1/me` |
| 持仓 | `GET /api/v1/me/portfolio` |
| 委托 | `GET /api/v1/me/orders`、`POST /api/v1/me/orders`、`DELETE /api/v1/me/orders/{id}` |
| 成交 | `GET /api/v1/me/trades` |

下单：

```json
POST /api/v1/me/orders
{"stock_id": 21, "side": "buy", "type": "limit", "price": 22100, "quantity": 100}
```

201 返回 `{"order": Order, "fills": [...]}`，`fills` 是这笔委托当场成交的部分。

## 6. WebSocket

地址 `wss://<站点>/api/v1/ws`，握手时带同样的 `Authorization` 头。服务器发来的每一帧可能含多条 JSON，按换行拆开。

```json
{"sub": ["quotes", "me"]}
```

- `quotes`：全市场行情，每秒最多一次，只发变化的行。
- `stock:<id>`：一支股票的盘口和成交，每条连接最多 16 支。
- `me`：自己的 `order`（委托状态变化）、`fill`（成交）、`cash`（资金）。
- `events`：热度快讯、大幅波动、冷静期、新股等事件。

每条消息带 `seq`。断线重连时在 `sub` 里带上 `"since": {"me": <最后的 seq>}`，服务器会补发；发现 `seq` 断档时发 `{"resync": ["quotes"]}` 取全量。40 秒收不到任何消息（服务器每 20 秒发一次 `time`）就重连。

## 7. Python 示例

```python
# pip install requests websockets
import asyncio, json, os, uuid
import requests, websockets

BASE = os.environ["ANIEX_URL"]            # 站点地址，如 https://example.com
KEY = os.environ["ANIEX_KEY"]             # anx_…，不要写进代码
H = {"Authorization": f"Bearer {KEY}"}


def api(method, path, **kw):
    r = requests.request(method, BASE + path, headers={**H, **kw.pop("headers", {})}, timeout=10, **kw)
    if r.status_code >= 400:
        e = r.json()
        raise RuntimeError(f"{r.status_code} {e['code']}: {e['message']} {e['details']}")
    return r.json() if r.content else None


def buy(stock_id, price_cents, qty):
    # 同一笔委托的重试用同一个 key，不会重复下单
    key = str(uuid.uuid4())
    body = {"stock_id": stock_id, "side": "buy", "type": "limit", "price": price_cents, "quantity": qty}
    return api("POST", "/api/v1/me/orders", json=body, headers={"Idempotency-Key": key})


async def watch():
    url = BASE.replace("http", "ws", 1) + "/api/v1/ws"
    async with websockets.connect(url, additional_headers=H) as ws:
        await ws.send(json.dumps({"sub": ["me"]}))
        async for frame in ws:
            for line in frame.splitlines():
                m = json.loads(line)
                if m["t"] == "fill":
                    print("成交", m["trade"])
                elif m["t"] == "order":
                    print("委托", m["order"]["id"], m["order"]["status"])


if __name__ == "__main__":
    me = api("GET", "/api/v1/me")
    print(me["username"], "可用资金", me["available_cash"] / 100)
    asyncio.run(watch())
```
