"""Prepare a freshly seeded game DB as the base of the daily-close difftest (writes only the given file).

    ANISTOCK_DB=<copy of the collector DB> ANISTOCK_GAME_DB=<scratch>/base.db uv run python -m server.seed
    uv run python backend/difftest/prep_close_base.py <scratch>/base.db

Adds four well-funded players (so IPOs get oversubscribed and players trade), sets every company one
week short of delisting (finished low earners delist on the first Sunday, the others reset), and writes a
money-stock row 28 days before that Sunday so the central bank makes a rate decision.
Then make the inputs (never against data/: point ANISTOCK_DB at the collector copy):

    for D in <days after last_closed>; do
      ANISTOCK_DB=<collector copy> ANISTOCK_GAME_DB=<scratch>/base.db \\
        uv run engine/daily_inputs.py --day $D --window-start <state window_start> --out <scratch>/real/$D.json
    done
    python3 backend/difftest/make_inputs.py <scratch>/real <scratch>/inputs     # premieres, finales, IPOs, .bad
    backend/difftest/run_close.sh <scratch>/base.db <scratch>/inputs 1 2 3
"""

import sys
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from server import db as gdb  # noqa: E402
from server import exchange as ex  # noqa: E402


def main(path: str) -> None:
    conn = gdb.connect(path)
    last = date.fromisoformat(gdb.get_state(conn, "last_closed"))
    ts = f"{last.isoformat()}T20:00:00+08:00"
    sunday = last + timedelta(days=(6 - last.weekday()) % 7 or 7)
    conn.execute("BEGIN IMMEDIATE")
    world = gdb.special(conn, "world")
    players = []
    for i in range(4):
        pid = conn.execute("INSERT INTO accounts(kind, name, username, created_at) VALUES ('player',?,?,?)",
                           (f"difftest{i}", f"difftest{i}", ts)).lastrowid
        ex.transfer(conn, world, pid, 300_000_000, "grant", day=last.isoformat(), ts=ts)
        players.append(pid)
    conn.execute("UPDATE companies SET low_weeks=7")
    m2 = conn.execute("SELECT SUM(cash) FROM accounts WHERE kind IN "
                      "('player','mm','noise','committee','company')").fetchone()[0]
    conn.execute("INSERT OR REPLACE INTO macro_days(day, rate, m2, m2_growth, injected, exported, taxes, interest) "
                  "VALUES (?, 0.03, ?, NULL, 0, 0, 0, 0)", ((sunday - timedelta(days=28)).isoformat(), int(m2 * 0.9)))
    conn.execute("COMMIT")
    gdb.check_invariants(conn)
    print(f"players {players}; macro row {(sunday - timedelta(days=28)).isoformat()} for Sunday {sunday}")


if __name__ == "__main__":
    main(sys.argv[1])
