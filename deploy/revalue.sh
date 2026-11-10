#!/usr/bin/env bash
# A release that changes the valuation, end to end (docs/ops/revalue.md). From the repo root:
#
#   bash deploy/revalue.sh vX.Y.Z [--skip-collect] [--yes]
#
# Stops the game, backs the database up, installs the release (code checked out at its tag), collects once (unless
# --skip-collect), rebuilds the cn/jp series and every daily input, checks them, rebuilds the valuation on a copy and
# shows what it changes, then (after you confirm, or with --yes) reprices, starts the game with the valuation rebuilt
# and checks that values and prices agree. Any failed step stops the script with the game still stopped; the last
# lines say how to go back. Never during the morning run (05:40-07:30: the 06:00 collection and the 06:30 close).
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:-}"
shift || true
SKIP_COLLECT=0
YES=0
for a in "$@"; do
  case "$a" in
    --skip-collect) SKIP_COLLECT=1 ;;
    --yes) YES=1 ;;
    *) echo "不认识的参数：$a" >&2; exit 2 ;;
  esac
done
if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "用法：bash deploy/revalue.sh vX.Y.Z [--skip-collect] [--yes]" >&2
  exit 2
fi

say() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
ok() { printf '\033[1;32m%s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m!! %s\033[0m\n' "$*"; }
die() { printf '\033[1;31m!! %s\033[0m\n' "$*" >&2; exit 1; }

# .env (the collector's tokens, proxies), as systemd reads it
if [ -f .env ]; then
  set -a
  # shellcheck disable=SC1091
  source <(grep -E '^[A-Za-z_][A-Za-z0-9_]*=' .env)
  set +a
fi
UV="$(command -v uv)" || die "找不到 uv"
STAMP="$(date +%Y%m%d-%H%M%S)"
VALUES="data/values-$STAMP.db"
BACKUP=""
PREV="$(cat bin/anistock.version 2>/dev/null || echo '?')"

rollback_help() {
  local code=$?
  [ "$code" -eq 0 ] && return
  systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD 2>/dev/null || true
  rm -f "$VALUES" "$VALUES-wal" "$VALUES-shm"
  printf '\n\033[1;31m!! 没有完成（退出码 %s）。游戏服务现在：%s\033[0m\n' "$code" "$(systemctl --user is-active anistock-server 2>/dev/null || true)"
  if [ -n "$BACKUP" ]; then
    cat <<EOF
回到上线前：
  systemctl --user stop anistock-server
  cp $BACKUP data/game.db && rm -f data/game.db-wal data/game.db-shm
  ANISTOCK_VERSION=$PREV ANISTOCK_NO_RESTART=1 bash deploy/update.sh
  uv run engine/daily_inputs.py --from 2026-07-01 --day \$(date -d yesterday +%F) --force
  systemctl --user start anistock-server
（问题解决后也可以直接重新运行本脚本，加 --skip-collect 跳过采集。）
EOF
  fi
}
trap rollback_help EXIT

# ------------------------------------------------------------------------------------------------ 0. preflight
say "检查"
now="$(date +%H%M)"
if [ "$now" -ge 0540 ] && [ "$now" -le 0730 ]; then die "现在是 05:40–07:30（06:00 采集、06:30 日结），换个时间"; fi
for f in data/game.db data/derived/heat_backfill.db data/derived/platform_us.json; do
  [ -f "$f" ] || die "缺少 $f"
done
command -v sqlite3 >/dev/null || die "找不到 sqlite3"
ok "当前版本 $PREV，目标 $VERSION"

# ------------------------------------------------------------------------------------------------ 1. stop, back up
say "停服，备份数据库"
systemctl --user stop anistock-server
systemctl --user is-active --quiet anistock-server && die "游戏服务没有停下来"
mkdir -p data/backups
BACKUP="data/backups/game-before-$VERSION-$STAMP.db"
sqlite3 data/game.db ".backup '$BACKUP'"
ok "备份：$BACKUP"

