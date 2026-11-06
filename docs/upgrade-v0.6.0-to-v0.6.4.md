# 从 v0.6.0 升级到 v0.6.4（运营）

线上是 v0.6.0 时，一次装到 v0.6.4。v0.6.1–v0.6.3 不用单独部署：v0.6.3 的服务程序没有发布成功（CI 失败），v0.6.4 包含前面所有版本的内容。

各版本的内容：

| 版本 | 内容 | 详细说明 |
|---|---|---|
| v0.6.1 | 估值模型修正、K 线修复、重算开市前走势的工具 | `docs/release-v0.6.1.md`、`docs/ops-rebuild-replay.md` |
| v0.6.2 | 开放注册的准备：注册开关、工作量证明、协议和风险揭示书、身份验证（绑定 QQ）、风险测评、修改和找回密码、注销账号、货币单位改为星辉 | `docs/v2-account.md` |
| v0.6.3 | 网页链接到官方 QQ 机器人；机器人指令改为 `/安易`（`/aniex`）的子命令 | `docs/qq-bot.md` |
| v0.6.4 | 机器人链接和联系邮箱改为 `.env` 配置；修复 v0.6.3 的 CI | 本文 |

整个过程约 45 分钟，第 5 步期间游戏不可用。避开每天 06:30 的日结。

## 0. 准备

- GitHub 发布页上已经有 `v0.6.4` 的服务程序（CI 通过）。
- 磁盘上至少有游戏数据库两倍大小的空余空间（第 2 步和第 5 步各备份一次）。
- 准备好上一个已结算的交易日的日期（下文的 `<上一个交易日>`）。

## 1. 改 `.env`

在 `~/anistock/.env` 里加三行（已经有的就改原来那一行，不要重复）。每个值单独一行，行尾不要写注释：

```bash
cd ~/anistock
openssl rand -hex 32     # 生成机器人令牌，复制输出
```

```bash
ANISTOCK_BOT_TOKENS=koishi:<上面生成的令牌>
ANISTOCK_QQ_BOT_URL=https://bot.q.qq.com/s/wvxT7Td8qI
ANISTOCK_CONTACT_EMAIL=aniex@huanchengfly.top
```

- 令牌要同时填进 QQ 机器人的配置里，机器人请求时带 `Authorization: Bearer <令牌>`。
- 服务器只保存令牌的哈希，丢了就重新生成一个，两边一起换。

## 2. 更新程序

```bash
ANISTOCK_VERSION=v0.6.4 bash deploy/update.sh
journalctl --user -u anistock-server -n 50 --no-pager
```

- 脚本会拉代码、构建前端、下载服务程序、重启服务。
- 启动时自动执行数据库迁移 V11–V13（协议、测评、密码），执行前先把数据库备份到 `data/backups/`。日志里应有 `applied database migrations`。
- 日志里不应有 `ANISTOCK_BOT_TOKENS is not set`。

## 3. 重新生成每日输入

作品的中国、日本数据源改为按时点判断（v0.6.1），重建估值和重算走势都读这些文件：

```bash
set -a; source .env; set +a
uv run sim/reconstruct.py airtimes
uv run sim/reconstruct.py build
uv run engine/daily_inputs.py --from 2026-07-01 --day <上一个交易日> --force
```

- 先用新代码重建中国、日本序列（`data/derived/markets.json`）：它由每天日结前的旧代码生成，不重建的话输入会沿用旧规则（2026-11-06 的教训，docs/revalue-v0.6.7.md）。`build` 输出里的 `extended` 应为 0。

## 4. 用新模型重建估值

```bash
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user restart anistock-server
journalctl --user -u anistock-server -n 50 --no-pager
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
```

- 日志里应有 `valuation: started the filter from the inputs history, N companies valued`，N 约等于上市公司数。
- 不应有 `valuation bootstrap: skipping ...` 或 `valuation bootstrap failed`。
- **一定要取消这个环境变量**，否则以后每次重启都会重建一次。
- 估值会一次性变化：冷门作品（尤其是已完结的）下降，热门作品略升。

## 5. 重算开市前走势

```bash
systemctl --user stop anistock-server
set -a; source .env; set +a
uv run python -m server.tools.rebuild_replay data/game.db --dry-run --work /tmp/rebuild-replay
```

预演结果检查三点，没问题再执行：

- 每支股票最后一列收盘价“原”和“新”相同；
- 缩放比例大多在 0.5–2 之间；
- `kept as is` 的股票不多。

```bash
uv run python -m server.tools.rebuild_replay data/game.db --history /tmp/rebuild-replay/history.json
systemctl --user start anistock-server
rm -rf /tmp/rebuild-replay
```

- 执行前自动备份到 `data/game-before-rebuild-<时间>.db`。
- 最后一行应是 `rewrote the replay history of N stocks; untouched tables verified (...)`。
- 出错时数据库保持原样。

## 6. 检查

- 网页登录后出现“协议更新”页：同意用户协议和隐私政策、签署风险揭示书后才能下单。老玩家都会看到这一页。
- “设置 → 身份验证”里，“机器人”是链接，点开是加好友页面。
- 生成绑定码，私聊机器人发送 `/安易 绑定 <绑定码>`，网页显示“已通过 QQ 完成”。
- 用户协议、隐私政策的“联系方式”一节显示邮箱。
- 任意一支股票的 K 线：开市前的走势和开市后接得上，开市处有一条细线。
- 第二天日结之后的日志里没有 `valuation` 相关的警告。

## 对老玩家的影响（上线前在群里说一声）

- 下次登录要先同意协议、签署风险揭示书，约一分钟。
- 身份验证就是绑定 QQ。没绑定的账号不能领财政补贴、认购新股、进入排行榜，绑定后恢复。
- 融资融券：已经开通的不受影响。新开通需要注册满 14 天、风险测评 C4 以上、通过知识测试。注册天数在 `server/game.toml` 的 `[margin] min_days`。
- 货币单位从“币”改为“星辉”，数额不变。
- 注册仍然需要邀请码（`[account] registration = "invite"`）。

## 回滚

1. 停服：`systemctl --user stop anistock-server`。
2. 恢复数据库：
   - 已经做了第 5 步：用 `data/game-before-rebuild-<时间>.db` 恢复。
   - 回到 v0.6.0：用 `data/backups/` 里迁移前的备份恢复（v0.6.0 不认识 V11–V13 的表）。
3. 装回旧版本：`ANISTOCK_VERSION=v0.6.0 bash deploy/update.sh`。
4. 按 `docs/release-v0.6.0.md` 第 3 步重建一次估值。
