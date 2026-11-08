# 外部新闻

路线图第二阶段第 2 项的“新闻事件”（docs/roadmap.md）：番剧媒体的新闻经 news-feed 的股市扩展判断后，进入安易。原则沿用路线图总则（信号可靠、不留可以机械套利的规律）。**待定**表示数值是建议的默认值。

## 1. 已定事项（2026-11-08）

| 事项 | 决定 |
|---|---|
| 链路 | 番剧信源（animeanime、ANN，每 5 分钟）→ news-feed 合并去重 → 股市扩展（放在 AniEx 的 Koishi 插件里，和身份验证同一个插件）用 Jev 判断 → 把握足够的推给安易 `POST /api/v1/bot/news`（机器人令牌） |
| 中文标题 | 通过门槛的条目由扩展用 DeepSeek 写一句中文标题（每天几条，费用可以忽略） |
| 隐藏估值 | 新闻**不进**隐藏估值。续作、延期等官方消息，估值照旧在日结时从结构化数据（AniList 关联、Annict 排期）读到；新闻只是更早的信号 |
| 对市场 | 只作为情绪事件（按半衰期衰减，按可信度打折、设上限），和开播、完结同一种机制 |
| 分三档上线 | `shadow`（只存档）→ `publish`（发资讯，不影响市场）→ `market`（进入 NPC 市场）|

## 2. 推送格式

`POST /api/v1/bot/news`，`Authorization: Bearer <机器人令牌>`：

```json
{"event_id": "1234", "source": "ann", "url": "https://www.animenewsnetwork.com/news/…",
 "title": "公布第二季", "original_title": "… Anime Gets 2nd Season",
 "published_at": "2026-11-08T09:30:00+09:00", "fetched_at": "2026-11-08T08:35:12+08:00",
 "stock_id": 21, "event": "sequel", "direction": "positive", "magnitude": 6.5, "credibility": "official",
 "model": "jev-1.14.0",
 "probabilities": {"works": {"21": 0.97, "none": 0.03}, "event": {"sequel": 0.91, "movie": 0.05, "…": 0.04},
                   "direction": {"positive": 0.95, "negative": 0.01, "neutral": 0.04},
                   "credibility": {"official": 0.88, "media": 0.11, "rumor": 0.01}}}
```

| 字段 | 规则 |
|---|---|
| `event_id` | news-feed 的事件 id，最长 64。同一个 id 再推是更新判断：存档更新，已经发布的资讯**不改**（玩家可能已经按它交易） |
| `title` | 中文标题，最多 60 字。不含作品名时，游戏在前面加《作品名》 |
| `event` | `sequel`、`movie`、`delay`、`hiatus`、`sales`、`award`、`collab`、`controversy`、`staff`、`rumor`、`denial`、`other` |
| `direction` | `positive`、`negative`、`neutral` |
| `magnitude` | 0–10 |
| `credibility` | `official`、`media`、`rumor` |
| `probabilities` | Jev 每个问题的完整分布，原样存档，用于校准 |
| `published_at` | 可选；`fetched_at` 必填。两者之差是信源到我们的延迟 |

合并：`{"event_id": "1234", "merged_into": "1240"}` 只把这条标为已合并，不发布、不撤回。

返回 200：`{"id", "status": "stored" | "published" | "merged", "news_id", "mode"}`。错误：`invalid_request`、`stock_not_found`、`stock_not_tradable`、`forbidden`（不是机器人令牌）、`media_news_not_found`（合并了不存在的事件）。

## 3. 安易这边

- 存档表 `media_news`（V20，不是游戏状态，和 `api_keys` 一样在旁路连接上写）：每个事件一行，含全部判断、概率、三个时间，`label` 列留给人工标注。
- `[news] mode`（`server/game.toml`，`ANISTOCK_NEWS_MODE` 可以覆盖）：
  - `shadow`（默认）：只存档。
  - `publish`：第一次推送时生成一条游戏资讯，类型 `media`、分组“媒体”；正文写信源、可信度（官方发布 / 媒体报道 / 传闻，未经证实）、原标题和链接。
  - `market`：还没有实现（§5）。
- 资讯页的“媒体”筛选随 `publish` 打开（2026-11-08 起）。

## 4. 校准（`shadow` 期间，一两周）

- 人工标注存档里的每一条（作品、事件、方向是否正确），写进 `label`。
- 看 Jev 的概率是否可信：概率 0.9 的判断是否约 90% 正确。据此定扩展的推送门槛（建议起点：作品 ≥ 0.9，事件 ≥ 0.7，**待定**）。
- 量延迟分布（`published_at` → `fetched_at` → `received_at`）。

## 5. `market` 之前要做的

- 情绪事件的参数：各类事件的强度上限、可信度折扣、半衰期；辟谣按反方向。
- 回放里按 `shadow` 期间的分布合成新闻事件，加 `snipe_news` 机器人过门禁（开播、完结这类情绪事件的零延迟抢跑已经验证不赚钱，docs/v2-api-access.md §7.1；新闻的强度可能更大，要单独验证）。抢跑有利润时，让做市商在情绪事件发布时挪动报价（和跟随估值修正同一个机制）。
- 重大官方事件（热门作品公布续作）配合短暂停牌和集合竞价（docs/v2-npc-market.md §14 第二层）。
