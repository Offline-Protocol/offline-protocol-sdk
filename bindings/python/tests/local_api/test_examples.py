"""The shipped client examples, run against in-process servers, so a change
to the wire that breaks either is caught here rather than by a reader.

The Python example is imported as a module and driven function by function,
then run as a subprocess the way a reader runs it. The Node example runs
end to end when ``node`` is on the path, over the TCP carrier with a token
file, because Node's built-in WebSocket dials only ``ws:`` and ``wss:``."""

from __future__ import annotations

import asyncio
import importlib.util
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from offline_protocol_sdk.protocol_manager import ProtocolManager

from .conftest import make_config

PYTHON_EXAMPLE = Path(__file__).resolve().parents[2] / "examples" / "local_api_client.py"
NODE_EXAMPLE = Path(__file__).resolve().parents[4] / "examples" / "local-api" / "client.mjs"


def load_example():
    spec = importlib.util.spec_from_file_location("local_api_client", PYTHON_EXAMPLE)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


async def until(predicate, timeout: float = 15.0) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not predicate():
        if loop.time() > deadline:
            raise AssertionError("condition not met in time")
        await asyncio.sleep(0.05)


async def finish(process, timeout: float = 60.0) -> tuple[bytes, bytes]:
    """``communicate`` with a deadline that does not leave the child running."""
    try:
        return await asyncio.wait_for(process.communicate(), timeout)
    except (TimeoutError, asyncio.TimeoutError):
        process.kill()
        await process.wait()
        raise


def output_lines(stdout: bytes) -> list[dict]:
    return [json.loads(line) for line in stdout.decode().splitlines() if line.strip()]


async def two_servers(harness, *, tcp_a: bool = False):
    """Two engines with the data layer on, over loopback streams: B keeps a
    stream to A. Server A serves TCP with a token when ``tcp_a``."""
    config_a = make_config(profile="alice", wifi_direct_enabled=True, internet_enabled=False, data_enabled=True)
    manager_a = ProtocolManager(config_a)
    manager_a.peer_stream.configure(listen_host="127.0.0.1", listen_port=0)
    server_a = await harness.server(manager=manager_a, tcp=tcp_a)
    port_a = manager_a.peer_stream.listen_port
    config_b = make_config(profile="bob", wifi_direct_enabled=True, internet_enabled=False, data_enabled=True)
    manager_b = ProtocolManager(config_b)
    manager_b.peer_stream.configure(listen_host="127.0.0.1", listen_port=0, peers=[f"127.0.0.1:{port_a}"])
    server_b = await harness.server(manager=manager_b)
    await until(lambda: manager_a.local_address in manager_b.peer_stream.connected_peers())
    await until(lambda: manager_b.local_address in manager_a.peer_stream.connected_peers())
    return server_a, server_b


async def test_the_python_example_round_trips_a_message_and_a_document(harness):
    example = load_example()
    server_a, server_b = await two_servers(harness)
    receiver, _ = await example.open_client(str(server_b.socket_path), None, None)
    sender, _ = await example.open_client(str(server_a.socket_path), None, None)
    try:
        greeting = await example.hello(receiver, "notes", None)
        assert greeting["api_version"] == 1
        assert greeting["local_address"] == server_b.manager.local_address
        await example.hello(sender, "notes", None)

        delivered = await example.send_and_confirm(sender, server_b.manager.local_address, "from the example")
        received = await receiver.next_event("message_received", message_id=delivered["message_id"])
        assert received["content"] == "from the example"
        assert received["app_id"] == "notes"

        document = await example.edit_document(sender, "notes-1", "todo", "edited_by", "notes")
        assert document == {"fields": {"edited_by": "notes"}}
        # A second edit through the same path: create_doc is a no-op on an
        # existing document, and the read shows the newer value.
        document = await example.edit_document(sender, "notes-1", "todo", "edited_by", "notes again")
        assert document == {"fields": {"edited_by": "notes again"}}

        one_shot = await example.one_shot(str(server_a.socket_path), None, None, "notes", "local_address")
        assert one_shot == server_a.manager.local_address
    finally:
        await sender.close()
        await receiver.close()


