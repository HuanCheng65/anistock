#!/usr/bin/env bash
# First-time install of the AniStock game server on the box that already runs the collector.
# Safe to re-run: every step checks before it acts. Run from the repo root:  bash deploy/install.sh
#   ANISTOCK_VERSION=vX.Y.Z  install that release of the game server (default: the checked-out tag, else latest)
set -euo pipefail
cd "$(dirname "$0")/.."
source deploy/binary.sh

# Port the game server listens on: ANISTOCK_PORT in .env, else 18620 (same default as the systemd unit)
game_port() { local p; p="$(grep -E '^ANISTOCK_PORT=[0-9]+' .env 2>/dev/null | tail -1 | cut -d= -f2)"; echo "${p:-18620}"; }
# Ours: /api/internal/health (v0.4.0+) has "engine_lag_ms"; a release before v0.4.0 still answers /api/status
# with "launched_on" (the running server during an update). Anything else on the port is another program.
is_ours() {
  local u="http://127.0.0.1:$(game_port)"
  curl -s --max-time 3 --noproxy '*' "$u/api/internal/health" | grep -q '"engine_lag_ms"' ||
    curl -s --max-time 3 --noproxy '*' "$u/api/status" | grep -q '"launched_on"'
}
wait_ours() { for _ in $(seq 1 40); do is_ours && return 0; sleep 1; done; return 1; }
port_taken_by_other() {
  curl -s -o /dev/null --max-time 3 --noproxy '*' "http://127.0.0.1:$(game_port)/" && ! is_ours
}

# systemd keeps everything after "=", trailing "# comment" included: flag such lines in .env
check_env() {
  local bad
  bad="$(grep -nE '^[A-Za-z_][A-Za-z0-9_]*=.*[[:space:]]#' .env 2>/dev/null || true)"
  if [ -n "$bad" ]; then
    printf '\033[1;33m!! .env 里这些行带了行尾注释，systemd 会把注释当成值的一部分（比如仓库地址、代理地址会失效）：\033[0m\n%s\n' "$bad"
    printf '\033[1;33m!! 请把注释挪到单独一行再运行\033[0m\n'
    return 1
  fi
}

