# 数据来源条款核对：AniList、Bangumi、Annict

docs/v3-futures.md §3.3 要求上线前逐条核对三家的条款。本文是核对结果，2026-11-08 读取原文。只读核对，没有改代码。

标记：**【解读】** 表示条文没有直接写到，是按文意推断的结论，平台可能有不同理解；没有标记的结论有原文直接支持。本文不是法律意见；期货是否构成赌博之类的法律问题不在三家条款的范围内，只在 §5 提一句。

---

> **2026-11-08 决定**：只做一行就能改的三项（Annict 串行请求、User-Agent、数据来源署名，已改）。公开 API 的原始平台数值、封面缓存、分平台数值维持现状，不联系平台。理由：社团规模的非商业游戏，条款的这几处是从严解读，实际风险低；平台提出要求时再改。下面的分析留作那时的参考。

## 1. 摘要

### 1.1 各平台结论

| 平台 | 总体 | 主要问题 |
|---|---|---|
| Annict | 基本允许，有条件 | 2026-11-25 起新 API 条款明确允许公开“个人を特定できない統計や集計”，分项 jp 没问题；但公开 API 原样返回 `watchersCount` 属于“自分のAPIでの再提供”【解读】；保存用户 id 允许，但要有保留期限；并行拉取违反帮助页的请求方式 |
| Bangumi | 不明确，偏严 | 开发者协议写“不得私自以任何形式将平台数据提供给任何其他第三人使用”。公开 API 原样返回收藏人数、评分、各集吐槽数，按字面是违反的【解读】；派生的每日增量 cn 不明确；封面图缓存后转发，和 docs/game-design.md 的“封面图不转存”不符；User-Agent 不合要求 |
| AniList | 基本允许，有风险 | 非商业免费；“Hoarding or mass collection”“backup or data storage service”两条和采集器“原始 JSON 全部长期保存”有冲突风险【解读】；没有限制再分发或公开派生统计的条文 |

三家条款都**没有**提到赌博、下注、金融产品或用平台数据做结算。

### 1.2 要改的，按紧迫程度

