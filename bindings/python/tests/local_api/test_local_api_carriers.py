"""The service starts every transport its configuration enables.

`ProtocolManager` builds a manager per enabled transport and leaves starting
them to its owner; the server is the owner. These tests pin which transports
the server starts, that one failing to start leaves the others running, and
that the command line refuses a transport flag the configuration cannot
honour instead of ignoring it.
"""

from __future__ import annotations

import asyncio
import json
import logging
import sys

import pytest
from websockets.asyncio.server import serve

from offline_protocol_sdk.ble_manager import BleManager
from offline_protocol_sdk.ble_peripheral import BlePeripheral
from offline_protocol_sdk.protocol_manager import ProtocolManager
from offline_protocol_sdk.transport_manager import TransportError

from .conftest import make_config
from .test_local_api_server import _cli_args


def _bluetooth_present(
    monkeypatch, started: list[str], *, central_fails=False, peripheral_fails=False, peripheral_hangs=False
):
    """Stands in for the radio: both roles report available, and start()
    records itself, raises the way a refused adapter does, or never returns
    the way a wedged backend does."""

    async def central_start(self):
        if central_fails:
            raise TransportError("no adapter")
        started.append("central")

    async def peripheral_start(self):
        if peripheral_hangs:
            await asyncio.Event().wait()
        if peripheral_fails:
            raise TransportError("org.bluez.Error.Failed: Maximum advertisements reached")
        started.append("peripheral")

    async def stop(self):
        return None

    monkeypatch.setattr(BleManager, "is_available", lambda self: True)
    monkeypatch.setattr(BlePeripheral, "is_available", lambda self: True)
    monkeypatch.setattr(BleManager, "start", central_start)
    monkeypatch.setattr(BlePeripheral, "start", peripheral_start)
    monkeypatch.setattr(BleManager, "stop", stop)
    monkeypatch.setattr(BlePeripheral, "stop", stop)


def _peer_stream_manager(profile: str, **config) -> ProtocolManager:
    manager = ProtocolManager(make_config(profile=profile, wifi_direct_enabled=True, **config))
    manager.peer_stream.configure(listen_host="127.0.0.1", listen_port=0)
    return manager


async def test_both_bluetooth_roles_start_with_the_server(harness, monkeypatch):
    started: list[str] = []
    _bluetooth_present(monkeypatch, started)
    manager = ProtocolManager(make_config(profile="ble-user", ble_enabled=True))
    server = await harness.server(manager=manager)
    client = await harness.client(server)
    assert (await client.hello("notes"))["state"] == "Running"
    assert started == ["central", "peripheral"]


async def test_a_refused_advertiser_leaves_the_peer_stream_running(harness, monkeypatch, caplog):
    """The failure this rule prevents: a radio another process holds taking
    the LAN path down with it."""
    started: list[str] = []
    _bluetooth_present(monkeypatch, started, peripheral_fails=True)
    manager = _peer_stream_manager("half-ble-user", ble_enabled=True)
    with caplog.at_level(logging.ERROR, logger="offline_protocol_sdk.local_api.server"):
        server = await harness.server(manager=manager)
    client = await harness.client(server)
    assert (await client.hello("notes"))["state"] == "Running"
    assert started == ["central"]
    assert manager.peer_stream.listen_port
    # The log is the one place the operator learns why: it names the
    # transport and carries the backend's own refusal.
    [record] = [r for r in caplog.records if r.levelno == logging.ERROR]
    assert "Bluetooth LE peripheral" in record.getMessage()
    assert "Maximum advertisements reached" in record.getMessage()


async def test_a_transport_that_never_starts_counts_as_failed(harness, monkeypatch, caplog):
    """A wedged backend must not keep the API socket from opening."""
    started: list[str] = []
    _bluetooth_present(monkeypatch, started, peripheral_hangs=True)
    manager = ProtocolManager(make_config(profile="wedged-ble-user", ble_enabled=True))
    with caplog.at_level(logging.ERROR, logger="offline_protocol_sdk.local_api.server"):
        server = await asyncio.wait_for(harness.server(manager=manager, carrier_start_timeout=0.2), 10)
    client = await harness.client(server)
    assert (await client.hello("notes"))["state"] == "Running"
    assert started == ["central"]
    assert any("Bluetooth LE peripheral" in r.getMessage() and "within" in r.getMessage() for r in caplog.records)


async def test_a_configured_relay_is_started_with_the_server(harness):
    """The relay `--relay` names is dialled and authenticated with the token."""
    first_frame: asyncio.Future[dict] = asyncio.get_running_loop().create_future()

    async def relay(websocket):
        frame = json.loads(await websocket.recv())
        if not first_frame.done():
            first_frame.set_result(frame)
        async for _ in websocket:
            pass

    async with serve(relay, "127.0.0.1", 0) as fake_relay:
        port = fake_relay.sockets[0].getsockname()[1]
        manager = ProtocolManager(make_config(profile="relay-user"))
        manager.internet.configure(server_url=f"ws://127.0.0.1:{port}", auto_reconnect=False)
        manager.internet.set_auth_token("relay-secret")
        await harness.server(manager=manager)
        frame = await asyncio.wait_for(first_frame, 5)
        assert frame == {"type": "Authenticate", "token": "relay-secret"}
        await manager.stop()


async def test_the_server_fails_when_no_transport_starts(harness, monkeypatch):
    started: list[str] = []
    _bluetooth_present(monkeypatch, started, central_fails=True, peripheral_fails=True)
    manager = ProtocolManager(make_config(profile="no-ble-user", ble_enabled=True))
    with pytest.raises(TransportError, match="no adapter"):
        await harness.server(manager=manager)
    assert not manager._running


