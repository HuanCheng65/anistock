#!/usr/bin/env bash
# Run the v2 core benchmark: each implementation x each load multiplier, server pinned to SERVER_CPUS.
#   bash bench/run.sh                        # all of python go rust, multipliers 1 3 5 10
#   IMPLS="go rust" MULTS="1 10" bash bench/run.sh
# Base load (1x): 200 player orders/s, 300 NPC orders/s (+ market makers), 500 WebSocket clients.
# Results: one JSON line per run in bench/out/results.jsonl
set -euo pipefail
trap 'kill "${SPID:-}" 2>/dev/null || true' EXIT
cd "$(dirname "$0")"
BENCH="$PWD"

IMPLS="${IMPLS:-python go rust}"
MULTS="${MULTS:-1 3 5 10}"
DUR="${DUR:-30s}"
SERVER_CPUS="${SERVER_CPUS:-0,1}"      # the server gets 2 cores
GEN_CPUS="${GEN_CPUS:-8-31}"           # the load generator gets its own cores (on a 2-core box: leave empty)
PORT="${BENCH_PORT:-18700}"
OUT="$BENCH/out"
mkdir -p "$OUT"
ulimit -n 65536 2>/dev/null || ulimit -n "$(ulimit -Hn)"

pin() { local cpus="$1"; shift; if [ -n "$cpus" ] && command -v taskset >/dev/null; then taskset -c "$cpus" "$@"; else "$@"; fi; }

build() {
  export HTTPS_PROXY="${https_proxy:-${HTTPS_PROXY:-}}" HTTP_PROXY="${http_proxy:-${HTTP_PROXY:-}}" GOTOOLCHAIN=local
  [ -x loadgen/loadgen ] || (cd loadgen && CGO_ENABLED=0 go build -o loadgen .)
  for impl in $IMPLS; do
    case "$impl" in
      go)   [ -x go/server ] || (cd go && CGO_ENABLED=0 go build -o server .) ;;
      rust) [ -x rust/target/release/anistock-bench ] || (cd rust && cargo build --release -q) ;;
      go-v2)   (cd go-v2 && CGO_ENABLED=0 go build -o server .) ;;
      rust-v2) (cd rust-v2 && cargo build --release -q) ;;
      python)
        [ -x python/.venv/bin/python ] || { uv venv -q --python "${PY_VERSION:-3.13}" python/.venv && uv pip install -q -p python/.venv -r python/requirements.txt; }
        PY="$BENCH/python/.venv/bin/python" ;;
    esac
  done
}

start() {   # start <impl> <npc_rate>; sets SPID
  local impl="$1" npc="$2" log="$OUT/server-$1.log"
  if curl -s -o /dev/null --max-time 1 --noproxy '*' "http://127.0.0.1:$PORT/health"; then
    echo "port $PORT is already in use (a leftover server?)" >&2; return 1
  fi
  local -a cmd
  case "$impl" in
    python) cmd=("$PY" "$BENCH/python/server.py") ;;
    go)     cmd=("$BENCH/go/server") ;;
    rust)   cmd=("$BENCH/rust/target/release/anistock-bench") ;;
    go-v2)   cmd=("$BENCH/go-v2/server") ;;
    rust-v2) cmd=("$BENCH/rust-v2/target/release/anistock-bench") ;;
  esac
  local -a pre=()
  if [ -n "$SERVER_CPUS" ] && command -v taskset >/dev/null; then pre=(taskset -c "$SERVER_CPUS"); fi
  # env and taskset both exec, so $! is the server process itself (needed for CPU / RSS sampling)
  env BENCH_PORT="$PORT" BENCH_NPC_RATE="$npc" BENCH_LOG="$OUT/events-$impl.log" GOMAXPROCS=2 TOKIO_WORKER_THREADS=2 \
    "${pre[@]}" "${cmd[@]}" > "$log" 2>&1 &
  SPID=$!
  for _ in $(seq 1 100); do
    kill -0 "$SPID" 2>/dev/null || break
    curl -s --max-time 1 --noproxy '*' "http://127.0.0.1:$PORT/health" >/dev/null && return 0
    sleep 0.2
  done
  echo "$impl did not start; log:" >&2; tail -20 "$log" >&2; return 1
}

stop() { kill "$SPID" 2>/dev/null || true; wait "$SPID" 2>/dev/null || true; rm -f "$OUT"/events-*.log; }

build
for impl in $IMPLS; do
  for m in $MULTS; do
    start "$impl" $((300 * m))
    printf '%-6s %2sx ... ' "$impl" "$m"
    # hard cap per run so a collapsed server cannot hang the whole benchmark
    line="$(pin "$GEN_CPUS" timeout 180 "${LOADGEN:-loadgen/loadgen}" -url "http://127.0.0.1:$PORT" -rate $((200 * m)) \
            -ws $((500 * m)) -dur "$DUR" -pid "$SPID" -label "$impl ${m}x" || true)"
    stop
    [ -n "$line" ] || line="{\"label\":\"$impl ${m}x\",\"collapsed\":true}"
    echo "$line" >> "$OUT/results.jsonl"
    verdict="$(echo "$line" | python3 -c '
import json,sys; r=json.load(sys.stdin)
ok = not r.get("collapsed") and r["orders_per_s"] >= 0.95*r["target_rate"] and 0 <= r["rtt_p99_ms"] < 1000 and r["order_failed"] == 0
if r.get("collapsed"): print("COLLAPSED (no result within 180 s)")
else: print("orders/s %(orders_per_s)s  p50 %(rtt_p50_ms)sms  p99 %(rtt_p99_ms)sms  ws lag p99 %(ws_lag_p99_ms)sms  cpu %(server_cpu)s  rss %(server_rss_mb)sMB  failed %(order_failed)s  dropped %(ws_dropped)s" % r)
print("OK" if ok else "BROKEN")')"
    echo "$verdict" | head -1
    if [ "$(echo "$verdict" | tail -1)" = BROKEN ]; then echo "$impl collapsed at ${m}x: skipping higher loads"; break; fi
    sleep 2
  done
done