| # | 改什么 | 原因 | 期限 |
|---|---|---|---|
| 1 | 公开 API 不再返回各平台的原始值：`GET /api/v1/stocks/{id}` 的 `audiences`（Bangumi 收藏五项和评分、Annict `watchers`、AniList `popularity`/`score`），`/stocks/{id}/works` 的各集吐槽数。或者整体去掉，或者对 API 密钥调用方不返回、并从 api-v1.md 和 OpenAPI 中去掉。网页和第三方程序用的是同一个接口，Annict 区分的“表示”和“自分のAPIでの再提供”在网络上没有区别，实际的界线是是否作为接口对外提供；个股页不登录也能看，所以“只对登录会话返回”会让未登录的访客看不到这几项，需要取舍 | Bangumi 开发者协议“不得…将平台数据提供给…第三人”（现在就适用）；Annict API 条款第 6 条“自分のAPIでの再提供”（11-25 起）。这两个接口无需认证，任何人可以批量取 | 11-25 前 |
| 2 | 封面：`/api/v1/stocks/{id}/cover` 把 Bangumi 的封面下载到服务器再转发（`fetch_cover`），和 docs/game-design.md“封面图不转存”的承诺不符。要么改成直接引用 Bangumi 图片地址，要么改文档，并在封面旁标注来源 | 封面版权在作品权利人；Bangumi 版权声明只说“遵照 Fair use 原则处理，并标注来源”，没有授权转存 | 期货上线前 |
| 3 | Annict 拉取改为串行：`collect.py` `ANNICT_VIEWER_WORKERS = 4` 改为 1，`sim/backfill_annict_records.py --workers` 默认改为 1 | 帮助页“リクエストは並列にせず、1件ずつ順番に送ってください”，API 条款第 11 条把帮助页的限制纳入条款。总量 300/分钟在上限 600 以内，问题只在并行 | 尽快（一行） |
| 4 | User-Agent 改为带开发者和项目地址，例如 `HuanCheng65/anistock (https://github.com/HuanCheng65/anistock)`（`collect.py`、`engine/flash_fetch.py`、封面的 `AniEx/0.4 (cover cache)`） | Bangumi 要求“带有开发者个人 ID 和应用名称”，开源项目“附上项目主页”，否则“可能会被禁用”。Annict 第 8 条也希望写名称和联系方式，但那条针对网页抓取，API 没有要求 | 尽快（一行） |
| 5 | AniList 原始 JSON 只保留短期（例如最近两次快照），长期只存派生的每日指标；不再做大范围的历史补登；给 contact@anilist.co 发信说明用途 | API 使用条款“Using the AniList API as a backup or data storage service is strictly prohibited”“Hoarding or mass collection … is strictly prohibited”【解读：现在的规模（约 265 部作品每天一次）不像“mass collection”，但“保存原始数据以便以后改公式”接近“data storage”】 | 期货上线前 |
| 6 | `annict_viewers` 定保留规则：作品离开采集范围（不再计算指数）后删除这部作品各集的行；停止使用 Annict API 时全部删除（含备份）。写进 docs/v3-futures.md §3.1 | Annict 第 7 条：利用者情報只能在“アプリケーションの提供に必要な範囲”内保存，停止使用 API 时要全部删除 | 期货上线前 |
| 7 | 规则页的数据来源说明加上链接和 Bangumi 的许可：“条目信息来自 Bangumi（CC BY-SA）” | Bangumi 条目信息“遵循 Creative Commons BY-SA License 协议”；Annict、AniList 不要求署名 | 期货上线前 |
| 8 | 决定是否公开各市场分项（见 §5.2）：公开的常数可以把分项点数反推成原始增量，所以“只公开点数不公开增量”没有区别。真正的选择是公开分项还是只公开总指数 | Annict 明确允许；Bangumi 不明确；AniList 没有规定 | 主会话决定 |
| 9 | 停用 `sim/fetch_comments.py`（取 `next.bgm.tv/p1` 的各集评论） | 不是文档化的 `api.bgm.tv` 接口，开发者协议要求“在 API 提供的接口范围内获取数据”；评论是用户数据。脚本只留时间，研究已完成 | 不急 |
| 10 | 向 Bangumi、Annict 说明期货的用途并询问 §6 的问题 | 期货让玩家有动机去刷平台数据；条款没写，但这是平台最可能反对的用法 | 期货上线前 |

### 1.3 现在就存在的问题

- **Bangumi 原始数据经公开 API 提供给第三方**（#1）：开发者协议从 2022 年起就有效，不是新条款。按字面是违反的，按“协议的本意是禁止转卖或批量转移数据”的宽松理解是灰色地带【解读】。
- **Annict 旧条款**：现行的 2014 年利用規約（到 2026-11-25 为止有效）第 6 条禁止“本サービスの他の利用者の情報を収集する行為”，没有给 API 留例外。逐集拉取观看记录拿到了其他用户的 id，按字面属于收集。新条款 11-25 施行后明确改为“API利用規約で認める範囲を超えて”才禁止，新 API 条款允许在必要范围内保存。Annict 说“これまでAPIを利用していた方の多くは、使い方を変えずに引き続き利用できます”。剩 17 天，影响小，但这是事实【解读】。
- **Annict 并行拉取**（#3）和 **User-Agent**（#4）：帮助页和 README 的要求，现在就不符合。
- **封面转存**（#2）：和项目自己的合规承诺不符。

---

## 2. 读过的条款

