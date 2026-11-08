#!/usr/bin/env bash
# Pull the latest code and restart. Database migrations run automatically at startup
# (with a backup in data/backups/ first). Run from the repo root:  bash deploy/update.sh
#   ANISTOCK_VERSION=vX.Y.Z  install that release of the game server (default: the checked-out tag, else latest)
# Rolling back means restoring the automatic pre-migration backup and the previous release
# (docs/api-v1.md §7.5); the v1 Python server cannot run on this database any more.
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
# Override the image with NODE_IMAGE=..., the npm registry with NPM_REGISTRY=... (e.g. https://registry.npmmirror.com),
# and the docker command with DOCKER="sudo docker" if your user is not in the docker group.
build_web() {
  if command -v node >/dev/null && [ "$(node -p 'process.versions.node.split(".")[0]')" -ge 20 ]; then
    ( cd web && npm ci && npm run build )
  elif command -v docker >/dev/null; then
    local image="${NODE_IMAGE:-node:24-alpine}"
    echo "本机没有 Node.js 20+，用容器 $image 构建前端"
    if ! ${DOCKER:-docker} info >/dev/null 2>&1; then
      echo "当前用户不能访问 Docker。用 DOCKER=\"sudo docker\" 重新运行本脚本，或者把自己加入 docker 组" >&2
      return 1
    fi
    mkdir -p "$HOME/.cache/anistock-npm"
    ${DOCKER:-docker} run --rm --network host -u "$(id -u):$(id -g)" \
      -e HOME=/tmp -e npm_config_cache=/tmp/.npm \
      ${NPM_REGISTRY:+-e npm_config_registry="$NPM_REGISTRY"} \
      ${https_proxy:+-e https_proxy="$https_proxy"} ${HTTPS_PROXY:+-e HTTPS_PROXY="$HTTPS_PROXY"} \
      -v "$PWD:/repo" -v "$HOME/.cache/anistock-npm:/tmp/.npm" -w /repo/web \
      "$image" sh -c "npm ci && npm run build"
  else
    echo "既没有 Node.js 20+，也没有 Docker：装其中一个再运行" >&2
    return 1
  fi
}
say() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
UV="$(command -v uv)"

# the binary first: a failed download (GitHub answers 504 now and then) leaves everything as it was, instead of new
# code and frontend against the old server
say "下载游戏服务程序"
install_binary stage

say "拉取代码"
git pull --ff-only
check_env || exit 1

say "更新依赖、构建前端"
"$UV" sync
build_web

if installed_unit_is_v1; then
  say "第一次从 v1 切换到 v2：停服并备份数据库"
  systemctl --user stop anistock-server || true
  backup_game_db before-v2
fi

say "更新 systemd 服务定义"
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-server.service > ~/.config/systemd/user/anistock-server.service
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-collect.service > ~/.config/systemd/user/anistock-collect.service
cp deploy/anistock-collect.timer ~/.config/systemd/user/
systemctl --user daemon-reload
# v0.3.0+: the game server fetches the hourly comment snapshots itself; retire the earlier record-only timer
systemctl --user disable --now anistock-flash.timer 2>/dev/null || true
rm -f ~/.config/systemd/user/anistock-flash.service ~/.config/systemd/user/anistock-flash.timer

say "重启游戏服务"
if port_taken_by_other; then
  echo "端口 $(game_port) 被别的程序占用：在 .env 里设置 ANISTOCK_PORT=其他端口" >&2; exit 1
fi
activate_binary
systemctl --user restart anistock-server
if wait_ours; then
  printf '\033[1;32m更新完成（%s，端口 %s）\033[0m\n' "$(cat bin/anistock.version 2>/dev/null)" "$(game_port)"
else
  journalctl --user -u anistock-server -n 40 --no-pager; exit 1
fi
