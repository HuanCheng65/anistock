#!/usr/bin/env bash
# Pull the latest code and restart. Database migrations run automatically at startup
# (with a backup in data/backups/ first). Run from the repo root:  bash deploy/update.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# Build the frontend with local Node.js 20+, or else inside a throwaway Node container.
# Override the image with NODE_IMAGE=..., the npm registry with NPM_REGISTRY=... (e.g. https://registry.npmmirror.com),
# and the docker command with DOCKER="sudo docker" if your user is not in the docker group.
build_web() {
  if command -v node >/dev/null && [ "$(node -p 'process.versions.node.split(".")[0]')" -ge 20 ]; then
    ( cd web && npm ci && npm run build )
  elif command -v docker >/dev/null; then
    local image="${NODE_IMAGE:-node:22-alpine}"
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
      -v "$PWD/web:/app" -v "$HOME/.cache/anistock-npm:/tmp/.npm" -w /app \
      "$image" sh -c "npm ci && npm run build"
  else
    echo "既没有 Node.js 20+，也没有 Docker：装其中一个再运行" >&2
    return 1
  fi
}
say() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
UV="$(command -v uv)"

say "拉取代码"
git pull --ff-only

say "更新依赖、构建前端"
"$UV" sync
build_web

say "更新 systemd 服务定义"
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-server.service > ~/.config/systemd/user/anistock-server.service
sed "s#%h/.local/bin/uv#$UV#" deploy/anistock-collect.service > ~/.config/systemd/user/anistock-collect.service
cp deploy/anistock-collect.timer ~/.config/systemd/user/
systemctl --user daemon-reload

say "重启游戏服务"
systemctl --user restart anistock-server
for _ in $(seq 1 30); do
  curl -s -o /dev/null --noproxy '*' http://127.0.0.1:8080/api/status && break
  sleep 1
done
curl -s --noproxy '*' http://127.0.0.1:8080/api/status >/dev/null \
  && printf '\033[1;32m更新完成\033[0m\n' \
  || { journalctl --user -u anistock-server -n 40 --no-pager; exit 1; }