| 平台 | 文件 | 地址 | 版本 |
|---|---|---|---|
| Annict | API利用規約 | https://annict.com/terms/api | 2026-10-25 制定，**2026-11-25 施行**，首版 |
| | 利用規約 | https://annict.com/terms | 2026-10-25 改定，2026-11-25 施行 |
| | 利用規約（现行旧版） | https://annict.com/terms/archive/20140310 | 2014-03-10 施行，到 2026-11-24 有效 |
| | プライバシーポリシー | https://annict.com/privacy | 2026-10-25 改定并施行 |
| | Annictのレート制限 | https://wikino.app/s/annict/pages/419 | 无日期，2026-11-08 读取 |
| | Annict APIの認証 | https://wikino.app/s/annict/pages/356 | 无日期，2026-11-08 读取 |
| Bangumi | 版权声明（含“Bangumi 开发者平台使用协议”） | https://bgm.tv/about/copyright | 最后更新 2022-10-04 |
| | 社区指导原则 | https://bgm.tv/about/guideline | 最后更新 2008-08-06 |
| | API README、User Agent 建议 | https://github.com/bangumi/api （`README.md`、`docs-raw/user agent.md`） | master，2026-11-08 读取 |
| | Archive README | https://github.com/bangumi/Archive | master，2026-11-08 读取；仓库**没有 LICENSE** |
| | 用户协议、隐私政策 | bgm.tv/about/tos、/about/privacy、/about/terms | **不存在**（404）；站点页脚只有关于、指导原则、版权声明、链接我们 |
| AniList | API Terms of Use | https://docs.anilist.co/guide/terms-of-use | 无日期，2026-11-08 读取 |
| | Rate Limiting、Considerations | https://docs.anilist.co/guide/rate-limiting 、/guide/considerations | 无日期，2026-11-08 读取 |
| | 网站 Terms（anilist.co/terms 指向 iubenda） | https://www.iubenda.com/privacy-policy/909656/full-legal | Latest update: April 09, 2026 |

---

## 3. 本站怎么用这些数据（核对的对象）

| 用途 | 内容 | 位置 |
|---|---|---|
| 采集 | 每天一次。AniList：在播作品的 GraphQL 数据和 trends（本机开发库约 265 部，生产库同一量级）；Bangumi：`api.bgm.tv` 的条目和各集（收藏人数、吐槽数），每小时一次各集吐槽数（热度快讯）；Annict：GraphQL 作品、各集、`recordsCount`，以及各集新增的观看记录（只要记录人 id 和时间） | `collect.py`、`engine/flash_fetch.py` |
| 回填 | Bangumi 每周 Archive（58 份）；Annict 四季的观看记录（只存每天的计数）；AniList 四季补登 | `sim/backfill_*.py` |
| 存储 | 原始 JSON 快照长期保存（`data/anistock.db`）；`annict_viewers (episode_id, user_id, first_at)` 用于去重和刷量监控，不公开；数据不进仓库（`.gitignore`） | `collect.py` |
| 公开（页面） | 指数和三个市场分项、Bangumi 评分和在看人数、AniList 评分和人气、Annict 观看人数、各集吐槽数、封面 | 个股页 |
| 公开（API） | 同上，无需认证：`/stocks/{id}`（`audiences`）、`/stocks/{id}/works`、`/stocks/{id}/heat`、`/futures/{id}` 的每日 `us`/`cn`/`jp`、`/stocks/{id}/cover` | `backend/src/api/v1/` |
| 游戏 | 营收按平台热度分配；热度期货按 4 周平均指数现金结算；虚拟货币星辉，不能充值或提现，没有广告和付费 | docs/game-design.md、docs/v3-futures.md §4 |
| 署名 | 规则页和设置页页脚：“作品数据来自 AniList、Bangumi、Annict 的公开统计。” | `web/src/views/RulesView.vue` |

`us`/`cn`/`jp` 是经过校验的每日热度：us 是 AniList 几个量的加权组合，cn 是 Bangumi 标记人数的日增量，jp 是 Annict 按账号和集去重后的记录日增量（`engine/heat_index.py`）。

---

## 4. 逐条核对

### 4.1 Annict

新条款 11-25 施行，下面按新条款核对；旧条款的差别见 §1.3。定义（API条款第 2 条）：“作品情報”是作品、集、播出等用户以外的信息；“利用者情報”是“プロフィール・記録・感想・視聴ステータス…”等用户的信息。观看记录和记录人 id 属于利用者情報。

