"""HTTP API difftest: the same scripted requests against the v1 server (FastAPI) and the v2 server (Rust),
each on its own copy of one seeded game database, with the clock frozen; then every response (status,
content type, Set-Cookie, body) and finally every table of both databases are compared.

    uv run python backend/difftest/api_diff.py [--v2 BIN] [--work DIR] [--scenario NAME ...] [--show]

    --v2 BIN       the v2 binary (default backend/target/release/anistock); "none" runs v1 only and prints
                   its responses (capture mode)
    --show         print every response pair, not only the differences

Scenarios (each starts both servers fresh from the same base database):
    open      ANISTOCK_ALWAYS_OPEN=1, 10:00: accounts, validation, orders, cancels, every GET, web/dist
    auction   08:50 (opening auction phase): orders rest, the auction preview, market orders refused
    closed    23:30: orders refused; reads
    openfail  09:00:05, the first orders of the day are rejected: the opening auction they triggered is
              rolled back with them (v1's one transaction), so nothing about the open is written
    nodist    like open but without web/dist (no SPA catch-all: Starlette 404 / 405 bodies)
    dbsecret  ANISTOCK_SECRET unset, state.session_secret seeded: cookies signed with the stored key
    gensecret ANISTOCK_SECRET unset and no stored key: each server creates one (compared for shape only)

JSON bodies are compared as parsed values with int and float kept apart (100000 != 100000.0) and also
byte for byte (reported separately as "bytes"). Database rows that legitimately differ: `pw_hash` of
accounts registered during the run (random salt; each is checked with v1's check_pw instead) and a
generated `state.session_secret`.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import http.client
import json
import re
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
PY = REPO / ".venv" / "bin" / "python"
PORTS = {"v1": 18742, "v2": 18743}
SECRET = "difftest-secret-0123456789"
INVITE = "club-invite"

sys.path.insert(0, str(HERE))
import compare  # noqa: E402

SCENARIOS = {
    "open": dict(time="2026-11-05T10:00:00+08:00", always_open=True, dist=True, secret=SECRET),
    "auction": dict(time="2026-11-05T08:50:00+08:00", always_open=False, dist=True, secret=SECRET),
    "closed": dict(time="2026-11-05T23:30:00+08:00", always_open=False, dist=True, secret=SECRET),
    "openfail": dict(time="2026-11-05T09:00:05+08:00", always_open=False, dist=True, secret=SECRET),
    "nodist": dict(time="2026-11-05T10:00:00+08:00", always_open=True, dist=False, secret=SECRET),
    "dbsecret": dict(time="2026-11-05T10:00:00+08:00", always_open=True, dist=True, secret=None, secret_in_db=True),
    "gensecret": dict(time="2026-11-05T10:00:00+08:00", always_open=True, dist=True, secret=None),
}


# ---------------------------------------------------------------- fixtures

def make_root(work: Path, dist: bool) -> Path:
    """A repository root for the test: a real copy of server/ (v1 finds its root from app.py's resolved
    path), sim/ as symlinks except a fixture matches.json, and optionally a fixture web/dist."""
    root = work / "root"
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    shutil.copytree(REPO / "server", root / "server", ignore=shutil.ignore_patterns("__pycache__"))
    (root / "engine").symlink_to(REPO / "engine")
    (root / "sim").mkdir()
    for f in (REPO / "sim").iterdir():
        if f.name != "matches.json":
            (root / "sim" / f.name).symlink_to(f)
    if dist:
        d = root / "web" / "dist"
        (d / "assets").mkdir(parents=True)
        (d / "index.html").write_text("<!doctype html><title>AniStock</title><div id=app></div>\n")
        (d / "assets" / "app-123.js").write_text("console.log('anistock')\n")
        (d / "assets" / "style.css").write_text("body{margin:0}\n")
        (d / "favicon.svg").write_text("<svg xmlns='http://www.w3.org/2000/svg'/>\n")
        (d / "sub").mkdir()
        (d / "sub" / "page.txt").write_text("plain text\n")
    return root


def seed(work: Path, root: Path, secret_in_db: bool) -> tuple[Path, Path]:
    base, coll = work / "base.db", work / "collector.db"
    args = [str(PY), str(HERE / "api_setup.py"), str(base), str(coll), str(root / "sim" / "matches.json")]
    if secret_in_db:
        args.append("--secret-in-db")
    subprocess.run(args, check=True, cwd=REPO)
    return base, coll


class Server:
    def __init__(self, name: str, cmd: list[str], env: dict, log: Path):
        self.name, self.port = name, int(env["PORT"])
        self.log = open(log, "w")
        self.proc = subprocess.Popen(cmd, env=env, stdout=self.log, stderr=subprocess.STDOUT, cwd=REPO)
        deadline = time.time() + 30
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise SystemExit(f"{name} exited at startup; see {log}")
            try:
                c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=2)
                c.request("GET", "/api/status")
                c.getresponse().read()
                return
            except OSError:
                time.sleep(0.1)
        raise SystemExit(f"{name} did not start; see {log}")

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)       # by PID
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.log.close()


def start(which: str, binary: str, root: Path, db: Path, coll: Path, sc: dict, log: Path) -> Server:
    env = {k: v for k, v in os.environ.items() if not k.startswith("ANISTOCK_")}
    env.update(ANISTOCK_GAME_DB=str(db), ANISTOCK_DB=str(coll), ANISTOCK_NO_LOOPS="1", ANISTOCK_INVITE=INVITE,
               ANISTOCK_FREEZE_TIME=sc["time"], PORT=str(PORTS[which]), ANISTOCK_PORT=str(PORTS[which]),
               ANISTOCK_HOST="127.0.0.1", ANISTOCK_ROOT=str(root), V1_ROOT=str(root),
               # the reference is v1: v1 economy rules and no v2 NPC market
               ANISTOCK_ECONOMY_V1="1")
    if sc["always_open"]:
        env["ANISTOCK_ALWAYS_OPEN"] = "1"
    if sc["secret"]:
        env["ANISTOCK_SECRET"] = sc["secret"]
    if which == "v1":
        return Server("v1", [str(PY), str(HERE / "v1_frozen.py")], env, log)
    pin = ["taskset", "-c", "64-127"] if shutil.which("taskset") else []
    return Server("v2", [*pin, binary], env, log)


# ---------------------------------------------------------------- requests

class Resp:
    def __init__(self, status: int, headers: dict, body: bytes):
        self.status, self.headers, self.body = status, headers, body

    def cookie(self) -> str | None:
        sc = self.headers.get("set-cookie")
        if sc and sc.startswith("session="):
            v = sc.split(";")[0][len("session="):]
            return v if v and v != '""' else None
        return None


def call(port: int, method: str, path: str, body=None, raw: bytes | None = None, cookie: str | None = None,
         ctype: str | None = "application/json", extra: dict | None = None) -> Resp:
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
    headers = dict(extra or {})
    data = raw
    if body is not None:
        data = json.dumps(body, ensure_ascii=False).encode()
    if data is not None and ctype:
        headers["Content-Type"] = ctype
    if cookie:
        headers["Cookie"] = f"session={cookie}"
    c.request(method, path, body=data, headers=headers)
    r = c.getresponse()
    out = Resp(r.status, {k.lower(): v for k, v in r.getheaders()}, r.read())
    c.close()
    return out


def typed(v):
    """A JSON value with ints and floats kept apart (Python's == treats 1 and 1.0 as equal)."""
    if isinstance(v, bool) or v is None or isinstance(v, str):
        return v
    if isinstance(v, int):
        return ("int", v)
    if isinstance(v, float):
        return ("float", v)
    if isinstance(v, list):
        return [typed(x) for x in v]
    return [(k, typed(x)) for k, x in v.items()]          # key order matters too


# ---------------------------------------------------------------- the script

def script(sc_name: str):
    """Steps: (label, method, path, kwargs). kwargs: body / raw / ctype / as_ (cookie jar name) /
    login_as (store the response cookie in this jar) / cross (use the other server's cookie)."""
    S = []

    def step(label, method, path, **kw):
        S.append((label, method, path, kw))

    reads = ["/api/status", "/api/market", "/api/stocks/101", "/api/stocks/102", "/api/stocks/103", "/api/stocks/104",
             "/api/stocks/105", "/api/stocks/106", "/api/stocks/107", "/api/stocks/101/candles",
             "/api/stocks/104/candles", "/api/stocks/101/intraday", "/api/ipos", "/api/leaderboard", "/api/news",
             "/api/macro"]
    for p in reads:
        step("anon read", "GET", p)
    for p in ["/api/me", "/api/portfolio", "/api/orders", "/api/trades", "/api/ledger"]:
        step("anon private", "GET", p)

    if sc_name in ("closed", "auction"):
        step("login p0", "POST", "/api/login", body={"username": "p0", "password": "secret0"}, login_as="p0")
        step("p0 me", "GET", "/api/me", as_="p0")
        step("limit order", "POST", "/api/orders", as_="p0",
             body={"company_id": 101, "side": "buy", "kind": "limit", "qty": 10, "price": 499})
        step("market order", "POST", "/api/orders", as_="p0",
             body={"company_id": 101, "side": "buy", "kind": "market", "qty": 10})
        step("bad price closed", "POST", "/api/orders", as_="p0",
             body={"company_id": 101, "side": "buy", "kind": "limit", "qty": 10, "price": "x"})
        step("unknown co", "POST", "/api/orders", as_="p0",
             body={"company_id": 999, "side": "buy", "kind": "limit", "qty": 10, "price": 1})
        step("crossing sell", "POST", "/api/orders", as_="p0",
             body={"company_id": 102, "side": "sell", "kind": "limit", "qty": 1, "price": 1000})
        for p in ["/api/stocks/101", "/api/stocks/102", "/api/stocks/104", "/api/market", "/api/orders",
                  "/api/orders?status=all", "/api/portfolio", "/api/status"]:
            step("after", "GET", p, as_="p0")
        return S

    if sc_name == "openfail":
        step("login p0", "POST", "/api/login", body={"username": "p0", "password": "secret0"}, login_as="p0")
        step("delisted", "POST", "/api/orders", as_="p0",
             body={"company_id": 105, "side": "buy", "kind": "limit", "qty": 10, "price": 499})
        step("too big", "POST", "/api/orders", as_="p0",
             body={"company_id": 101, "side": "buy", "kind": "limit", "qty": 9_000_000, "price": 9000})
        step("no shares", "POST", "/api/orders", as_="p0",
             body={"company_id": 101, "side": "sell", "kind": "market", "qty": 100_000})
        for p in ["/api/stocks/101", "/api/stocks/104", "/api/market", "/api/status", "/api/orders?status=all"]:
            step("after", "GET", p, as_="p0")
        return S

    if sc_name in ("dbsecret", "gensecret"):
        step("login p1", "POST", "/api/login", body={"username": "p1", "password": "secret1"}, login_as="p1")
        step("p1 me", "GET", "/api/me", as_="p1")
        step("register", "POST", "/api/register",
             body={"username": "carol", "password": "123456", "invite": INVITE}, login_as="carol")
        step("carol me", "GET", "/api/me", as_="carol")
        step("cross p1", "GET", "/api/me", as_="p1", cross=True)
        return S

    # ---- accounts: validation
    reg = "/api/register"
    for label, b in [
            ("empty obj", {}), ("short name", {"username": "a", "password": "123456", "invite": INVITE}),
            ("long name", {"username": "x" * 21, "password": "123456", "invite": INVITE}),
            ("short pw", {"username": "alice", "password": "12345", "invite": INVITE}),
            ("long pw", {"username": "alice", "password": "p" * 101, "invite": INVITE}),
            ("int name", {"username": 12345, "password": "123456", "invite": INVITE}),
            ("null name", {"username": None, "password": "123456", "invite": INVITE}),
            ("int invite", {"username": "alice", "password": "123456", "invite": 5}),
            ("no invite", {"username": "alice", "password": "123456"}),
            ("bad invite", {"username": "alice", "password": "123456", "invite": "nope"}),
            ("two errors", {"username": "a", "password": "1"}),
            ("taken", {"username": "p3", "password": "123456", "invite": INVITE}),
            ("taken strip", {"username": " p3 ", "password": "123456", "invite": INVITE}),
            ("taken cjk", {"username": "老玩家", "password": "123456", "invite": INVITE}),
            ("list body", [1, 2]), ("str body", "hello"), ("num body", 5)]:
        step(f"register {label}", "POST", reg, body=b)
    step("register invalid json", "POST", reg, raw=b'{"username": "alice",')
    step("register empty body", "POST", reg, raw=b"")
    step("register no ctype", "POST", reg, raw=json.dumps({"username": "zed", "password": "123456",
                                                            "invite": INVITE}).encode(), ctype=None)
    step("register text ctype", "POST", reg, raw=b'{"username": "yan", "password": "123456"}', ctype="text/plain")
    step("register non-utf8", "POST", reg, raw=b'{"username": "\xff\xfe", "password": "123456"}')
    # ---- accounts: success
    step("register alice", "POST", reg, body={"username": "  alice  ", "password": "123456", "invite": INVITE,
                                              "extra": 1}, login_as="alice")
    step("register bob", "POST", reg, body={"username": "芙莉莲粉丝俱乐部的会长大人好", "password": "长密码长密码",
                                            "invite": INVITE}, login_as="bob")
    step("register dup", "POST", reg, body={"username": "alice", "password": "123456", "invite": INVITE})
    step("alice me", "GET", "/api/me", as_="alice")
    step("alice portfolio", "GET", "/api/portfolio", as_="alice")
    step("bob me", "GET", "/api/me", as_="bob")
    for label, b in [("wrong pw", {"username": "alice", "password": "1234567"}),
                     ("unknown", {"username": "nobody", "password": "1234567"}),
                     ("short pw", {"username": "alice", "password": "1"}),
                     ("null-hash player", {"username": "老玩家", "password": "1234567"}),
                     ("committee (no username)", {"username": "holder101", "password": "1234567"})]:
        step(f"login {label}", "POST", "/api/login", body=b)
    step("login alice strip", "POST", "/api/login", body={"username": " alice ", "password": "123456"}, login_as="alice2")
    step("login p0", "POST", "/api/login", body={"username": "p0", "password": "secret0"}, login_as="p0")
    step("login p1", "POST", "/api/login", body={"username": "p1", "password": "secret1"}, login_as="p1")
    step("p0 me", "GET", "/api/me", as_="p0")
    step("alice2 me", "GET", "/api/me", as_="alice2")
    step("cross p0 me", "GET", "/api/me", as_="p0", cross=True)
    step("cross alice portfolio", "GET", "/api/portfolio", as_="alice", cross=True)
    for label, ck in [("garbage", "garbage"), ("two dots", "1.2"), ("bad sig", "4.4102444800." + "0" * 64),
                      ("four parts", "4.1.2.3"), ("empty", "")]:
        step(f"cookie {label}", "GET", "/api/me", cookie=ck)
    step("logout", "POST", "/api/logout", as_="alice2")
    step("logout anon", "POST", "/api/logout")

    # ---- orders: validation
    o = "/api/orders"
    good = {"company_id": 101, "side": "buy", "kind": "limit", "qty": 10, "price": 5}
    step("order anon bad body", "POST", o, body={"qty": 0})
    step("order anon", "POST", o, body=good)
    bad = [("qty 0", {"qty": 0}), ("qty -1", {"qty": -1}), ("qty big", {"qty": 10_000_001}),
           ("qty max", {"qty": 10_000_000, "price": 0.01}),
           ("qty str", {"qty": "5"}), ("qty float int", {"qty": 5.0}), ("qty float", {"qty": 5.5}),
           ("qty bool", {"qty": True}), ("qty null", {"qty": None}), ("qty str bad", {"qty": "five"}),
           ("qty huge", {"qty": 10 ** 30}), ("co str", {"company_id": "101"}), ("co str bad", {"company_id": "x"}),
           ("co float", {"company_id": 101.0}), ("side", {"side": "hold"}), ("side int", {"side": 1}),
           ("kind", {"kind": "stop"}), ("kind null", {"kind": None}),
           ("price str", {"price": "abc"}), ("price 3dp str", {"price": "5.005"}), ("price 3dp", {"price": 5.005}),
           ("price neg", {"price": -1}), ("price 0", {"price": 0}), ("price -0", {"price": "-0"}),
           ("price exp", {"price": "1e2"}), ("price space", {"price": " 5 "}),
           ("price under", {"price": "5_0"}), ("price bool", {"price": True}), ("price null", {"price": None}),
           ("price list", {"price": [1]}), ("price obj", {"price": {"a": 1}}), ("price tiny", {"price": 1e-7}),
           ("price big", {"price": 1e30}), ("price huge str", {"price": "1e400"}), ("price nan", {"price": "NaN"}),
           ("price inf", {"price": "Infinity"}), ("price float", {"price": 4.99}), ("price 0.1+0.2", {"price": 0.30000000000000004}),
           ("price int", {"price": 1001}), ("price too high", {"price": 100.01}), ("price cap", {"price": 100}),
           ("co 999", {"company_id": 999}), ("co delisted", {"company_id": 105}),
           ("co subscribing", {"company_id": 106}), ("co 999 bad price", {"company_id": 999, "price": "x"}),
           ("co 999 bad side", {"company_id": 999, "side": "x"})]
    for label, patch in bad:
        step(f"order {label}", "POST", o, body={**good, **patch}, as_="alice")
    for label, b in [("missing qty", {k: v for k, v in good.items() if k != "qty"}),
                     ("missing side", {k: v for k, v in good.items() if k != "side"}),
                     ("missing all", {}), ("defaults", {"company_id": 101, "side": "buy", "qty": 1, "price": 4})]:
        step(f"order {label}", "POST", o, body=b, as_="alice")
    step("order invalid json", "POST", o, raw=b"{bad", as_="alice")
    step("order market no price", "POST", o, body={"company_id": 103, "side": "buy", "kind": "market", "qty": 1,
                                                   "price": "junk"}, as_="alice")

    # ---- orders: trading (the first order of the day runs the opening auction)
    trades = [
        ("p0", {"company_id": 101, "side": "buy", "kind": "limit", "qty": 7, "price": 4.9}),
        ("alice", {"company_id": 101, "side": "buy", "kind": "limit", "qty": 40, "price": "5.10"}),
        ("alice", {"company_id": 101, "side": "buy", "kind": "market", "qty": 30}),
        ("alice", {"company_id": 102, "side": "buy", "kind": "limit", "qty": 3, "price": 12.5}),
        ("bob", {"company_id": 102, "side": "buy", "kind": "market", "qty": 5}),
        ("p0", {"company_id": 101, "side": "sell", "kind": "limit", "qty": 5, "price": 5.06}),
        ("alice", {"company_id": 101, "side": "sell", "kind": "limit", "qty": 10, "price": 4.95}),
        ("alice", {"company_id": 101, "side": "sell", "kind": "limit", "qty": 1000, "price": 4.95}),
        ("bob", {"company_id": 103, "side": "buy", "kind": "limit", "qty": 100, "price": 0.04}),
        ("p1", {"company_id": 103, "side": "sell", "kind": "market", "qty": 3}),
        ("p1", {"company_id": 101, "side": "buy", "kind": "limit", "qty": 50, "price": 6.5}),   # VCM
        ("bob", {"company_id": 104, "side": "buy", "kind": "limit", "qty": 999999, "price": 0.9}),
        ("bob", {"company_id": 104, "side": "buy", "kind": "limit", "qty": 9999999, "price": 1}),
        ("p0", {"company_id": 104, "side": "buy", "kind": "limit", "qty": 3, "price": 0.95}),
    ]
    for who, b in trades:
        step(f"trade {who}", "POST", o, body=b, as_=who)
    step("my orders", "GET", o, as_="alice")
    step("my orders all", "GET", o + "?status=all", as_="alice")
    step("my orders weird", "GET", o + "?status=", as_="p0")
    for oid, who in [(1, "p0"), (36, "p0"), (36, "p0"), (39, "p0"), (42, "alice"), (99999, "alice"), (0, "alice"),
                     (-3, "alice"), (43, "bob"), (43, "bob"), (10 ** 20, "bob")]:
        step(f"cancel {oid} as {who}", "DELETE", f"{o}/{oid}", as_=who)
    step("cancel abc", "DELETE", o + "/abc", as_="bob")
    step("cancel anon", "DELETE", o + "/36")
    step("cancel no id", "DELETE", o + "/", as_="bob")
    for who in ["alice", "bob", "p0", "p1"]:
        for p in ["/api/me", "/api/portfolio", "/api/orders", "/api/orders?status=all", "/api/trades", "/api/ledger",
                  "/api/stocks/101", "/api/stocks/102", "/api/stocks/104", "/api/ipos"]:
            step(f"{who} read", "GET", p, as_=who)
    for p in reads + ["/api/news?limit=2", "/api/news?limit=-1", "/api/news?limit=0", "/api/news?limit=1000",
                      "/api/news?limit=abc", "/api/news?limit=2.5", "/api/stocks/abc", "/api/stocks/999",
                      "/api/stocks/999/candles", "/api/stocks/999/intraday", "/api/stocks/101/candles?x=1",
                      "/api/stocks/-1", "/api/stocks/0101", "/api/stocks/+101", "/api/stocks/1e2"]:
        step("read after", "GET", p)

    # ---- IPO subscription: checks, then applications replaced, withdrawn, refused for lack of cash
    for who, path, b in [(None, "/api/ipos/106/subscribe", {"qty": 10}), ("alice", "/api/ipos/105/subscribe", {"qty": 10}),
                         ("alice", "/api/ipos/999/subscribe", {"qty": 10}), ("alice", "/api/ipos/104/subscribe", {"qty": 10}),
                         ("alice", "/api/ipos/107/subscribe", {"qty": 10}), ("alice", "/api/ipos/106/subscribe", {"qty": -1}),
                         ("alice", "/api/ipos/106/subscribe", {"qty": 10_000_001}), ("alice", "/api/ipos/106/subscribe", {}),
                         ("alice", "/api/ipos/abc/subscribe", {"qty": 1})]:
        step("subscribe", "POST", path, body=b, as_=who)
    for who, path in [(None, "/api/ipos/106/subscribe"), ("alice", "/api/ipos/105/subscribe"),
                      ("alice", "/api/ipos/999/subscribe")]:
        step("withdraw", "DELETE", path, as_=who)
    for who, qty in [("alice", 10), ("alice", 25), ("bob", 7), ("p0", 9_000_000), ("p8", 1), ("p1", 0), ("bob", "3")]:
        step(f"subscribe {who} {qty}", "POST", "/api/ipos/106/subscribe", body={"qty": qty}, as_=who)
    step("withdraw p0", "DELETE", "/api/ipos/106/subscribe", as_="p0")
    step("withdraw bob", "DELETE", "/api/ipos/106/subscribe", as_="bob")
    step("withdraw bob again", "DELETE", "/api/ipos/106/subscribe", as_="bob")
    for who in ["alice", "bob", "p0", "p1"]:
        for p in ["/api/ipos", "/api/stocks/106", "/api/me", "/api/portfolio"]:
            step(f"{who} ipo read", "GET", p, as_=who)
    step("anon ipos", "GET", "/api/ipos")

    # ---- odd requests: content types, encodings, HEAD, conditional GET, odd JSON
    good_reg = json.dumps({"username": "zed", "password": "123456", "invite": INVITE}).encode()
    for label, raw, ct in [
            ("ct charset", good_reg, "application/json; charset=utf-8"), ("ct upper", b'{"username":"a"}', "APPLICATION/JSON"),
            ("ct +json", b'{"username":"a"}', "application/vnd.api+json"), ("ct garbage", b'{"username":"a"}', "json"),
            ("ct form", b"username=a&password=b", "application/x-www-form-urlencoded"),
            ("raw non-utf8 text", b"\xff\xfe", "text/plain"), ("bom", b"\xef\xbb\xbf{}", "application/json"),
            ("json null", b"null", "application/json"), ("json ws", b"   ", "application/json"),
            ("json nan", b'{"username": NaN, "password": "123456"}', "application/json"),
            ("json dup", b'{"username": "x", "username": "yy", "password": "1"}', "application/json"),
            ("json trailing", b'{"username": "yy"} x', "application/json"),
            ("json ctrl", b'{"username": "a\x01b"}', "application/json"),
            ("json escape", b'{"username": "\\q"}', "application/json"),
            ("json big", b'{"username": 1e400, "password": 123456789012345678901234567890}', "application/json"),
            ("json surrogate", b'{"username": "\\ud83d\\ude00\\ud83d\\ude00", "password": "1"}', "application/json"),
            ("json nested", b'{"username": {"a": [1, {"b": null}]}, "password": [true, false, -0.0, 1.5e-9]}', "application/json")]:
        step(f"register {label}", "POST", reg, raw=raw, ctype=ct)
    step("order raw nan", "POST", o, raw=b'{"company_id": 101, "side": "buy", "qty": NaN, "price": 1}', as_="alice")
    step("order raw inf price", "POST", o, raw=b'{"company_id": 101, "side": "buy", "qty": 1, "price": -Infinity}', as_="alice")
    step("order raw qty inf", "POST", o, raw=b'{"company_id": 101, "side": "buy", "qty": Infinity}', as_="alice")
    step("order raw big qty", "POST", o, raw=b'{"company_id": 101, "side": "buy", "qty": 1e19}', as_="alice")
    step("order raw qty -0.0", "POST", o, raw=b'{"company_id": 101, "side": "buy", "qty": -0.0}', as_="alice")
    step("order raw co big", "POST", o, raw=b'{"company_id": 100000000000000000000, "side": "buy", "qty": 1, "price": 1}', as_="alice")
    step("order no auth bad json", "POST", o, raw=b'{x')
    step("order no auth non-utf8", "POST", o, raw=b'\xff')
    step("subscribe bad path + body", "POST", "/api/ipos/abc/subscribe", body={"qty": "x"}, as_="alice")
    step("subscribe no auth bad json", "POST", "/api/ipos/106/subscribe", raw=b"[")
    step("head status", "HEAD", "/api/status")
    step("head spa", "HEAD", "/portfolio")
    step("head asset", "HEAD", "/assets/app-123.js")
    step("post asset", "POST", "/assets/app-123.js", body={})
    step("asset 304", "GET", "/assets/app-123.js", inm_from="/assets/app-123.js")
    step("asset etag mismatch", "GET", "/assets/style.css", inm_from="/assets/app-123.js")
    step("spa ignores inm", "GET", "/favicon.svg", inm_from="/favicon.svg")
    step("options", "OPTIONS", "/api/status")
    step("query junk", "GET", "/api/news?limit=3&limit=1&x")
    step("query plus", "GET", "/api/orders?status=open+", as_="alice")
    step("query pct", "GET", "/api/news?limit=%32")
    step("path pct", "GET", "/api/stocks/%31%30%31/candles")
    step("path space", "GET", "/api/stocks/%20101")
    step("path huge", "GET", "/api/stocks/99999999999999999999999")
    step("path huge neg", "GET", "/api/stocks/-99999999999999999999999/intraday")
    step("news huge neg", "GET", "/api/news?limit=-99999999999999999999999")
    step("news huge", "GET", "/api/news?limit=99999999999999999999999")
    step("cookie quoted", "GET", "/api/me", cookie='"' + "x" + '"')

    # ---- routing and the SPA
    for p in ["/", "/index.html", "/assets/app-123.js", "/assets/style.css", "/assets/missing.js", "/favicon.svg",
              "/sub/page.txt", "/sub", "/portfolio", "/stocks/101", "/api", "/api/", "/api/nope", "/api/register",
              "/api/stocks/101/nope", "/%2e%2e/server/app.py", "/..%2fserver/app.py", "/assets/../index.html",
              "/api/market/", "/docs-not-here"]:
        step("route", "GET", p)
    step("route put", "PUT", "/api/orders", body={})
    step("route post market", "POST", "/api/market", body={})
    step("route delete", "DELETE", "/api/me")
    step("route post spa", "POST", "/portfolio", body={})
    return S


# ---------------------------------------------------------------- run and compare

def run_scenario(name: str, binary: str | None, work: Path, show: bool) -> int:
    sc = SCENARIOS[name]
    w = work / name
    if w.exists():
        shutil.rmtree(w)
    w.mkdir(parents=True)
    root = make_root(w, sc["dist"])
    base, coll = seed(w, root, sc.get("secret_in_db", False))
    dbs = {}
    servers = {}
    for which in (["v1", "v2"] if binary else ["v1"]):
        dbs[which] = w / f"{which}.db"
        shutil.copy(base, dbs[which])
    try:
        for which in dbs:
            servers[which] = start(which, binary, root, dbs[which], coll, sc, w / f"{which}.log")
        jars: dict[str, dict[str, str]] = {k: {} for k in servers}
        bad = 0
        n = 0
        for label, method, path, kw in script(name):
            out = {}
            for which, srv in servers.items():
                jar = jars[which]
                ck = kw.get("cookie")
                if kw.get("as_"):
                    other = "v2" if which == "v1" else "v1"
                    src = jars[other] if kw.get("cross") and other in jars else jar
                    ck = src.get(kw["as_"])
                extra = None
                if kw.get("inm_from"):        # conditional GET with this server's own etag
                    extra = {"If-None-Match": call(srv.port, "GET", kw["inm_from"]).headers.get("etag", "")}
                r = call(srv.port, method, path, body=kw.get("body"), raw=kw.get("raw"), cookie=ck,
                         ctype=kw.get("ctype", "application/json"), extra=extra)
                if kw.get("login_as") and r.cookie():
                    jar[kw["login_as"]] = r.cookie()
                out[which] = r
            n += 1
            bad += report(n, label, method, path, out, show, gensecret=(name == "gensecret"))
        print(f"[{name}] {n} requests, {bad} differ")
    finally:
        for s in servers.values():
            s.stop()
    if binary:
        bad += compare_dbs(dbs["v1"], dbs["v2"], base, gensecret=(name == "gensecret"))
    return bad


def report(n, label, method, path, out: dict[str, Resp], show: bool, gensecret: bool) -> int:
    head = f"#{n} {label}: {method} {path}"
    if "v2" not in out:
        r = out["v1"]
        print(f"{head} -> {r.status} {r.headers.get('content-type')} cookie={r.headers.get('set-cookie')}\n    "
              f"{r.body[:600].decode(errors='replace')}")
        return 0
    a, b = out["v1"], out["v2"]
    diffs = []
    if a.status != b.status:
        diffs.append(f"status {a.status} vs {b.status}")
    ca, cb = a.headers.get("content-type", ""), b.headers.get("content-type", "")
    if ca.split(";")[0] != cb.split(";")[0]:
        diffs.append(f"content-type {ca!r} vs {cb!r}")
    sa, sb = a.headers.get("set-cookie"), b.headers.get("set-cookie")
    if sa and sb:   # Starlette's delete_cookie stamps the real time
        sa, sb = (re.sub(r"expires=[^;]*", "expires=<now>", x) for x in (sa, sb))
    if sa != sb and not (gensecret and sa and sb and sa.split(".")[:2] == sb.split(".")[:2]):
        diffs.append(f"set-cookie {sa!r} vs {sb!r}")
    bytes_only = False
    if ca.startswith("application/json") and (a.body or b.body):
        try:
            ja, jb = json.loads(a.body), json.loads(b.body)
            if typed(ja) != typed(jb):
                diffs.append("json")
            elif a.body != b.body:
                bytes_only = True
        except ValueError:
            diffs.append("unparseable json")
    elif a.body != b.body:
        diffs.append("body")
    for h in ("etag", "last-modified", "content-length", "location"):
        ha, hb = (x.headers.get(h, "").replace(f":{PORTS[k]}/", ":<port>/") for x, k in ((a, "v1"), (b, "v2")))
        if ha != hb:
            diffs.append(f"{h} {a.headers.get(h)!r} vs {b.headers.get(h)!r}")
    if diffs or show or bytes_only:
        tag = "DIFF" if diffs else ("bytes" if bytes_only else "same")
        print(f"{tag} {head}: {', '.join(diffs)}")
        if diffs or bytes_only:
            print(f"    v1 {a.status}: {a.body[:1500].decode(errors='replace')}")
            print(f"    v2 {b.status}: {b.body[:1500].decode(errors='replace')}")
    return 1 if diffs else 0


def check_pw(pw: str, stored: str) -> bool:
    salt, h = stored.split(":")
    return hmac.compare_digest(hashlib.scrypt(pw.encode(), salt=bytes.fromhex(salt), n=2 ** 14, r=8, p=1).hex(), h)


PASSWORDS = {"alice": "123456", "芙莉莲粉丝俱乐部的会长大人好": "长密码长密码", "carol": "123456", "zed": "123456"}


def compare_dbs(a: Path, b: Path, base: Path, gensecret: bool) -> int:
    """Every table of both databases; new accounts' pw_hash is checked with check_pw instead."""
    old_ids = {r[0] for r in sqlite3.connect(base).execute("SELECT id FROM accounts")}
    bad = 0
    for t in compare.TABLES:
        ca, xa = compare.rows(str(a), t)
        cb, xb = compare.rows(str(b), t)
        if t == "accounts":
            i = ca.index("pw_hash")
            u = ca.index("username")
            for rows in (xa, xb):
                for k, r in enumerate(rows):
                    if r[0] not in old_ids and r[i]:
                        if not check_pw(PASSWORDS.get(r[u], "?"), r[i]):
                            print(f"accounts: bad pw_hash for {r[u]}")
                            bad += 1
                        rows[k] = r[:i] + ("<checked>",) + r[i + 1:]
        if t == "state" and gensecret:
            for rows in (xa, xb):
                for k, r in enumerate(rows):
                    if r[0] == "session_secret":
                        assert len(bytes.fromhex(r[1])) == 32, r
                        rows[k] = (r[0], "<generated>")
        if (ca, xa) != (cb, xb):
            bad += 1
            print(f"DB {t}: differ ({len(xa)} vs {len(xb)} rows)")
            sa, sb = set(xa), set(xb)
            for r in sorted(sa - sb, key=str)[:5]:
                print(f"    v1 only: {r}")
            for r in sorted(sb - sa, key=str)[:5]:
                print(f"    v2 only: {r}")
    if not bad:
        print("DB: all tables identical")
    return bad


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--v2", default=str(REPO / "backend" / "target" / "release" / "anistock"))
    ap.add_argument("--work", default=None)
    ap.add_argument("--scenario", nargs="*", default=list(SCENARIOS))
    ap.add_argument("--show", action="store_true")
    a = ap.parse_args()
    work = Path(a.work) if a.work else Path(tempfile.mkdtemp(prefix="api-diff-"))
    binary = None if a.v2 == "none" else a.v2
    total = 0
    for name in a.scenario:
        total += run_scenario(name, binary, work, a.show)
    print(f"work dir: {work}")
    print("IDENTICAL" if total == 0 else f"{total} DIFFERENCES")
    sys.exit(1 if total else 0)


if __name__ == "__main__":
    main()
