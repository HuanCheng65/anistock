"""Compare two game databases row by row (e.g. the same days closed by v1 Python and by the Rust server).

    uv run python -m server.tools.compare_close A.db B.db [--tables ledger,news] [--ignore-columns ts,created_at]
                                                       [--skip-tables _yoyo_migration] [--by-content] [--limit 10]

Every table present in either database is compared (SQLite internals and the migration bookkeeping
tables are skipped unless named in --tables). Rows are matched by rowid (by primary key for WITHOUT ROWID
tables); values must be identical, floats bit for bit. With --by-content, rows are compared as multisets
instead (for an implementation that inserts the same rows in another order, so rowids differ; combine
with --ignore-columns id to drop surrogate keys). Prints per-table counts and the first differences;
exits 1 if anything differs. Both databases are opened read-only.
"""

from __future__ import annotations

import argparse
import sqlite3
import sys

SKIP = ("sqlite_", "_yoyo")


def ro(path: str) -> sqlite3.Connection:
    return sqlite3.connect(f"file:{path}?mode=ro", uri=True)


def tables(db: sqlite3.Connection) -> list[str]:
    return [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]


def columns(db: sqlite3.Connection, t: str) -> list[str]:
    return [r[1] for r in db.execute(f'PRAGMA table_info("{t}")')]


def rows(db: sqlite3.Connection, t: str, cols: list[str]) -> dict[tuple, tuple]:
    """Rows keyed by rowid, or by primary key for WITHOUT ROWID tables."""
    sel = ", ".join(f'"{c}"' for c in cols)
    try:
        return {(r[0],): r[1:] for r in db.execute(f'SELECT rowid, {sel} FROM "{t}"')}
    except sqlite3.OperationalError:
        pk = [r[1] for r in sorted(db.execute(f'PRAGMA table_info("{t}")'), key=lambda r: r[5]) if r[5]] or cols
        idx = [cols.index(c) for c in pk]
        return {tuple(r[i] for i in idx): r for r in db.execute(f'SELECT {sel} FROM "{t}"')}


def content(db: sqlite3.Connection, t: str, cols: list[str]) -> dict[tuple, tuple]:
    """Rows keyed by their own values (with a counter for duplicates), for order-independent comparison."""
    sel = ", ".join(f'"{c}"' for c in cols)
    out, seen = {}, {}
    for r in sorted(db.execute(f'SELECT {sel} FROM "{t}"'), key=repr):
        n = seen[r] = seen.get(r, 0) + 1
        out[(repr(r), n)] = r
    return out


def same(x, y) -> bool:
    return type(x) is type(y) and x == y or (isinstance(x, float) and isinstance(y, float) and x != x and y != y)


def compare(a_path: str, b_path: str, only: list[str] | None = None, skip: list[str] = (),
            ignore_cols: set[str] = frozenset(), limit: int = 10, by_content: bool = False, out=sys.stdout) -> int:
    """Print differences; returns the number of differing rows (plus schema differences)."""
    a, b = ro(a_path), ro(b_path)
    ta, tb = set(tables(a)), set(tables(b))
    names = only or sorted(t for t in ta | tb if not t.startswith(SKIP) and t not in skip)
    total = 0
    for t in names:
        if t not in ta or t not in tb:
            print(f"{t}: only in {'A' if t in ta else 'B'}", file=out)
            total += 1
            continue
        ca, cb = columns(a, t), columns(b, t)
        if ca != cb:
            print(f"{t}: columns differ\n  A {ca}\n  B {cb}", file=out)
        cols = [c for c in ca if c in cb and c not in ignore_cols]
        get = content if by_content else rows
        ra, rb = get(a, t, cols), get(b, t, cols)
        diffs = []
        for k in sorted(ra.keys() | rb.keys(), key=repr):
            x, y = ra.get(k), rb.get(k)
            if x is None or y is None:
                diffs.append(f"  key {k}: only in {'A' if y is None else 'B'}: {x if y is None else y}")
            elif not all(same(u, v) for u, v in zip(x, y)):
                d = [f"{c}: {u!r} != {v!r}" for c, u, v in zip(cols, x, y) if not same(u, v)]
                diffs.append(f"  key {k}: " + "; ".join(d))
        status = "identical" if not diffs else f"{len(diffs)} differing rows"
        print(f"{t}: A {len(ra)} rows, B {len(rb)} rows, {status}", file=out)
        for line in diffs[:limit]:
            print(line, file=out)
        if len(diffs) > limit:
            print(f"  ... {len(diffs) - limit} more", file=out)
        total += len(diffs) + (ca != cb)
    return total


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--tables", help="comma-separated tables to compare (default: all)")
    ap.add_argument("--skip-tables", default="", help="comma-separated tables to leave out")
    ap.add_argument("--ignore-columns", default="", help="comma-separated column names to leave out everywhere")
    ap.add_argument("--by-content", action="store_true", help="match rows by value, not rowid")
    ap.add_argument("--limit", type=int, default=10, help="differences printed per table")
    args = ap.parse_args()
    split = lambda s: [x for x in s.split(",") if x]  # noqa: E731
    n = compare(args.a, args.b, split(args.tables) if args.tables else None, split(args.skip_tables),
                set(split(args.ignore_columns)), args.limit, args.by_content)
    print("IDENTICAL" if n == 0 else f"DIFFERENT: {n} rows")
    sys.exit(1 if n else 0)


if __name__ == "__main__":
    main()