| 用途 | 条款原文 | 结论 | 说明 |
|---|---|---|---|
| 自动采集、频率 | 第 11 条：“運営者が別に定めるレート制限が適用されます”。帮助页：有令牌每分钟 600（按令牌和 IP）；“リクエストは並列にせず、1件ずつ順番に送ってください”；“全件の取得は一度に済ませず、数時間から数日に分けて”；今期作品“1日1回”重新取得 | 有条件允许 | 每日采集间隔 0.3 秒（200/分钟）、每天一次，符合。`--annict-viewers` 首次同步和 `sim/backfill_annict_records.py` 用 4 个并行线程，违反“並列にせず”（§1.2 #3） |
| 账号、令牌 | 第 4 条：个人令牌要“第三者に知られないように保管”；帮助页：GraphQL 需要令牌 | 允许 | 用的是只读的个人令牌，放在服务器环境变量里。第 5 条（共用令牌只能取作品情報）针对“配布物に埋め込む”的令牌，服务器端的令牌不在此列【解读】；但情形相近，列入 §6 |
| 存储作品数据 | 第 7 条：作品情報“保存する期間と件数に上限はありません”“全件を保存することもできます” | 允许 | |
| 存储用户 id | 第 7 条：“アプリケーションの提供に必要な範囲で…保存・キャッシュできます”；隐私政策第 4 节：记录等公开信息经 API 提供，“ユーザーごとにWeb APIでの提供を止める設定はありません”；利用者情報在“ユーザーがアプリケーションとの連携を解除したとき”“APIの利用をやめたとき: すべての利用者情報”删除，“バックアップに残ったものは、通常の保持期間が過ぎたら消去” | 有条件允许 | 去重必须知道账号，属于“必要な範囲”【解读】。用于刷量监控（§4.4）是否算“必要”不明确【解读】。条款没有规定其他保留期限，但“必要な範囲”意味着不需要时应删除：建议作品离开采集范围后删除这部作品的行（§1.2 #6）。可选：用服务器密钥做 HMAC 保存，去重照常，但就不能再按 id 查看账号，和 §4.4 的监控冲突，需要取舍 |
| 公开聚合统计（每作品每日增量、指数） | 第 6 条：“個人を特定できない統計や集計の結果は、転載に当たりません。自由に公開できます。”；利用規約第 14 条：运营方自己也可以“個人を特定できない統計の情報にして、利用・公開” | 允许 | jp 是计数，不含账号。冷门作品某天只有 1 条记录时，计数本身也不能识别是谁【解读】 |
| 公开 API 中的派生统计（`jp`） | 同上 | 允许 | 统计不是“取得したデータ”的原样 |
| 公开 API 中的原始值（`audiences.annict.watchers`） | 第 6 条：“APIで取得したデータを、そのままの形で、またはまとまったデータとして第三者が取り出せるように提供してはいけません。ファイルでの配布、データセットとしての公開、自分のAPIでの再提供などがこれに当たります。”“作品情報をアプリケーションの画面やページに表示することは、これに当たりません。” | 不明确，偏向不允许（11-25 起） | 页面显示允许。公开 API 原样返回 `watchersCount`，正是“自分のAPIでの再提供”；每次只返回一部作品的一个数，可能不算“まとまったデータ”，但接口无需认证，可以逐个取全部作品【解读】。建议去掉（§1.2 #1） |
| 原始数据进仓库 | 第 6 条：“まとまったデータを誰でも取得できるURLや公開のリポジトリに置くこと”属于提供 | 允许（已做到） | `.gitignore` 排除 `data/`。仓库里的 `sim/matches.json` 有约 160 个 Annict 作品 id，第 7 条允许保留 id 用于“紐付け”；放在公开仓库是否算“まとまったデータ”不明确，量小，风险低【解读】 |
| 展示用户信息 | 第 6 条：“転載: 利用者情報をAnnictの外の公開物に載せること…本人が同意した場合を除き、転載してはいけません” | 允许（没有展示） | docs/game-design.md 已规定不展示用户名和评论 |
| 商业 / 非商业 | 第 3 条：“商用か非商用かを問わず、個別の許可なくAPIを利用できます”，例外只有商用 AI 和提供数据集 | 允许 | 不涉及 |
| 赌博、金融产品 | 没有相关条款。利用規約第 6 条禁止“法令に違反する行為”“公序良俗に反する行為”“運営者…に不利益や損害を与える行為” | 条款未涉及 | 期货让玩家有动机去 Annict 刷记录，可能被视为“本サービスの運営に支障が出る使い方”（API 条款第 15 条，可失效令牌、封锁访问）【解读】。§4.4 的去重、用户协议禁止刷量是对应措施；建议主动告知（§6） |
| 署名 | 第 13 条：“データの出典として示すかどうかは自由です”；不能让人误以为是 Annict 官方或合作方 | 允许 | 已有署名，名称里没有 Annict |
| AI / 机器学习 | 第 9 条：非商用可以，不能再分发，成果物不能有可识别的利用者情報 | 允许 | 估值的滤波和拟合即使算机器学习，也是非商用、不再分发 |
| 图片 | 第 10 条：不能再分发、改变图片 | 不涉及 | 封面取自 Bangumi，不取 Annict 的图 |

