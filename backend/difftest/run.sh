#!/usr/bin/env bash
# Differential test: the same seeded operation sequence through v1 (server/exchange.py) and the Rust
# engine, on two copies of one freshly seeded game.db; then every table and every result is compared.
#
#   backend/difftest/run.sh [N_OPS] [SEED...]      default: 10000 ops, seeds 1-5
#
# Work files go to $DIFFTEST_DIR (default: a temp dir). Needs uv (repo venv) and cargo.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
N="${1:-10000}"; shift || true
SEEDS=("$@"); [ ${#SEEDS[@]} -eq 0 ] && SEEDS=(1 2 3 4 5)
WORK="${DIFFTEST_DIR:-$(mktemp -d)}"; mkdir -p "$WORK"
PIN=(); command -v taskset >/dev/null && PIN=(taskset -c "${DIFFTEST_CPUS:-64-127}")

( cd "$ROOT/backend" && "${PIN[@]}" cargo build --release --quiet --bin difftest )
BIN="$ROOT/backend/target/release/difftest"
cd "$ROOT"
fail=0
for seed in "${SEEDS[@]}"; do
  d="$WORK/seed$seed"; mkdir -p "$d"; rm -f "$d"/*.db "$d"/*.db-wal "$d"/*.db-shm
  echo "== seed $seed, $N ops ($d)"
  uv run python backend/difftest/setup.py "$d/base.db" "$d/meta.json" "$seed"
  python3 backend/difftest/gen.py "$d/meta.json" "$seed" "$N" "$d/ops.jsonl"
  cp "$d/base.db" "$d/py.db"; cp "$d/base.db" "$d/rs.db"
  uv run python backend/difftest/driver.py "$d/py.db" "$d/ops.jsonl" "$d/py.results"
  "${PIN[@]}" "$BIN" "$d/rs.db" "$d/ops.jsonl" "$d/rs.results"
  if python3 backend/difftest/compare.py "$d/py.db" "$d/rs.db" "$d/py.results" "$d/rs.results" "$d/ops.jsonl"; then
    echo "== seed $seed: IDENTICAL"
  else
    echo "== seed $seed: DIFFERENT"; fail=1
  fi
done
exit $fail
