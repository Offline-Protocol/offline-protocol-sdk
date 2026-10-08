"""The demo provider in ``examples/http-front`` against two real hosts.

It is an ordinary HTTP program with no SDK import; the test runs it as a
separate process, lets it register with the far front, and calls it from the
near one. The example ships with the repository only, so the test skips
when the tests run from outside a checkout.
"""

from __future__ import annotations

import asyncio
import json
import os
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
        # Shorter than the wait below, so a refused or unreachable front
        # ends the provider with its reason rather than the test timing out.
        "--wait",
        "10",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
    )
    try:
        line = await asyncio.wait_for(process.stdout.readline(), 30)
        if not line:
            stderr = await asyncio.wait_for(process.stderr.read(), 5)
            pytest.fail(f"the provider exited before registering: {stderr.decode(errors='replace')}")
        assert json.loads(line)["registered"] == "timeofday"
        status, _, body = await hosts.request(
            hosts.a, host_for("timeofday", "bob"), "/now", method="GET", body=None
        )
        assert status == 200
        assert json.loads(body)["caller"] == hosts.manager_a.local_address
    finally:
        process.kill()
        await process.wait()


async def test_the_demo_provider_stops_at_a_refusal_with_the_fronts_reason():
    """A front that answers is not starting: its refusal (a token it lacks,
    a callback it will not take) ends the provider with the front's own
    detail, instead of retrying it until the deadline."""
    if not PROVIDER.exists():
        pytest.skip("the example ships with the repository only")
    from aiohttp import web

    async def refuse(request: web.Request) -> web.Response:
        return web.json_response({"error": "unauthorized", "detail": "the token is missing"}, status=401)

    app = web.Application()
    app.router.add_put("/services/timeofday", refuse)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    port = site._server.sockets[0].getsockname()[1]
    try:
        process = await asyncio.create_subprocess_exec(
            sys.executable,
            str(PROVIDER),
            "--front",
            f"http://127.0.0.1:{port}",
            "--port",
            "0",
            "--wait",
            "30",
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.PIPE,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        try:
            _, stderr = await asyncio.wait_for(process.communicate(), 10)
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()
        assert process.returncode != 0
        assert "401" in stderr.decode() and "the token is missing" in stderr.decode()
    finally:
        await runner.cleanup()