### 4.2 Bangumi

Bangumi 没有用户协议和隐私政策。约束开发者的是版权声明页里的“Bangumi 开发者平台使用协议”（2022-10-04），它明确覆盖“API 与归档数据”。

| 用途 | 条款原文 | 结论 | 说明 |
|---|---|---|---|
| 自动采集 | 开发者协议：“开发者应当在 API 提供的接口范围内获取数据，不以侵犯或可能侵犯 Bangumi 以及平台用户权益的方式使用网络爬虫或类似的方式进行数据收集” | 允许 | `collect.py` 和 `flash_fetch.py` 只用 `api.bgm.tv`。`sim/fetch_comments.py` 用 `next.bgm.tv/p1`（新前端的接口），是否在“API 提供的接口范围内”不明确【解读】（§1.2 #9） |
| 频率 | 没有公布数值限制 | 允许 | 间隔 0.4 秒，每天一次；热度快讯每小时每部作品一次请求 |
| User-Agent | README：“使用 API 前请查看 User Agent 建议”；建议：“请指定一个带有开发者个人 ID 和应用名称的 User Agent”“开源项目，请在 User Agent 附上项目主页”“各种请求库的默认 UA 可能会被禁用” | 不符合 | 现在是 `anistock-collector/0.1 (anime club market game)`，没有开发者 ID 和主页（§1.2 #4） |
| 账号 | 公开数据不需要令牌 | 允许 | |
| 只取必要的数据 | 开发者协议：“开发者应当仅获取为应用或服务运行及功能实现目的而必要的数据” | 允许 | 只取跟踪的作品；Archive 是整份下载，但它本来就是给“不需要实时数据的场景”用的【解读】 |
| Archive 回填 | Archive README：“定期导出 wiki 数据方便一些不需要实时数据的场景，顺便希望减少一些爬虫”；仓库没有许可证；开发者协议覆盖“归档数据” | 允许【解读】 | 没有单独的许可，按开发者协议和版权声明处理。只保留每周的收藏计数和动画条目名 |
| 存储 | 开发者协议没有期限；“一旦开发者停止使用 Bangumi 开发者平台…必须立即删除全部从 Bangumi 开发者平台中获得的数据”；“Bangumi 有权要求开发者删除相关数据” | 有条件允许 | 停止使用时要删除，包括派生的回填库【解读：派生的计数是否算“获得的数据”不明确】 |
| 用户数据 | “开发者需要收集用户数据的，必须事先获得用户的同意” | 允许（不收集） | 收藏人数、吐槽数是条目级的计数，不是用户数据【解读】。`fetch_comments.py` 拿到过评论（含用户），只保存了时间 |
| 公开页面展示 | “开发者不得私自以任何形式将平台数据提供给任何其他第三人使用” | 有条件允许【解读】 | 字面上连页面展示也算“提供给第三人”，但协议开头就说可以开发“网站与服务”，所以按本意是禁止把数据转给别人用，不禁止在自己的应用里展示 |
| 公开 API 中的原始值（收藏五项、评分、各集吐槽数） | 同上 | 不明确，偏向不允许【解读】 | 公开 API 让第三方的程序取用 Bangumi 的数据，正是“将平台数据提供给…第三人使用”。建议去掉（§1.2 #1） |
| 公开派生统计（每日增量 cn、指数） | 同上 | 不明确【解读】 | 增量是计算结果，不是“平台数据”原样；但配合公开的常数可以还原，和原始数据的日差相同。指数（三个市场的加权和）基本没有问题。见 §5.2 |
| 条目信息（名称、播出日期、集名） | 版权声明：“条目信息（包括但不限于封面、内容介绍、章节信息）、角色信息均由用户提供，遵循 Creative Commons BY-SA License 协议…对于已有版权的作品遵照 Fair use 原则处理，并标注来源” | 有条件允许 | 需要署名（BY）；再分发时相同方式共享（SA）。作品名、日期这类事实信息是否受版权保护本身存疑【解读】。建议署名写明 CC BY-SA（§1.2 #7） |
| 封面图 | 同上；社区指导原则“不提倡…盗图、盗链” | 不明确 | 封面版权在作品权利人，Bangumi 只能“遵照 Fair use…标注来源”。现在缓存到服务器再转发，封面旁没有标注来源（页脚只有笼统的数据来源），也和项目文档“封面图不转存”不符（§1.2 #2）。改为直接引用又碰上“不提倡盗链”，两难；至少标注来源【解读】 |
| 用户原创内容 | “用户所发布的原创日志、吐槽、图片等内容的版权归用户本人所有。未经用户本人许可，禁止转载” | 允许（不展示） | 只用计数，不展示吐槽内容 |
| 商业 / 非商业 | 开发者协议没有区分；“未经许可，不得用于商业目的”只针对 Bangumi 的 logo、网页样式、图形 | 允许 | 没有用 Bangumi 的 logo |
| 赌博、金融产品 | 没有相关条款。社区指导原则禁止“违反中国或 Bangumi 成员所在地法律法规的行为” | 条款未涉及 | 指导原则约束的是站内行为。开发者协议“如果 Bangumi 认为开发者使用用户数据的方式，会损害 Bangumi 用户体验，Bangumi 有权要求开发者删除相关数据”；期货引起刷收藏会损害体验【解读】 |
| 协议变更 | “Bangumi 可根据需要更新本协议且不另行通知” | — | 需要定期复查 |

