"""The verifier's commands against in-process servers.

Two engines with encryption off over loopback streams, from the local API
suite's harness: what is under test here is the command, its output and its
exit status, not the engine. The scenarios with encryption on and devices
switched off are in ``tests/scenarios``.
"""

from __future__ import annotations

import asyncio
import io
import json

import pytest

from offline_protocol_sdk.local_api.server import LocalApiServer
from offline_protocol_sdk.protocol_manager import ProtocolManager
from offline_protocol_sdk.verify import cli, commands
from offline_protocol_sdk.verify.client import VerifyClient

from local_api.conftest import harness, make_config  # noqa: F401
from local_api.test_examples import two_servers


def _output() -> commands.Output:
    return commands.Output(out=io.StringIO(), err=io.StringIO())


def _lines(output: commands.Output) -> list[dict]:
    return [json.loads(line) for line in output.out.getvalue().splitlines() if line.strip()]


def _target(server) -> commands.Target:
    return commands.Target(socket_path=str(server.socket_path))


async def test_state_reports_the_address_the_carriers_and_the_session(harness):
    server_a, server_b = await two_servers(harness)
    out = _output()
    status = await commands.state(_target(server_a), [server_b.manager.local_address], out)
    assert status == commands.PASS
    (line,) = _lines(out)
    report = line["state"]
    assert report["local_address"] == server_a.manager.local_address
    assert "WiFiDirect" in report["active_transports"]
    assert set(report) >= {
        "topology",
        "pending_ack_count",
        "retry_queue_size",
        "mesh_relay_stats",
        "sessions",
    }
    assert server_b.manager.local_address in report["sessions"]
    assert "forwarded" in report["mesh_relay_stats"]


async def test_send_then_await_passes_on_the_receipt_and_the_arrival(harness):
    server_a, server_b = await two_servers(harness)
    recipient = server_b.manager.local_address
    out = _output()
    # One connection for the send and the wait: a receipt that fires while no
    # client of the application is connected is not held.
    sender = await VerifyClient.open(socket_path=server_a.socket_path)
    try:
        message_id = await sender.call(
            "send_message", {"recipient": recipient, "content": "checked", "priority": "Medium"}
        )
        assert await commands._await_on(sender, message_id, "delivered", 15.0, out) == commands.PASS
    finally:
        await sender.close()
    result = _lines(out)[-1]
    assert result["result"] == "pass"
    assert result["event"]["type"] == "message_delivered"
    assert result["event"]["message_id"] == message_id
    assert result["event"]["transport"] == "wifiDirect"

    # The recipient's service held the message for the application id; a
    # verifier connecting afterwards still gets it.
    arrival = _output()
    assert await commands.await_message(_target(server_b), message_id, "received", 15.0, arrival) == commands.PASS
    event = _lines(arrival)[-1]["event"]
    assert event["type"] == "message_received" and event["content"] == "checked"


async def test_send_prints_the_id_await_matches(harness):
    server_a, server_b = await two_servers(harness)
    out = _output()
    assert await commands.send(_target(server_a), server_b.manager.local_address, "one", out) == commands.PASS
    message_id = _lines(out)[0]["sent"]["message_id"]
    arrival = _output()
    assert await commands.await_message(_target(server_b), message_id, "received", 15.0, arrival) == commands.PASS


async def test_await_times_out_with_status_two(harness):
    server = await harness.server(config=make_config(profile="alone"))
    out = _output()
    status = await commands.await_message(_target(server), "no-such-message", "delivered", 0.3, out)
    assert status == commands.TIMED_OUT
    assert _lines(out)[-1] == {"result": "timeout", "message_id": "no-such-message", "until": "delivered"}


async def test_await_fails_on_message_failed_and_prints_status_events(harness):
    server = await harness.server(config=make_config(profile="alone"))
    out = _output()
    client = await VerifyClient.open(socket_path=server.socket_path)
    try:
        # The engine's own events stand in for a real failure: what is under
        # test is that the command reads them, in order, by id.
        task = asyncio.ensure_future(commands._await_on(client, "m-1", "delivered", 5.0, out))
        await asyncio.sleep(0.1)
        for event in (
            {"type": "message_deferred", "message_id": "m-1", "recipient": "x", "reason": "peer_not_reachable"},
            {"type": "message_undeliverable", "message_id": "m-1", "recipient": "x", "reason": "recipient_unreachable"},
            {"type": "message_deferred", "message_id": "m-2", "recipient": "x", "reason": "peer_not_reachable"},
            {"type": "message_failed", "message_id": "m-1", "reason": "Outbox lifetime exceeded", "retry_count": 0},
        ):
            client._events.put_nowait(event)
        assert await task == commands.FAILED
    finally:
        await client.close()
    lines = _lines(out)
    assert [line["status"]["type"] for line in lines if "status" in line] == [
        "message_deferred",
        "message_undeliverable",
    ]
    assert lines[-1]["result"] == "failed"
    assert lines[-1]["event"]["reason"] == "Outbox lifetime exceeded"