# ------------------------------------------------------------------------------------------------ 2. install
say "安装 $VERSION（代码切到该版本，不重启）"
ANISTOCK_VERSION="$VERSION" ANISTOCK_NO_RESTART=1 bash deploy/update.sh
[ "$(cat bin/anistock.version)" = "$VERSION" ] || die "bin/anistock 不是 $VERSION"
[ "$(git describe --tags --exact-match 2>/dev/null)" = "$VERSION" ] || die "代码不在 $VERSION"

# ------------------------------------------------------------------------------------------------ 3. data
if [ "$SKIP_COLLECT" -eq 0 ]; then
  say "采集一次（十几分钟；第一次采到新作品时更久）"
  "$UV" run collect.py
fi
say "重建中国、日本序列和每日输入"
"$UV" run sim/reconstruct.py build
YESTERDAY="$(date -d yesterday +%F)"
"$UV" run engine/daily_inputs.py --from 2026-07-01 --day "$YESTERDAY" --force > /dev/null
python3 - "$YESTERDAY" <<'EOF'
import datetime, json, sqlite3, sys
last = datetime.date.fromisoformat(sys.argv[1])
c = sqlite3.connect("file:data/game.db?mode=ro", uri=True)
ids = {r[0] for r in c.execute("SELECT id FROM companies WHERE status='listed'")} | {r[0] for r in c.execute("SELECT work_id FROM company_works")}
print("日期        作品  欧美       中国     日本   （已上市公司的作品，热度合计）")
for k in range(7, -1, -1):
    d = (last - datetime.timedelta(days=k)).isoformat()
    try:
        x = json.load(open(f"data/derived/inputs/{d}.json"))
    except FileNotFoundError:
        sys.exit(f"缺少 {d} 的输入")
    s = [v for v in x["shows"] if v["id"] in ids]
    t = {m: sum(v["att"][m] for v in s) for m in ("us", "cn", "jp")}
    print(f"{d}  {len(s):4d}  {t['us']:9.0f}  {t['cn']:7.0f}  {t['jp']:6.0f}")
s = [v for v in x["shows"] if v["id"] in ids]
r = {m: sum(v["att"][m] for v in s) / max(1e-9, sum(v["expected"][m] for v in s)) for m in ("us", "cn", "jp")}
print("最后一天 热度/预期", {m: round(v, 2) for m, v in r.items()}, " 平台", {k: round(v) for k, v in x["platform"].items()},
      " platform_days", x["platform_days"])
zero = sum(1 for v in s if v["att"]["us"] == 0)
if min(r.values()) < 0.9 or zero > len(s) // 10:
    sys.exit("最后一天的数据不完整（热度/预期低于 0.9，或很多作品欧美为 0）：等采集器跑完再来")
EOF
ok "输入完整"

# ------------------------------------------------------------------------------------------------ 4. rebuild on a copy
say "在副本上重建估值"
sqlite3 data/game.db ".backup '$VALUES'"
LOG="$(mktemp)"
ANISTOCK_GAME_DB="$PWD/$VALUES" ANISTOCK_PORT=18699 ANISTOCK_NO_LOOPS=1 ANISTOCK_VALUATION_REBUILD=1 bin/anistock > "$LOG" 2>&1 &
PID=$!
for _ in $(seq 1 600); do
  grep -q "listening on" "$LOG" && break
  kill -0 "$PID" 2>/dev/null || break
  sleep 1
done
kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
grep -q "valuation: started the filter" "$LOG" || { cat "$LOG"; die "副本上没有重建估值"; }
grep "valuation" "$LOG"
python3 - "$VALUES" <<'EOF'
import sqlite3, statistics as s, sys
a = sqlite3.connect("file:data/game.db?mode=ro", uri=True)
b = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
q = "SELECT id, fair_value, last_price FROM companies WHERE status='listed' AND fair_value > 0"
old = {i: (f, p) for i, f, p in a.execute(q)}
new = {i: f for i, f, _ in b.execute(q)}
t = dict(b.execute("SELECT id, title FROM companies"))
rule = b.execute("SELECT value FROM state WHERE key='platform_rule'").fetchone()
print("换算系数（平台规模 欧美 中国 日本，系数 欧美 中国 日本）：", rule[0] if rule else "无（仍是相对规则）")
def dist(name, r):
    r = sorted(r)
    print(f"{name}：{len(r)} 只  中位 {s.median(r):.3f}  P10 {r[len(r) // 10]:.3f}  P90 {r[len(r) * 9 // 10]:.3f}  "
          f"最小 {r[0]:.3f}  最大 {r[-1]:.3f}  超过 1.5 倍 {sum(1 for x in r if x > 1.5 or x < 1 / 1.5)} 只")