### 4.3 AniList

| 用途 | 条款原文 | 结论 | 说明 |
|---|---|---|---|
| 自动采集、频率 | Rate Limiting：“The AniList API has a rate limit of 90 requests per minute”；目前“degraded state and is limited to 30 requests per minute”；另有 burst limiter | 允许 | 间隔 2.1 秒（约 28.5 次/分钟），低于 30；按 `X-RateLimit-Remaining` 减速，429 时按 `Retry-After` 等待 |
| 采集规模 | Terms of Use：“Hoarding or mass collection of data from the AniList API is strictly prohibited.”“For purely educational projects…we tend to be very lenient on the 3rd point.” | 有风险【解读】 | 每天约 265 部（本机开发库的数，生产库同一量级）在播作品和四季补登，不像整库抓取。但条款没有给数量标准，社团游戏也不是“purely educational”。建议不再扩大补登范围，并写信说明（§1.2 #5） |
| 存储 | “Using the AniList API as a backup or data storage service is strictly prohibited.” | 有风险【解读】 | 这条更像是禁止把 AniList 当作自己数据的存储后端。但采集器写明“stored as raw JSON in SQLite so metric formulas can change later”，长期保存全部原始响应，可能被看成拿 AniList 数据做存档。建议原始 JSON 只留短期，长期只存派生指标 |
| 公开派生统计（`us`、指数） | API 条款没有关于再分发或展示的条文 | 允许【解读】 | `us` 是几个量的加权组合 |
| 公开原始值（`popularity`、`averageScore`、`genres`） | API 条款没有；网站 Terms 的 Use License：“personal, non-commercial transitory viewing only…may not…use the materials…for any public display (commercial or non-commercial)” | 允许【解读】 | 网站 Terms 是 iubenda 的通用模板，管的是“materials on this Application's web site”，不是 API；而 API 条款允许“build whatever cool project you want”，展示 API 数据是常见用法。为了和 #1 一致，可以一并从公开 API 去掉 |
| 商业 / 非商业 | “Free for non-commercial usage.”“commercial applications…operating at less than $150 of revenue per month free of charge” | 允许 | 没有收入。即使将来有收入，低于每月 150 美元也免费 |
| 竞争服务 | “Use of the AniList API within competing, non-complementary services of the same nature is prohibited…anime and manga list or tracker services” | 允许 | 不是列表或追番服务 |
| 名称 | 名称里用“AniList”要加“UNOFFICIAL”或“for AniList” | 允许 | 名称是“安易”/AniEx |
| 账号、令牌 | 公开数据不需要认证 | 允许 | |
| 用户隐私 | 不取用户数据 | 允许 | |
| 署名 | 没有要求 | 允许 | 已有署名 |
| 赌博、金融产品 | 没有相关条款 | 条款未涉及 | |
| 封禁 | Considerations：“AniList may block your IP address…due to a large number of requests” | — | 不影响结论 |

