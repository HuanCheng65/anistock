#!/usr/bin/env bash
# Pull the latest code and restart. Database migrations run automatically at startup
# (with a backup in data/backups/ first). Run from the repo root:  bash deploy/update.sh
set -euo pipefail
cd "$(dirname "$0")/.."
say() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
UV="$(command -v uv)"

say "拉取代码"
git pull --ff-only

say "更新依赖、构建前端"
"$UV" sync
( cd web && npm ci && npm run build )

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