ids = [i for i in old if i in new]
dist("新估值/旧估值", [new[i] / old[i][0] for i in ids])
dist("新估值/现价  ", [new[i] / max(1, old[i][1]) for i in ids])
ranked = sorted(ids, key=lambda i: new[i] / old[i][0])
print("下调最多：" + "，".join(f"{t[i][:14]} {new[i] / old[i][0]:.2f}" for i in ranked[:8]))
print("上调最多：" + "，".join(f"{t[i][:14]} {new[i] / old[i][0]:.2f}" for i in ranked[::-1][:8]))
EOF

if [ "$YES" -eq 0 ]; then
  printf '\n对照上线文档里这一版的预期数字。继续（重新定价并启动）？[y/N] '
  read -r answer
  [[ "$answer" =~ ^[Yy]$ ]] || die "已取消：数据库没有改动，游戏服务停着"
fi

# ------------------------------------------------------------------------------------------------ 5. reprice, start
say "重新定价"
RULE="$(sqlite3 "$VALUES" "SELECT value FROM state WHERE key='platform_rule'")"
bin/anistock reprice "$VALUES"
rm -f "$VALUES" "$VALUES-wal" "$VALUES-shm"

say "启动（正式数据库上重建估值）"
SINCE="$(date '+%Y-%m-%d %H:%M:%S')"
systemctl --user set-environment ANISTOCK_VALUATION_REBUILD=1
systemctl --user start anistock-server
for _ in $(seq 1 600); do
  journalctl --user -u anistock-server --since "$SINCE" --no-pager 2>/dev/null | grep -q "listening on" && break
  systemctl --user is-active --quiet anistock-server || break
  sleep 1
done
systemctl --user unset-environment ANISTOCK_VALUATION_REBUILD
journalctl --user -u anistock-server --since "$SINCE" --no-pager | grep -q "valuation: started the filter" ||
  { journalctl --user -u anistock-server --since "$SINCE" --no-pager | tail -30; die "正式数据库没有重建估值"; }
systemctl --user is-active --quiet anistock-server || die "游戏服务没有起来"

# ------------------------------------------------------------------------------------------------ 6. verify
say "核对"
LIVE_RULE="$(sqlite3 "file:data/game.db?mode=ro" "SELECT value FROM state WHERE key='platform_rule'")"
[ "$LIVE_RULE" = "$RULE" ] || warn "正式数据库的换算系数和副本不同：$LIVE_RULE"
python3 - <<'EOF'
import sqlite3, statistics as s, sys
c = sqlite3.connect("file:data/game.db?mode=ro", uri=True)
r = sorted(f / p for f, p in c.execute("SELECT fair_value, last_price FROM companies WHERE status='listed' AND fair_value > 0 AND last_price > 0"))
print(f"估值/现价：{len(r)} 只  中位 {s.median(r):.3f}  最小 {r[0]:.3f}  最大 {r[-1]:.3f}")
ra = c.execute("SELECT value FROM state WHERE key='repriced_after'").fetchone()
print("repriced_after", ra[0] if ra else "（没有）")
if not 0.97 <= s.median(r) <= 1.03:
    sys.exit("估值和价格没有对齐")
EOF
trap - EXIT
ok "完成：$VERSION 已上线，备份在 $BACKUP"
echo "发公告（看过网页没问题后）：bin/anistock announce \"标题\" docs/ops/deploy-$VERSION-announcement.md"