async def test_the_python_example_reports_an_engine_refusal_by_variant(harness):
    example = load_example()
    server = await harness.server(config=make_config(profile="solo", data_enabled=False))
    client, _ = await example.open_client(str(server.socket_path), None, None)
    try:
        await example.hello(client, "notes", None)
        with pytest.raises(example.RpcFailure) as err:
            await example.edit_document(client, "notes-1", "todo", "k", "v")
        assert err.value.variant == "DataDisabled"
        assert err.value.code <= -32000
    finally:
        await client.close()


async def test_the_python_example_runs_from_the_shell(harness):
    """The example as a reader runs it: a subprocess against server A, sending
    to server B, whose client (this test) receives the message."""
    example = load_example()
    server_a, server_b = await two_servers(harness)
    receiver, _ = await example.open_client(str(server_b.socket_path), None, None)
    try:
        await example.hello(receiver, "notes", None)
        env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
        process = await asyncio.create_subprocess_exec(
            sys.executable,
            str(PYTHON_EXAMPLE),
            "--socket",
            str(server_a.socket_path),
            "--app-id",
            "notes",
            "--to",
            server_b.manager.local_address,
            "--content",
            "from the shell",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        stdout, stderr = await finish(process)
        assert process.returncode == 0, stderr.decode()
        lines = output_lines(stdout)
        keys = [next(iter(line)) for line in lines]
        assert keys == ["hello", "delivered", "document", "one_shot"]
        assert lines[0]["hello"]["local_address"] == server_a.manager.local_address
        assert lines[2]["document"] == {"fields": {"edited_by": "notes"}}
        assert lines[3]["one_shot"] == server_a.manager.local_address
        received = await receiver.next_event("message_received", message_id=lines[1]["delivered"]["message_id"])
        assert received["content"] == "from the shell"
    finally:
        await receiver.close()


@pytest.mark.parametrize("language", ["python", "node"])
async def test_the_examples_wait_past_undeliverable_for_the_delivery(harness, language):
    """A recipient that is away makes the engine report the message
    undeliverable, keep it, and deliver it later. Both examples print each
    report and keep waiting, and the server keeps routing the message's
    events to the sender alone, so the later delivery reaches them."""
    node = shutil.which("node")
    if language == "node" and node is None:
        pytest.skip("node is not on the path")
    example = load_example()
    away = await harness.server(config=make_config(profile="away", wifi_direct_enabled=True, internet_enabled=False))
    config = make_config(profile="alice", wifi_direct_enabled=True, internet_enabled=False, data_enabled=True)
    server = await harness.server(config=config, tcp=language == "node")
    recipient = away.manager.local_address
    observer, observer_token = await example.open_client(
        str(server.socket_path) if server.socket_path else None,
        server.port,
        str(server.token_path) if server.token_path else None,
    )
    stranger, stranger_token = await example.open_client(
        str(server.socket_path) if server.socket_path else None,
        server.port,
        str(server.token_path) if server.token_path else None,
    )
    try:
        # Another client of the same application sees the send's own events,
        # which is how this test learns the id the example was handed.
        await example.hello(observer, "notes", observer_token)
        await example.hello(stranger, "other", stranger_token)
        if language == "python":
            command = [sys.executable, str(PYTHON_EXAMPLE), "--socket", str(server.socket_path)]
        else:
            command = [node, str(NODE_EXAMPLE), "--port", str(server.port), "--token-file", str(server.token_path)]
        process = await asyncio.create_subprocess_exec(
            *command,
            "--app-id",
            "notes",
            "--to",
            recipient,
            "--content",
            "to someone away",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        named = await observer.next_event(
            ("message_sent", "message_deferred", "message_retrying"), timeout=15.0, recipient=recipient
        )
        message_id = named["message_id"]
        undeliverable = {
            "type": "message_undeliverable",
            "message_id": message_id,
            "recipient": recipient,
            "reason": "recipient_unreachable",
        }
        server._route(undeliverable)
        server._route(dict(undeliverable))
        server._route(
            {"type": "message_delivered", "message_id": message_id, "latency_ms": 7, "hop_count": 0, "transport": "ble"}
        )
        stdout, stderr = await finish(process)
        assert process.returncode == 0, stderr.decode()
        lines = output_lines(stdout)
        assert [next(iter(line)) for line in lines] == [
            "hello",
            "undeliverable",
            "undeliverable",
            "delivered",
            "document",
            "one_shot",
        ]
        assert lines[3]["delivered"]["message_id"] == message_id
        # The other application heard none of it.
        with pytest.raises((TimeoutError, asyncio.TimeoutError)):
            await stranger.next_event(
                ("message_undeliverable", "message_delivered"), timeout=0.5, message_id=message_id
            )
    finally:
        await observer.close()
        await stranger.close()


def test_the_node_example_parses():
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is not on the path")
    subprocess.run([node, "--check", str(NODE_EXAMPLE)], check=True)


async def test_the_node_example_runs_end_to_end_over_tcp(harness):
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is not on the path")
    example = load_example()
    server_a, server_b = await two_servers(harness, tcp_a=True)
    assert server_a.token_path is not None and server_a.port is not None
    receiver, _ = await example.open_client(str(server_b.socket_path), None, None)
    try:
        await example.hello(receiver, "notes", None)
        process = await asyncio.create_subprocess_exec(
            node,
            str(NODE_EXAMPLE),
            "--port",
            str(server_a.port),
            "--token-file",
            str(server_a.token_path),
            "--app-id",
            "notes",
            "--to",
            server_b.manager.local_address,
            "--content",
            "from Node",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        stdout, stderr = await finish(process)
        assert process.returncode == 0, stderr.decode()
        lines = output_lines(stdout)
        keys = [next(iter(line)) for line in lines]
        assert keys == ["hello", "delivered", "document", "one_shot"]
        assert lines[0]["hello"]["local_address"] == server_a.manager.local_address
        assert lines[2]["document"] == {"fields": {"edited_by": "notes"}}
        assert lines[3]["one_shot"] == server_a.manager.local_address
        received = await receiver.next_event("message_received", message_id=lines[1]["delivered"]["message_id"])
        assert received["content"] == "from Node"
    finally:
        await receiver.close()


async def test_the_python_example_fails_instead_of_hanging_when_the_server_closes(tmp_path):
    """A server that reads one frame and closes without answering: the pending
    ``hello`` must fail with ``ConnectionError``, not wait forever."""
    from websockets.asyncio.server import serve

    example = load_example()

    async def read_one_then_close(websocket):
        await websocket.recv()
        await websocket.close(1011, "gone")

    token_file = tmp_path / "token"
    token_file.write_text("not-checked\n", encoding="ascii")
    async with serve(read_one_then_close, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        client, token = await example.open_client(None, port, str(token_file))
        try:
            with pytest.raises(ConnectionError):
                await asyncio.wait_for(example.hello(client, "notes", token), 5)
        finally:
            await client.close()


async def test_the_examples_report_a_connection_failure_on_one_line(tmp_path):
    """No traceback and no unsettled await: a socket that is not there, and a
    port nothing listens on, each cost one JSON line on stderr and exit 1."""
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        str(PYTHON_EXAMPLE),
        "--socket",
        str(tmp_path / "nowhere.sock"),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    stdout, stderr = await finish(process)
    assert process.returncode == 1
    assert stdout == b""
    (line,) = stderr.decode().splitlines()
    assert "message" in json.loads(line)["error"]

    node = shutil.which("node")
    if node is None:
        pytest.skip("node is not on the path")
    token_file = tmp_path / "token"
    token_file.write_text("unused\n", encoding="ascii")
    process = await asyncio.create_subprocess_exec(
        node,
        str(NODE_EXAMPLE),
        "--port",
        "1",
        "--token-file",
        str(token_file),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout, stderr = await finish(process)
    assert process.returncode == 1, stderr.decode()
    assert stdout == b""
    (line,) = stderr.decode().splitlines()
    assert "message" in json.loads(line)["error"]
