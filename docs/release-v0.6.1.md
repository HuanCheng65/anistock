# v0.6.1 发布说明

## 玩家看到的变化

- **欧美热度按天计入估值。** 一集播出后，从第 2 天起每天用这一周已有几天的 AniList 数据修正估值，不再等到整周结束。周内的消息不会再晚一周才反映到估值里。
- **三个市场之间的带动按数据重新拟合。** 新番开播时，一个市场的数据好于或差于预期，会带动另外两个市场的估计。现在三对市场分别拟合：中国和日本之间带动强，它们和欧美之间弱。中国、日本的第一批数据对欧美估计的影响约为原来的一半，开播周里“先被拉低、几天后再回升”的情况少了。
- **热门作品季中衰减更慢。** 开播满 4 周时按欧美热度分三档，热门一档之后的预期衰减比冷门一档每周慢约 1%（按 2018–2025 年的历史数据）。热门作品的估值因此略高。
- 方法和检验见 `docs/v2-valuation.md` §3.8–§3.10。

## 上线步骤（运营）

上线前确认 GitHub 上已经发布了 `v0.6.1` 的服务程序（`deploy/update.sh` 从发布页下载）。避开每天 06:30 的日结，整个过程约 10 分钟。

### 1. 更新

```bash
cd ~/anistock
bash deploy/update.sh
```

- 没有数据库迁移，日志里不会出现 `applied database migrations`。
- 不需要重新生成每日输入，每日输入的格式没有变化。

### 2. 用新模型重建估值（只重启这一次）

估值状态新增了字段（这一周已有几天的欧美数据、热度档）。旧状态可以读取，但正在播出的作品的热度档会按上线当天的水平定，而不是开播第 4 周的水平。重建一次，让所有作品按新模型从输入历史重新计算：

```bash
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user restart anistock-server
journalctl --user -u anistock-server -n 50 --no-pager
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
```

- 日志里应看到 `valuation: started the filter from the inputs history, N companies valued`，N 约等于上市公司数。
- 不应出现 `valuation bootstrap: skipping <日期>: ...` 或 `valuation bootstrap failed`。出现时按 `docs/release-v0.6.0.md` 第 2、3 步检查每日输入后再做一次。
- 一定要取消这个环境变量：留着的话，以后每次重启都会重建一次。
- 重建后估值会一次性变化：热门作品略升，开播周受中国、日本第一批数据影响的作品回到按欧美数据的水平。

### 3. 检查

- 第二天日结之后的日志：没有 `valuation` 相关的警告。
- 正在播出的作品，播出后第 2 天起每天的估值变化应和当天的 AniList 数据方向一致。

### 回滚

停服，用 `ANISTOCK_VERSION=v0.6.0 bash deploy/update.sh` 装回上一个版本，再按 `docs/release-v0.6.0.md` 第 3 步重建一次估值。没有迁移，数据库不需要恢复；不重建的话，正在播出的作品这一周已经按天计入的欧美数据会在整周结束时再算一次。