# Build the frontend with local Node.js 20+, or else inside a throwaway Node container.
# Packages come through pnpm (web/pnpm-lock.yaml). Override the image with NODE_IMAGE=..., the registry with NPM_REGISTRY=... (e.g. https://registry.npmmirror.com),
# and the docker command with DOCKER="sudo docker" if your user is not in the docker group.
build_web() {
  if command -v node >/dev/null && [ "$(node -p 'process.versions.node.split(".")[0]')" -ge 20 ]; then
    # pnpm if installed, else the one Node ships through corepack (Node 16.9–24), else a one-off download
    local pnpm="pnpm"
    if ! command -v pnpm >/dev/null; then
      if command -v corepack >/dev/null; then pnpm="corepack pnpm"; else pnpm="npx --yes pnpm@10.12.4"; fi
    fi
    ( cd web && COREPACK_ENABLE_DOWNLOAD_PROMPT=0 $pnpm install --frozen-lockfile && $pnpm run build )
  elif command -v docker >/dev/null; then
    local image="${NODE_IMAGE:-node:24-alpine}"
    echo "本机没有 Node.js 20+，用容器 $image 构建前端"
    if ! ${DOCKER:-docker} info >/dev/null 2>&1; then
      echo "当前用户不能访问 Docker。用 DOCKER=\"sudo docker\" 重新运行本脚本，或者把自己加入 docker 组" >&2
      return 1
    fi
    mkdir -p "$HOME/.cache/anistock-npm"
    ${DOCKER:-docker} run --rm --network host -u "$(id -u):$(id -g)" \
      -e HOME=/tmp -e npm_config_cache=/tmp/.npm -e npm_config_store_dir=/tmp/.npm/pnpm-store \
      -e COREPACK_HOME=/tmp/.npm/corepack -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 \
      ${NPM_REGISTRY:+-e npm_config_registry="$NPM_REGISTRY"} \
      ${https_proxy:+-e https_proxy="$https_proxy"} ${HTTPS_PROXY:+-e HTTPS_PROXY="$HTTPS_PROXY"} \
      -v "$PWD:/repo" -v "$HOME/.cache/anistock-npm:/tmp/.npm" -w /repo/web \
      "$image" sh -c "corepack pnpm install --frozen-lockfile && corepack pnpm run build"
  else
    echo "既没有 Node.js 20+，也没有 Docker：装其中一个再运行" >&2
    return 1
  fi
}
ROOT="$PWD"
say()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m!! %s\033[0m\n' "$*"; }
die()  { printf '\033[1;31mxx %s\033[0m\n' "$*"; exit 1; }

say "检查依赖"
UV="$(command -v uv || true)"
[ -n "$UV" ] || die "没找到 uv"
if command -v node >/dev/null && [ "$(node -p 'process.versions.node.split(".")[0]')" -ge 20 ]; then
  echo "uv: $UV   node: $(node -v)"
elif command -v docker >/dev/null; then
  echo "uv: $UV   node: 本机没有，用 Docker 容器构建前端"
else
  die "构建前端需要 Node.js 20+ 或 Docker，两个都没找到"
fi

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
check_env || exit 1

say "安装 Python 依赖"
"$UV" sync

say "构建前端"
build_web

say "重建市场序列"
"$UV" run sim/reconstruct.py airtimes || warn "刷新 Annict 播出时间失败，继续用已有数据"
"$UV" run sim/reconstruct.py build

say "开服"
if [ -f data/game.db ]; then
  echo "data/game.db 已存在，跳过（要重新开服请运行 uv run python -m server.seed --force，会删除所有玩家）"
else
  "$UV" run python -m server.seed
fi

say "下载游戏服务程序"
install_binary || die "游戏服务程序下载失败（需要代理时先 export https_proxy=...）"
if installed_unit_is_v1; then
  say "从 v1 切换到 v2：停服并备份数据库"
  systemctl --user stop anistock-server || true
  backup_game_db before-v2
fi

say "检查端口"
if port_taken_by_other; then
  die "端口 $(game_port) 已经被别的程序占用：在 .env 里设置 ANISTOCK_PORT=其他端口 后重新运行"
fi
echo "使用端口 $(game_port)"

say "安装 systemd 服务"
mkdir -p ~/.config/systemd/user
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-server.service > ~/.config/systemd/user/anistock-server.service
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-collect.service > ~/.config/systemd/user/anistock-collect.service
cp deploy/anistock-collect.timer ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now anistock-collect.timer
# v0.3.0+: the game server fetches the hourly comment snapshots itself; retire the earlier record-only timer
systemctl --user disable --now anistock-flash.timer 2>/dev/null || true
rm -f ~/.config/systemd/user/anistock-flash.service ~/.config/systemd/user/anistock-flash.timer
systemctl --user enable anistock-server
systemctl --user restart anistock-server
loginctl enable-linger "$USER" 2>/dev/null || warn "enable-linger 失败：退出 SSH 后服务可能停止，请让管理员执行 sudo loginctl enable-linger $USER"

say "等待服务启动"
if wait_ours; then
  printf '\n\033[1;32m上线完成。\033[0m\n'
  echo "游戏服务只监听本机：http://127.0.0.1:$(game_port)，请用反向代理（Caddy / Nginx）绑定域名对外提供，配置示例见 README"
  echo "邀请码：$(grep '^ANISTOCK_INVITE=' .env | cut -d= -f2)"
  echo "日志：journalctl --user -u anistock-server -f"
else
  journalctl --user -u anistock-server -n 40 --no-pager || true
  die "服务没起来（端口 $(game_port)），日志在上面"
fi
