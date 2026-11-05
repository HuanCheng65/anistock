"""Run the v1 server (`server.app`) with a frozen clock, for the API difftest.

    V1_ROOT=<root> ANISTOCK_FREEZE_TIME=2026-11-05T10:00:00+08:00 PORT=18751 python backend/difftest/v1_frozen.py

`V1_ROOT` is the repository root to import `server` from (the difftest uses a temporary copy so that
`web/dist` and `sim/matches.json` can be test fixtures). The game clock and `time.time()` (session
expiry) are pinned to ANISTOCK_FREEZE_TIME; nothing in v1 is modified.
"""

import os
import sys
from datetime import datetime

sys.path.insert(0, os.environ["V1_ROOT"])

import uvicorn  # noqa: E402

import server.app as v1  # noqa: E402

when = datetime.fromisoformat(os.environ["ANISTOCK_FREEZE_TIME"])
v1.clock.freeze(when.astimezone(v1.clock.tz))


class _FrozenTime:
    @staticmethod
    def time() -> float:
        return when.timestamp()


v1.time = _FrozenTime
uvicorn.run(v1.app, host="127.0.0.1", port=int(os.environ["PORT"]), log_level="warning")
