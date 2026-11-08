# Shared by install.sh and update.sh (sourced from the repo root): the v2 game server binary.
#
# The server has no Rust toolchain: GitHub Actions builds a static binary for every v* tag and attaches it
# to that GitHub Release (.github/workflows/backend.yml). This downloads it to bin/anistock.
#
#   ANISTOCK_VERSION=v2.0.0   which release (default: the tag checked out, else the latest release)
#   ANISTOCK_REPO=owner/name  where the releases are (default: from `git remote get-url origin`)
#   https_proxy               used for the downloads when set (curl reads it)

BIN_ASSET="anistock-x86_64-linux-musl"

release_repo() {
  if [ -n "${ANISTOCK_REPO:-}" ]; then echo "$ANISTOCK_REPO"; return; fi
  git remote get-url origin 2>/dev/null | sed -E 's#^(git@github\.com:|https?://github\.com/)##; s#\.git$##'
}

# curl, through $https_proxy when it is set
# (GitHub's release downloads answer 504 now and then: retried 6 times, 10 s apart)
fetch() { curl -fsSL --retry 6 --retry-delay 10 --connect-timeout 20 ${https_proxy:+--proxy "$https_proxy"} "$@"; }

release_version() {
  if [ -n "${ANISTOCK_VERSION:-}" ]; then echo "$ANISTOCK_VERSION"; return; fi
  local tag
  if tag="$(git describe --tags --exact-match 2>/dev/null)" && [[ "$tag" == v* ]]; then echo "$tag"; return; fi
  local json
  json="$(fetch "https://api.github.com/repos/$(release_repo)/releases/latest" 2>/dev/null)" || return 1
  python3 -c 'import json,sys; print(json.loads(sys.argv[1])["tag_name"])' "$json"
}

# One asset of release `version` through the GitHub API (asset id from the release, then the asset itself).
fetch_api_asset() {
  local repo="$1" version="$2" name="$3" out="$4" json id
  json="$(fetch "https://api.github.com/repos/$repo/releases/tags/$version")" || return 1
  id="$(python3 -c 'import json,sys; print(next(a["id"] for a in json.loads(sys.argv[1])["assets"] if a["name"] == sys.argv[2]))' "$json" "$name")" || return 1
  fetch -H "Accept: application/octet-stream" -o "$out" "https://api.github.com/repos/$repo/releases/assets/$id"
}

# Download the release binary, check its sha256, install it to bin/anistock atomically. `install_binary stage`
# puts it at bin/anistock.next instead, for `activate_binary` to move into place when the server restarts (update.sh:
# a failure between the download and the restart leaves the running install as it was).
install_binary() {
  local version repo url tmp dest="bin/anistock"
  [ "${1:-}" = stage ] && dest="bin/anistock.next"
  version="$(release_version)" || { echo "查不到最新的发布版本（GitHub 需要代理时先 export https_proxy=...，或者用 ANISTOCK_VERSION=vX.Y.Z 指定）" >&2; return 1; }
  [ -n "$version" ] || { echo "没有可用的发布版本" >&2; return 1; }
  rm -f bin/anistock.next bin/anistock.next.version
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
    # the release download links answer 504 for a while after a release (seen on v0.7.10); the API serves the
    # same files
    echo "直接下载失败，改用 GitHub API 下载" >&2
    if ! fetch_api_asset "$repo" "$version" "$BIN_ASSET" "$tmp/$BIN_ASSET" ||
       ! fetch_api_asset "$repo" "$version" "$BIN_ASSET.sha256" "$tmp/$BIN_ASSET.sha256"; then
      rm -rf "$tmp"; echo "下载失败：$url/$BIN_ASSET" >&2; return 1
    fi
  fi
  if ! (cd "$tmp" && sha256sum -c "$BIN_ASSET.sha256" >/dev/null); then
    rm -rf "$tmp"; echo "校验失败（sha256 不符），没有安装" >&2; return 1
  fi
  chmod 755 "$tmp/$BIN_ASSET"
  mv -f "$tmp/$BIN_ASSET" "$dest"          # same directory: an atomic rename
  echo "$version" > "$dest.version"
  rm -rf "$tmp"
  echo "已下载 $dest $version"
}

# Move a staged binary (`install_binary stage`) into place.
activate_binary() {
  [ -f bin/anistock.next ] || return 0
  mv -f bin/anistock.next bin/anistock
  mv -f bin/anistock.next.version bin/anistock.version
  echo "已安装 bin/anistock $(cat bin/anistock.version)"
}

# Is the installed systemd unit still the v1 Python server (an install from before v0.2.0)? It is replaced
# by the Rust server once; there is no way back to it.
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