async def test_pair_passes_once_confirmed_and_times_out_otherwise(harness, monkeypatch):
    server = await harness.server(config=make_config(profile="alone"))
    out = _output()
    status = await commands.pair(_target(server), "off1nobody", 0.4, out)
    assert status == commands.TIMED_OUT
    assert _lines(out)[-1]["result"] == "timeout"

    # A real confirmation needs encryption on and is what the scenarios
    # wait for; here the engine's answers are scripted to pin the polling.
    states = iter(["NoKeyPackage", "SessionPending", "SessionConfirmed"])
    real_call = VerifyClient.call

    async def fake_call(self, method, params=None):
        if method != "get_establishment_state":
            return await real_call(self, method, params)
        return next(states)

    monkeypatch.setattr(commands, "PAIR_POLL_INTERVAL", 0.01)
    monkeypatch.setattr(VerifyClient, "call", fake_call)
    out = _output()
    assert await commands.pair(_target(server), "off1somebody", 5.0, out) == commands.PASS
    assert _lines(out)[-1] == {"result": "pass", "peer": "off1somebody", "state": "SessionConfirmed"}


async def test_ping_reports_each_message_and_the_carrier(harness):
    server_a, server_b = await two_servers(harness)
    out = _output()
    status = await commands.ping(_target(server_a), server_b.manager.local_address, 0.1, 3, 15.0, out)
    assert status == commands.PASS
    lines = _lines(out)
    per_message = [line for line in lines if "seq" in line]
    assert sorted(line["seq"] for line in per_message) == [0, 1, 2]
    assert {line["outcome"] for line in per_message} == {"delivered"}
    assert lines[-1] == {"result": "pass", "sent": 3, "delivered": 3, "failed": 0, "carriers": ["wifiDirect"]}


async def test_ping_times_out_toward_nobody(harness):
    server = await harness.server(config=make_config(profile="alone", internet_enabled=False, wifi_direct_enabled=True))
    out = _output()
    status = await commands.ping(_target(server), "off1nobody", 0.05, 2, 0.3, out)
    assert status == commands.TIMED_OUT
    assert _lines(out)[-1] == {"result": "timeout", "sent": 2, "delivered": 0, "failed": 0, "carriers": []}


async def test_watch_logs_every_event_and_spans_a_restart(harness, tmp_path):
    server_a, server_b = await two_servers(harness)
    log = tmp_path / "events.jsonl"
    out = _output()
    seen: list[dict] = []
    watcher = asyncio.ensure_future(
        commands.watch(_target(server_a), log, 30.0, out, on_event=seen.append)
    )
    try:
        await asyncio.sleep(0.3)
        sender = await VerifyClient.open(socket_path=server_a.socket_path)
        try:
            message_id = await sender.call(
                "send_message",
                {"recipient": server_b.manager.local_address, "content": "logged", "priority": "Medium"},
            )
        finally:
            await sender.close()
        for _ in range(300):
            if any(e.get("type") == "message_delivered" and e.get("message_id") == message_id for e in seen):
                break
            await asyncio.sleep(0.05)
        else:
            raise AssertionError(f"no receipt in the watch: {[e.get('type') for e in seen]}")
    finally:
        watcher.cancel()
        await asyncio.gather(watcher, return_exceptions=True)
    logged = [json.loads(line) for line in log.read_text().splitlines()]
    assert any(line["event"].get("message_id") == message_id for line in logged)
    assert all("at_ms" in line for line in logged)


async def test_watch_reconnects_when_the_service_comes_back(harness, tmp_path, monkeypatch):
    monkeypatch.setattr(commands, "WATCH_RECONNECT_DELAY", 0.05)
    first = await harness.server(config=make_config(profile="first"))
    socket_path = first.socket_path
    out = _output()
    watcher = asyncio.ensure_future(commands.watch(commands.Target(socket_path=str(socket_path)), None, 30.0, out))
    try:
        for _ in range(100):
            if "connected" in out.err.getvalue():
                break
            await asyncio.sleep(0.05)
        await first.stop()
        second = LocalApiServer(ProtocolManager(make_config(profile="second")), socket_path=socket_path, health=False)
        await second.start()
        try:
            for _ in range(200):
                if out.err.getvalue().count("watch: connected") >= 2:
                    break
                await asyncio.sleep(0.05)
            assert out.err.getvalue().count("watch: connected") == 2, out.err.getvalue()
        finally:
            await second.stop()
    finally:
        watcher.cancel()
        await asyncio.gather(watcher, return_exceptions=True)


async def test_the_tcp_carrier_reads_the_token_file(harness):
    server = await harness.server(config=make_config(profile="over-tcp"), tcp=True)
    target = commands.Target(tcp_port=server.port, token_file=str(server.token_path))
    out = _output()
    assert await commands.state(target, [], out) == commands.PASS
    assert _lines(out)[0]["state"]["local_address"] == server.manager.local_address


def test_the_command_line_refuses_tcp_without_a_token_file(capsys):
    with pytest.raises(SystemExit):
        cli.main(["--tcp", "7800", "state"])
    assert "--tcp needs --token-file" in capsys.readouterr().err


def test_the_command_line_reports_a_missing_service_as_a_failure(tmp_path, capsys):
    status = cli.main(["--socket", str(tmp_path / "absent.sock"), "state"])
    assert status == commands.FAILED
    assert json.loads(capsys.readouterr().err.strip().splitlines()[-1])["error"]["message"]


def test_the_command_line_refuses_a_zero_count():
    with pytest.raises(SystemExit):
        cli.build_parser().parse_args(["ping", "off1x", "--count", "0"])
