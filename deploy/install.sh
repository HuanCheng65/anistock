#!/usr/bin/env bash
# First-time install of the AniStock game server on the box that already runs the collector.
# Safe to re-run: every step checks before it acts. Run from the repo root:  bash deploy/install.sh
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
say()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m!! %s\033[0m\n' "$*"; }
die()  { printf '\033[1;31mxx %s\033[0m\n' "$*"; exit 1; }

say "检查依赖"
UV="$(command -v uv || true)"
[ -n "$UV" ] || die "没找到 uv"
command -v node >/dev/null || die "没找到 Node.js（需要 20 以上）。安装方法见 README『游戏网站』一节"
NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]')"
[ "$NODE_MAJOR" -ge 20 ] || die "Node.js 版本是 $(node -v)，需要 20 以上"
echo "uv: $UV   node: $(node -v)"

say "检查采集数据库"
[ -f data/anistock.db ] || die "没有 data/anistock.db：采集器还没跑过，或者需要从开发机拷贝过来"
count() { python3 -c "import sqlite3,sys;print(sqlite3.connect('data/anistock.db').execute(\"SELECT COUNT(*) FROM works WHERE source=?\",(sys.argv[1],)).fetchone()[0])" "$1"; }
SUMMER="$(count anilist)"
BGM="$(count bgm)"
echo "AniList 作品 $SUMMER 部，Bangumi 作品 $BGM 部"
if [ "$SUMMER" -lt 250 ] || [ "$BGM" -lt 150 ]; then
  warn "采集数据库看起来是旧的（缺夏季番回填或补充的 Bangumi 条目）。"
  warn "请先从开发机拷贝最新的 data/anistock.db，或者运行：uv run collect.py --season 2026-summer && uv run collect.py"
  read -r -p "仍然继续？[y/N] " ans; [ "${ans:-N}" = "y" ] || exit 1
fi

say "配置 .env"
[ -f .env ] || { cp .env.example .env; chmod 600 .env; echo "已从 .env.example 创建 .env"; }
grep -q '^ANISTOCK_SECRET=' .env || echo 'ANISTOCK_SECRET=' >> .env
grep -q '^ANISTOCK_INVITE=' .env || echo 'ANISTOCK_INVITE=' >> .env
if grep -q '^ANISTOCK_SECRET=$' .env; then
  SECRET="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
  sed -i "s/^ANISTOCK_SECRET=$/ANISTOCK_SECRET=$SECRET/" .env
  echo "已生成 ANISTOCK_SECRET"
fi
if grep -q '^ANISTOCK_INVITE=$' .env; then
  INVITE="$(python3 -c 'import secrets,string; print("".join(secrets.choice(string.ascii_lowercase+string.digits) for _ in range(8)))')"
  sed -i "s/^ANISTOCK_INVITE=$/ANISTOCK_INVITE=$INVITE/" .env
  echo "已生成邀请码：$INVITE（可在 .env 里改）"
fi
grep -q '^ANNICT_TOKEN=.\+' .env || warn ".env 里没有 ANNICT_TOKEN：日本市场的播出时间不会更新"

say "安装 Python 依赖"
"$UV" sync

say "构建前端"
( cd web && npm ci && npm run build )

say "重建市场序列"
"$UV" run sim/reconstruct.py airtimes || warn "刷新 Annict 播出时间失败，继续用已有数据"
"$UV" run sim/reconstruct.py build

say "开服"
if [ -f data/game.db ]; then
  echo "data/game.db 已存在，跳过（要重新开服请运行 uv run python -m server.seed --force，会删除所有玩家）"
else
  "$UV" run python -m server.seed
fi

say "安装 systemd 服务"
mkdir -p ~/.config/systemd/user
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-server.service > ~/.config/systemd/user/anistock-server.service
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-collect.service > ~/.config/systemd/user/anistock-collect.service
cp deploy/anistock-collect.timer ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now anistock-collect.timer
systemctl --user enable anistock-server
systemctl --user restart anistock-server
loginctl enable-linger "$USER" 2>/dev/null || warn "enable-linger 失败：退出 SSH 后服务可能停止，请让管理员执行 sudo loginctl enable-linger $USER"

say "等待服务启动"
for _ in $(seq 1 30); do
  curl -s -o /dev/null --noproxy '*' http://127.0.0.1:8080/api/status && break
  sleep 1
done
if curl -s --noproxy '*' http://127.0.0.1:8080/api/status >/dev/null; then
  IP="$(curl -s --max-time 5 --noproxy '*' https://ipinfo.io/ip 2>/dev/null || hostname -I | awk '{print $1}')"
  printf '\n\033[1;32m上线完成。\033[0m\n'
  echo "访问：http://$IP:8080   （云服务器防火墙/安全组要放行 8080 端口）"
  echo "邀请码：$(grep '^ANISTOCK_INVITE=' .env | cut -d= -f2)"
  echo "日志：journalctl --user -u anistock-server -f"
else
  journalctl --user -u anistock-server -n 40 --no-pager || true
  die "服务没起来，日志在上面"
fi
