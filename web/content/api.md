# 开发者说明

用 API 密钥，脚本可以读取行情和自己的账户、接收成交推送，也可以下单。接口和字段的完整定义见 [OpenAPI 文档](https://aniex.huanchengfly.top/api/v1/openapi.json)。

## API 密钥 {#keys}

在[设置](/settings)里生成，明文只显示一次。请求时带上请求头：

```
Authorization: Bearer anx_…
```

| 权限 | 范围 | 条件 |
|---|---|---|
| 只读 | 行情、资讯、自己的账户、持仓、委托、成交和资金明细，订阅自己的推送 | 无 |
| 交易 | 另外可以下单、撤单（含融资融券委托），认购和撤回新股与配股，偿还融资 | 完成身份验证，签署[《程序化交易风险揭示书》](/legal/api_disclosure) |

每个账号最多同时持有 5 个有效密钥，有效期 30、90 或 365 天。登录、修改密码、签署文件、开通融资融券、破产重整、注销账号和管理密钥只能在网页上操作。找回密码时全部密钥吊销；身份验证失效期间交易密钥暂停使用，重新验证后恢复。

用交易密钥下过单的账号，在排行榜上标记为“程序化”。

## 数据格式 {#conventions}

金额和价格一律以**分**为单位的整数表示，`22100` 即 221.00 星辉。时间为带时区的 ISO 8601（`+08:00`），交易日为 `YYYY-MM-DD`。

出错时返回 `{"code", "message", "details"}`。`message` 是给人看的说明，措辞可能调整，程序按 `code` 判断。

## 频率与数量限制 {#limits}

| 项目 | 上限 |
|---|---|
| 全部请求 | 每秒 10 次，可短时达到 20 次。按账号计，网页和所有密钥合计 |
| 下单和撤单 | 每秒 5 次，可短时达到 10 次 |
| 每个交易日的委托 | 5,000 笔 |
| 未成交委托 | 同时 100 笔，单支股票 20 笔 |
| 用密钥打开的 WebSocket | 每个账号 3 条 |

超出限制时的返回：

| 状态 | `code` | 处理 |
|---|---|---|
| 429 | `rate_limited` | 等待 `Retry-After` 秒后重试 |
| 429 | `daily_order_limit` | 下一个交易日再下单 |
| 409 | `too_many_open_orders` | 先撤掉一部分。`details.scope` 为 `account` 或 `stock` |
| 403 | `forbidden` | `details.required` 列出缺少的条件：`verification` 身份验证，`api_disclosure` 风险揭示书，`api_trading` 本站暂停了 API 下单 |

网络不稳定时，同一笔委托可能被重复提交。下单时带上请求头 `Idempotency-Key`（任意字符串，最长 64 个字符），重试时沿用同一个值：24 小时内同一个值只下一笔，重复的请求返回第一次的结果。

## 交易时段 {#hours}

| 时段 | 时间 | 可以提交 |
|---|---|---|
| 集合竞价 | 08:45–09:00 | 限价委托 |
| 连续交易 | 09:00 至次日 04:00 | 限价和市价委托；融资融券委托只在这个时段 |
| 休市 | 04:00–08:45 | 只能撤单，下单返回 `market_closed` |

当前时段见 `GET /api/v1/market/status` 或 WebSocket 的 `time` 消息。委托和成交的规则见[规则](/rules#orders)。

## 常用接口 {#endpoints}

| 用途 | 接口 |
|---|---|
| 全市场行情 | `GET /api/v1/market/quotes` |
| 盘口 | `GET /api/v1/stocks/{id}/book` |
| 账户 | `GET /api/v1/me` |
| 持仓 | `GET /api/v1/me/portfolio` |
| 委托 | `GET`、`POST /api/v1/me/orders`，`DELETE /api/v1/me/orders/{id}` |
| 成交 | `GET /api/v1/me/trades` |

下单示例：

```
POST /api/v1/me/orders
{"stock_id": 21, "side": "buy", "type": "limit", "price": 22100, "quantity": 100}
```

成功时返回 201 和 `{"order", "fills"}`，`fills` 是这笔委托当场成交的部分。

## 实时推送 {#ws}

WebSocket 地址为 `wss://aniex.huanchengfly.top/api/v1/ws`，握手时带同样的 `Authorization` 请求头，连接后发送要订阅的主题：

```
{"sub": ["quotes", "me"]}
```

| 主题 | 内容 |
|---|---|
| `quotes` | 全市场行情，每秒最多一次，只发有变化的股票 |
| `stock:<id>` | 一支股票的盘口和成交，每条连接最多 16 支 |
| `futures:<id>` | 一个热度期货合约的盘口和成交，每条连接最多 16 个 |
| `me` | 自己的委托状态（`order`）、成交（`fill`）和资金（`cash`） |
| `news` | 资讯 |
| `events` | 热度快讯、大幅波动、冷静期、新股等事件 |

服务器的一帧可能包含多条 JSON，按换行拆开。每条消息带递增的 `seq`：断线重连时在订阅消息里加上 `"since": {"me": 最后收到的 seq}`，服务器补发中间的消息；发现 `seq` 不连续时发送 `{"resync": ["quotes"]}` 取全量。服务器每 20 秒发一次 `time`，40 秒没有收到任何消息时重连。

## Python 示例 {#example}

读取账户，然后持续打印自己的委托和成交：

```
# pip install requests websockets
import asyncio, json, os, uuid
import requests, websockets

BASE = "https://aniex.huanchengfly.top"
KEY = os.environ["ANIEX_KEY"]   # anx_…，放在环境变量里，不写进代码
H = {"Authorization": f"Bearer {KEY}"}


def api(method, path, **kw):
    r = requests.request(method, BASE + path, headers={**H, **kw.pop("headers", {})}, timeout=10, **kw)
    if r.status_code >= 400:
        e = r.json()
        raise RuntimeError(f"{r.status_code} {e['code']}: {e['message']} {e['details']}")
    return r.json() if r.content else None


def buy(stock_id, price_cents, qty):
    # 同一笔委托重试时用同一个 key，不会重复下单
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
