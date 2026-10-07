"""The demo provider in ``examples/http-front`` against two real hosts.

It is an ordinary HTTP program with no SDK import; the test runs it as a
separate process, lets it register with the far front, and calls it from the
near one. The example ships with the repository only, so the test skips
where it is absent (the wheel suite runs from an installed package).
"""

from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path

import pytest

pytest.importorskip("aiohttp")

from .test_front import harness, host_for, hosts  # noqa: E402,F401

PROVIDER = Path(__file__).resolve().parents[4] / "examples" / "http-front" / "provider.py"


async def test_the_demo_provider_answers_through_two_fronts(hosts):
    if not PROVIDER.exists():
        pytest.skip("the example ships with the repository only")
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        str(PROVIDER),
        "--front",
        f"http://127.0.0.1:{hosts.b.port}",
        "--port",
        "0",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.DEVNULL,
        env={"PYTHONDONTWRITEBYTECODE": "1"},
    )
    try:
        line = await asyncio.wait_for(process.stdout.readline(), 30)
        assert json.loads(line)["registered"] == "timeofday"
        status, _, body = await hosts.request(
            hosts.a, host_for("timeofday", "bob"), "/now?tz=utc", method="GET", body=None
        )
        assert status == 200
        answer = json.loads(body)
        assert answer["tz"] == "utc"
        assert answer["caller"] == hosts.manager_a.local_address
    finally:
        process.kill()
        await process.wait()
