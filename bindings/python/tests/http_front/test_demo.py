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


async def _run_provider_against(*answers: tuple[int, dict]) -> tuple[int, str, str, int]:
    """Runs the provider against a stub front that gives ``answers`` in turn
    (the last one repeated); returns its exit code, stdout, stderr and the
    number of registration attempts."""
    from aiohttp import web

    attempts = 0

    async def answer(request: web.Request) -> web.Response:
        nonlocal attempts
        status, doc = answers[min(attempts, len(answers) - 1)]
        attempts += 1
        return web.json_response(doc, status=status)

    app = web.Application()
    app.router.add_put("/services/timeofday", answer)
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
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        try:
            # A registered provider serves until killed: read its one line.
            line = await asyncio.wait_for(process.stdout.readline(), 10)
            if line:
                return 0, line.decode(), "", attempts
            _, stderr = await asyncio.wait_for(process.communicate(), 10)
            return process.returncode, "", stderr.decode(), attempts
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()
    finally:
        await runner.cleanup()


async def test_the_demo_provider_stops_at_a_refusal_with_the_fronts_reason():
    """A 4xx is the front refusing this registration (a token it lacks, a
    callback it will not take): the provider stops with the front's own
    detail, instead of retrying it until the deadline."""
    if not PROVIDER.exists():
        pytest.skip("the example ships with the repository only")
    code, _, stderr, attempts = await _run_provider_against(
        (401, {"error": "unauthorized", "detail": "the token is missing"})
    )
    assert code != 0
    assert "401" in stderr and "the token is missing" in stderr
    assert attempts == 1


async def test_the_demo_provider_retries_a_front_still_connecting():
    """A front listens before it reaches the service and answers 503 until
    then: a provider started beside it waits that out."""
    if not PROVIDER.exists():
        pytest.skip("the example ships with the repository only")
    code, stdout, _, attempts = await _run_provider_against(
        (503, {"error": "not_connected"}), (503, {"error": "not_connected"}), (200, {})
    )
    assert code == 0
    assert json.loads(stdout)["registered"] == "timeofday"
    assert attempts == 3