---

## 5. 共同问题

### 5.1 期货和下注

三家条款都没有提到赌博、下注、金融产品，也没有禁止用平台数据做结算。星辉不能充值或提现，没有广告和付费，所以三家的“商用”条件都不触发【解读：AniList 的 commercial 以收入为准，Annict 本来就允许商用，Bangumi 不区分】。

风险在条款之外：以平台数据结算的合约给了玩家去平台刷量的动机，这正是平台不愿意看到的。Annict API 条款第 15 条允许对“本サービスの運営に支障が出る使い方”失效令牌、封锁访问；Bangumi 有权“限制或阻止开发者获取用户数据及平台数据”。所以在期货上线前主动告知 Annict 和 Bangumi，比被发现后再解释好（§6）。

期货在中国法律下是否构成赌博（虚拟货币、不能兑现）不是这三家条款的问题，本文不作结论。

### 5.2 公开分项可以反推原始增量

编制方法公开了权重、平台规模和常数（`engine/heat_index.py` 的 `WEIGHTS`、`PLATFORM`、`SCALE`），所以“只公开每个市场的指数点数，不公开增量”和公开增量没有区别：点数 ÷ 权重 × 平台规模 ÷ 常数就是增量。可选的做法只有两种：

| 做法 | Annict | Bangumi | AniList | 代价 |
|---|---|---|---|---|
| 公开分项（现在的设计） | 允许 | 不明确 | 允许【解读】 | 无 |
| 只公开总指数 | 允许 | 基本没有问题【解读】 | 允许 | 违背 docs/v3-futures.md §1“每天公布每部作品的指数和各市场分项”，玩家无法复算、核对补值 |

建议先保留分项，同时向 Bangumi 询问（§6）；如果 Bangumi 不同意，再改为只公开总指数。这需要主会话决定。

---

## 6. 需要问平台的问题

| 平台 | 渠道 | 问题 |
|---|---|---|
| Annict | API 条款第 17 条的邮箱（annict.com/terms/api 页末） | 1. 服务器端用一个个人令牌，为不需要 Annict 账号的网站取各集观看记录（含记录人 id），只用于按账号去重和计数、不展示，是否可以？第 5 条的“作品情報だけ”是否适用？ 2. 为了识别刷量账号保存记录人 id，算不算“アプリケーションの提供に必要な範囲”？ 3. 公开 API 返回作品的 `watchersCount` 是否属于第 6 条的“自分のAPIでの再提供”？ 4. 告知：每日计数用于一个虚拟货币游戏的营收和期货结算，已按账号去重防刷 |
| Bangumi | 站务论坛，或 github.com/bangumi/api 的 issue | 1. 开发者协议的“不得…将平台数据提供给任何其他第三人使用”是否包括：网页展示收藏人数和评分；开放 API 返回原始收藏人数；公开由收藏人数计算的每日增量？ 2. 缓存封面再转发是否可以，还是应直接引用 lain.bgm.tv 的地址？ 3. 告知期货用途，询问是否反对 |
| AniList | contact@anilist.co | 1. 每天对约 300 部以内的在播作品各请求一次、保存响应以便重算指标，是否属于“hoarding or mass collection”或“data storage service”？ 2. 历史补登（按季度、人气 1000 以上）是否可以？ |

问完把答复记在本文，按答复调整 §1.2。