async def test_an_unconfigured_relay_is_not_started(harness):
    """`internet_enabled` with no relay to dial has nothing to start; the
    server must come up all the same, as it always has."""
    manager = ProtocolManager(make_config(profile="no-relay-user"))
    assert manager.internet is not None and not manager.internet.is_available()
    server = await harness.server(manager=manager)
    client = await harness.client(server)
    assert (await client.hello("notes"))["state"] == "Running"


# -- the command line ------------------------------------------------------------


async def test_lan_switches_on_advertising_and_discovery(tmp_path):
    pytest.importorskip("zeroconf")
    cli, args = _cli_args(tmp_path, "--lan", "--listen", "127.0.0.1:0", wifi_direct_enabled=True)
    manager = cli.build_manager(args)
    try:
        assert manager.peer_stream.advertise is True
        assert manager.peer_stream.discover is True
    finally:
        await manager.close()


async def test_lan_without_the_extra_is_refused_naming_it(tmp_path, monkeypatch):
    """Refused here rather than by the transport: under the start rule a
    missing extra would be one logged error while Bluetooth carried the
    server, and the LAN the operator asked for would never appear."""
    monkeypatch.setitem(sys.modules, "zeroconf", None)
    cli, args = _cli_args(tmp_path, "--lan", wifi_direct_enabled=True)
    with pytest.raises(SystemExit, match=r"offline-protocol-sdk\[lan\]"):
        cli.build_manager(args)


async def test_without_lan_the_peer_stream_stays_off_the_lan(tmp_path):
    cli, args = _cli_args(tmp_path, "--listen", "127.0.0.1:0", wifi_direct_enabled=True)
    manager = cli.build_manager(args)
    try:
        assert manager.peer_stream.advertise is False
        assert manager.peer_stream.discover is False
    finally:
        await manager.close()


@pytest.mark.parametrize(
    "flags",
    [("--lan",), ("--listen", "127.0.0.1:7878"), ("--peer", "10.0.0.2:7878")],
)
async def test_a_peer_stream_flag_without_the_transport_is_refused(tmp_path, flags):
    """These were once ignored silently, which left a service that reached
    nobody with nothing in the log to say why."""
    cli, args = _cli_args(tmp_path, *flags)
    with pytest.raises(SystemExit, match="wifi_direct_enabled"):
        cli.build_manager(args)


async def test_a_malformed_peer_is_refused_with_the_flag_named(tmp_path):
    cli, args = _cli_args(tmp_path, "--peer", "10.0.0.2", wifi_direct_enabled=True)
    with pytest.raises(SystemExit, match="--peer"):
        cli.build_manager(args)


async def test_the_relay_flag_configures_the_internet_transport(tmp_path, monkeypatch):
    monkeypatch.setenv("OP_TEST_RELAY_TOKEN", "relay-secret")
    cli, args = _cli_args(
        tmp_path, "--relay", "wss://relay.example.test", "--relay-token-env", "OP_TEST_RELAY_TOKEN"
    )
    manager = cli.build_manager(args)
    try:
        assert manager.internet.is_available()
        assert manager.internet._server_url == "wss://relay.example.test"
        assert manager.internet._auth_token == "relay-secret"
    finally:
        await manager.close()


async def test_the_relay_token_is_optional(tmp_path, monkeypatch):
    monkeypatch.delenv("OFFLINE_PROTOCOL_RELAY_TOKEN", raising=False)
    cli, args = _cli_args(tmp_path, "--relay", "wss://relay.example.test")
    manager = cli.build_manager(args)
    try:
        assert manager.internet._auth_token is None
    finally:
        await manager.close()


async def test_the_relay_flag_without_internet_is_refused(tmp_path):
    cli, args = _cli_args(
        tmp_path, "--relay", "wss://relay.example.test", internet_enabled=False, wifi_direct_enabled=True
    )
    with pytest.raises(SystemExit, match="internet_enabled"):
        cli.build_manager(args)


@pytest.mark.parametrize("url", ["https://relay.example.test", "relay.example.test:443", "wss://", "wss:///path"])
async def test_a_relay_that_is_not_a_websocket_url_is_refused(tmp_path, url):
    """The transport retries a failed connect for as long as it runs, so a
    mis-typed relay would otherwise be a service that reports the relay
    running and never reaches it."""
    cli, args = _cli_args(tmp_path, "--relay", url)
    with pytest.raises(SystemExit, match="--relay takes a ws:// or wss:// URL"):
        cli.build_manager(args)


async def test_a_named_token_variable_that_is_unset_is_refused(tmp_path, monkeypatch):
    """Without a token the transport authenticates with the profile name."""
    monkeypatch.delenv("OP_TEST_UNSET_RELAY_TOKEN", raising=False)
    cli, args = _cli_args(
        tmp_path, "--relay", "wss://relay.example.test", "--relay-token-env", "OP_TEST_UNSET_RELAY_TOKEN"
    )
    with pytest.raises(SystemExit, match="OP_TEST_UNSET_RELAY_TOKEN is not set"):
        cli.build_manager(args)


@pytest.mark.parametrize(("url", "warned"), [("ws://relay.example.test", True), ("ws://127.0.0.1:9000", False)])
async def test_a_cleartext_relay_off_loopback_is_warned_about(tmp_path, caplog, url, warned):
    cli, args = _cli_args(tmp_path, "--relay", url)
    with caplog.at_level(logging.WARNING, logger="offline_protocol_sdk.local_api.cli"):
        manager = cli.build_manager(args)
    try:
        assert any("not TLS" in r.getMessage() for r in caplog.records) is warned
    finally:
        await manager.close()
