"""Schema migrations (yoyo) and backups."""

from server import db as gdb


def test_fresh_database_gets_every_migration_once(tmp_path):
    path = tmp_path / "game.db"
    applied = gdb.init(path)
    assert applied and applied[0] == "001_initial"
    assert gdb.init(path) == []
    c = gdb.connect(path)
    tables = {r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    assert {"accounts", "companies", "orders", "trades", "ledger", "lots", "ipo_applications"} <= tables


def test_backup_is_a_usable_copy(tmp_path):
    gdb.init(tmp_path / "g.db")
    c = gdb.connect(tmp_path / "g.db")
    c.execute("INSERT INTO state(key, value) VALUES ('x', '1')")
    copy = gdb.connect(gdb.backup(c, tmp_path / "backups" / "copy.db"))
    assert copy.execute("SELECT value FROM state WHERE key='x'").fetchone()[0] == "1"
