# Shared by install.sh and update.sh (sourced from the repo root): the v2 game server binary.
#
# The server has no Rust toolchain: GitHub Actions builds a static binary for every v* tag and attaches it
# to that GitHub Release (.github/workflows/backend.yml). This downloads it to bin/anistock.
#
#   ANISTOCK_VERSION=v2.0.0   which release (default: the tag checked out, else the latest release)
#   ANISTOCK_REPO=owner/name  where the releases are (default: from `git remote get-url origin`)
#   ANISTOCK_SERVER=v1        keep running the v1 Python server instead (rollback; nothing is downloaded)
#   https_proxy               used for the downloads when set (curl reads it)

BIN_ASSET="anistock-x86_64-linux-musl"

server_kind() { if [ "${ANISTOCK_SERVER:-v2}" = "v1" ]; then echo v1; else echo v2; fi; }

# the systemd unit for the chosen server
server_unit_file() { if [ "$(server_kind)" = "v1" ]; then echo deploy/anistock-server-v1.service; else echo deploy/anistock-server.service; fi; }

release_repo() {
  if [ -n "${ANISTOCK_REPO:-}" ]; then echo "$ANISTOCK_REPO"; return; fi
  git remote get-url origin 2>/dev/null | sed -E 's#^(git@github\.com:|https?://github\.com/)##; s#\.git$##'
}

# curl, through $https_proxy when it is set
fetch() { curl -fsSL --retry 3 --connect-timeout 20 ${https_proxy:+--proxy "$https_proxy"} "$@"; }

release_version() {
  if [ -n "${ANISTOCK_VERSION:-}" ]; then echo "$ANISTOCK_VERSION"; return; fi
  local tag
  if tag="$(git describe --tags --exact-match 2>/dev/null)" && [[ "$tag" == v* ]]; then echo "$tag"; return; fi
  local json
  json="$(fetch "https://api.github.com/repos/$(release_repo)/releases/latest" 2>/dev/null)" || return 1
  python3 -c 'import json,sys; print(json.loads(sys.argv[1])["tag_name"])' "$json"
}

# Download the release binary, check its sha256, install it to bin/anistock atomically.
install_binary() {
  local version repo url tmp
  version="$(release_version)" || { echo "查不到最新的发布版本（GitHub 需要代理时先 export https_proxy=...，或者用 ANISTOCK_VERSION=vX.Y.Z 指定）" >&2; return 1; }
  [ -n "$version" ] || { echo "没有可用的发布版本" >&2; return 1; }
  if [ -x bin/anistock ] && [ "$(cat bin/anistock.version 2>/dev/null)" = "$version" ]; then
    echo "游戏服务程序已是 $version"
    return 0
  fi
  repo="$(release_repo)"
  url="https://github.com/$repo/releases/download/$version"
  echo "下载游戏服务程序 $version（$repo）"
  mkdir -p bin
  tmp="$(mktemp -d bin/.download.XXXXXX)"
  if ! fetch -o "$tmp/$BIN_ASSET" "$url/$BIN_ASSET" || ! fetch -o "$tmp/$BIN_ASSET.sha256" "$url/$BIN_ASSET.sha256"; then
    rm -rf "$tmp"; echo "下载失败：$url/$BIN_ASSET" >&2; return 1
  fi
  if ! (cd "$tmp" && sha256sum -c "$BIN_ASSET.sha256" >/dev/null); then
    rm -rf "$tmp"; echo "校验失败（sha256 不符），没有安装" >&2; return 1
  fi
  chmod 755 "$tmp/$BIN_ASSET"
  mv -f "$tmp/$BIN_ASSET" bin/anistock          # same directory: an atomic rename
  echo "$version" > bin/anistock.version
  rm -rf "$tmp"
  echo "已安装 bin/anistock $version"
}

# Is the installed systemd unit the v1 Python server?
installed_unit_is_v1() { grep -q uvicorn ~/.config/systemd/user/anistock-server.service 2>/dev/null; }

# Consistent copy of the game database (SQLite backup API; safe while it is in use).
backup_game_db() {
  local tag="$1" dest
  [ -f data/game.db ] || return 0
  mkdir -p data/backups
  dest="data/backups/game-$tag-$(date +%Y%m%d-%H%M%S).db"
  python3 - "$dest" <<'EOF'
import sqlite3, sys
src = sqlite3.connect("file:data/game.db?mode=ro", uri=True)
out = sqlite3.connect(sys.argv[1])
with out:
    src.backup(out)
out.close(); src.close()
EOF
  echo "已备份游戏数据库到 $dest"
}
