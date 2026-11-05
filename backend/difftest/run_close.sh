#!/usr/bin/env bash
# Daily-close differential test: the same days of trading (NPC ticks with one shared seeded RNG, player
# orders, IPO subscriptions) and the same closes from the same inputs files, through v1 (server/daily.py,
# server/npc.py, server/exchange.py) and through the Rust engine, on two copies of one game DB; then every
# table (rowid-keyed too, via server/tools/compare_close.py) and every operation's result are compared.
#
#   backend/difftest/run_close.sh BASE.db INPUTS_DIR [SEED...]       default seeds: 1 2 3
#
# BASE.db: a launched game DB (server/seed.py on a collector copy, then prep_close_base.py; see its
# docstring for the full recipe). INPUTS_DIR: <day>.json files from engine/daily_inputs.py for the days
# after last_closed, best passed through make_inputs.py (real collector data alone gives one IPO and no
# premieres/finales; make_inputs.py adds those and a <day>.bad file whose close fails and must roll back).
# Env: LATE_DAY (default: the 3rd day) is closed late; from CATCHUP_FROM (default: the second-to-last
# day) on, days are closed by one catch_up. Each run gets a new directory under $DIFFTEST_DIR (default: a temp dir).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BASE="$(realpath "$1")"; INPUTS="$(realpath "$2")"; shift 2
SEEDS=("$@"); [ ${#SEEDS[@]} -eq 0 ] && SEEDS=(1 2 3)
WORK="${DIFFTEST_DIR:-$(mktemp -d)}"; mkdir -p "$WORK"
PIN=(); command -v taskset >/dev/null && PIN=(taskset -c "${DIFFTEST_CPUS:-64-127}")
mapfile -t DAYS < <(ls "$INPUTS" | sed -n 's/\.json$//p' | sort)
LATE="${LATE_DAY-${DAYS[2]:-}}"; CATCH="${CATCHUP_FROM-${DAYS[-2]:-}}"

( cd "$ROOT/backend" && "${PIN[@]}" cargo build --release --quiet --bin difftest )
BIN="$ROOT/backend/target/release/difftest"
cd "$ROOT"
fail=0
for seed in "${SEEDS[@]}"; do
  d="$(mktemp -d "$WORK/close$seed.XXXX")"
  echo "== seed $seed (late close: ${LATE:-none}, catch_up from: ${CATCH:-none}) in $d"
  python3 backend/difftest/gen_close.py "$BASE" "$INPUTS" "$seed" "$d/ops.jsonl" "$LATE" "$CATCH"
  cp "$BASE" "$d/py.db"; cp "$BASE" "$d/rs.db"
  ( export ANISTOCK_GAME_DB="$d/py.db"
    time uv run python backend/difftest/driver.py "$d/py.db" "$d/ops.jsonl" "$d/py.results" )
  ( time "${PIN[@]}" "$BIN" "$d/rs.db" "$d/ops.jsonl" "$d/rs.results" )
  ok=1
  python3 backend/difftest/compare.py "$d/py.db" "$d/rs.db" "$d/py.results" "$d/rs.results" "$d/ops.jsonl" || ok=0
  uv run python -m server.tools.compare_close "$d/py.db" "$d/rs.db" --skip-tables refinery_schema_history,valuation_state,valuation_joint,player_days,bindings,bind_codes || ok=0
  if [ $ok = 1 ]; then echo "== seed $seed: IDENTICAL"; else echo "== seed $seed: DIFFERENT"; fail=1; fi
done
exit $fail
