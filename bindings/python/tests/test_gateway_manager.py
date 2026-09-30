"""Tests for GatewayManager, the gateway-daemon client behind the
``reticulum`` slot.

The core is mocked; the socket is a real loopback socket to a fake daemon
that speaks the newline protocol and is driven by each test by hand, because
the attach order, the verdict correlation and what a dead connection owes
are what this manager promises, and a fake socket would test the fake.
"""

from __future__ import annotations

import asyncio
import base64
import collections
import json
import threading
import time
from typing import Any
from unittest.mock import MagicMock

import pytest

from offline_protocol_sdk import gateway_attach_policy as policy
from offline_protocol_sdk import gateway_manager as gm
from offline_protocol_sdk.gateway_manager import GatewayManager, parse_daemon_address
from offline_protocol_sdk.offline_protocol import GatewayAddressDeclaration, ReticulumMessage
from offline_protocol_sdk.transport_manager import TransportError, TransportState

OUR_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
PEER = "off1qyulwy7s5ezz20cy222zrw04rwds39uapqv9j8r0"
OTHER = "off1qzk5zj9m3f5c8k2v8x6q4n7r0t2w5y8b3d6g9h1j4"
CHALLENGE = bytes(range(32))
PUBLIC_KEY = bytes([1]) * 32
SIGNATURE = bytes([2]) * 64


# ---------------------------------------------------------------------------
# A fake daemon
# ---------------------------------------------------------------------------


class Conn:
    """One accepted connection, driven by the test."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.reader = reader
        self.writer = writer
        self.done = asyncio.Event()

    async def read_frame(self, timeout: float = 2.0) -> dict[str, Any]:
        line = await asyncio.wait_for(self.reader.readline(), timeout)
        if not line:
            raise EOFError("client closed")
        return json.loads(line)

    async def send(self, frame: dict[str, Any]) -> None:
        await self.send_raw(json.dumps(frame).encode() + b"\n")

    async def send_raw(self, data: bytes) -> None:
        self.writer.write(data)
        await self.writer.drain()

    async def closed_by_client(self, timeout: float = 2.0) -> bool:
        """True when the manager closed its end within the deadline."""
        try:
            return await asyncio.wait_for(self.reader.read(), timeout) == b""
        except (asyncio.IncompleteReadError, ConnectionError):
            return True
        except asyncio.TimeoutError:
            return False

    async def no_frame_for(self, seconds: float) -> bool:
        """True when nothing arrived (and the client did not close)."""
        try:
            data = await asyncio.wait_for(self.reader.readline(), seconds)
        except asyncio.TimeoutError:
            return True
        raise AssertionError(f"unexpected frame or close: {data!r}")

    def close(self) -> None:
        self.writer.close()
        self.done.set()


class FakeDaemon:
    def __init__(self) -> None:
        self.server: asyncio.base_events.Server | None = None
        self.port = 0
        self.accepted: asyncio.Queue[Conn] = asyncio.Queue()
        self.connections: list[Conn] = []

    async def start(self) -> None:
        self.server = await asyncio.start_server(self._on_connection, "127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]

    async def _on_connection(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        conn = Conn(reader, writer)
        self.connections.append(conn)
        await self.accepted.put(conn)
        await conn.done.wait()

    async def next_connection(self, timeout: float = 2.0) -> Conn:
        return await asyncio.wait_for(self.accepted.get(), timeout)

    async def no_connection_for(self, seconds: float) -> bool:
        try:
            await asyncio.wait_for(self.accepted.get(), seconds)
        except asyncio.TimeoutError:
            return True
        return False

    async def stop(self) -> None:
        for conn in self.connections:
            conn.close()
        if self.server is not None:
            self.server.close()
            try:
                await asyncio.wait_for(self.server.wait_closed(), 2.0)
            except Exception:
                pass


@pytest.fixture
async def daemon():
    fake = FakeDaemon()
    await fake.start()
    try:
        yield fake
    finally:
        await fake.stop()


# ---------------------------------------------------------------------------
# A fake core
# ---------------------------------------------------------------------------


def make_protocol(address: str | None = OUR_ADDRESS) -> MagicMock:
    p = MagicMock()
    p.calls: list[str] = []
    p.outbox: collections.deque[ReticulumMessage] = collections.deque()
    p.local_address = MagicMock(return_value=address)
    p.gateway_address_declaration = MagicMock(
        return_value=GatewayAddressDeclaration(
            address=OUR_ADDRESS, public_key=list(PUBLIC_KEY), signature=list(SIGNATURE)
        )
    )
    p.reticulum_get_next_message = MagicMock(
        side_effect=lambda: p.outbox.popleft() if p.outbox else None
    )
    p.reticulum_presence_watchlist = MagicMock(return_value=[])
    p.reticulum_gateway_capabilities = MagicMock(
        side_effect=lambda capabilities: p.calls.append("capabilities")
    )
    p.reticulum_address_declared = MagicMock(side_effect=lambda address: p.calls.append("declared"))
    p.reticulum_status_changed = MagicMock(
        side_effect=lambda is_connected: p.calls.append(f"status:{is_connected}")
    )
    return p


def queue_message(p: MagicMock, message_id: str, recipient: str = PEER, data: bytes = b"hello") -> None:
    p.outbox.append(
        ReticulumMessage(message_id=message_id, recipient_id=recipient, data=list(data), reply_to_msg=None)
    )


@pytest.fixture
def protocol():
    return make_protocol()


@pytest.fixture
def fast(monkeypatch):
    """Short clocks, so the tests run in milliseconds."""
    monkeypatch.setattr(gm, "RECONNECT_INITIAL_DELAY", 0.02)
    monkeypatch.setattr(gm, "RECONNECT_MAX_DELAY", 0.1)
    monkeypatch.setattr(gm, "MESSAGE_POLL_INTERVAL", 0.05)
    monkeypatch.setattr(gm, "DEFAULT_TICK_INTERVAL", 0.05)


def make_manager(protocol: MagicMock, daemon: FakeDaemon, **configure: Any) -> GatewayManager:
    manager = GatewayManager(protocol, device_id="test-device")
    manager.configure(daemon_address=f"127.0.0.1:{daemon.port}", **configure)
    return manager


@pytest.fixture
async def manager(protocol, daemon, fast):
    m = make_manager(protocol, daemon)
    await m.start()
    try:
        yield m
    finally:
        await m.stop()


async def until(predicate, timeout: float = 2.0) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() > deadline:
            raise AssertionError("condition not met in time")
        await asyncio.sleep(0.005)


async def attach(
    conn: Conn,
    *,
    echo: str = OUR_ADDRESS,
    capabilities: list[str] | None = None,
) -> dict[str, Any]:
    """Plays the daemon's half of the attach and returns the declaration."""
    identify = await conn.read_frame()
    assert identify["type"] == "Identify"
    await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
    declaration = await conn.read_frame()
    assert declaration["type"] == "DeclareAddress"
    await conn.send({"type": "AddressDeclared", "address": echo})
    await conn.send({"type": "Capabilities", "tokens": capabilities or ["gateway_v1"]})
    await conn.send({"type": "StatusUpdate", "status": "connected"})
    return declaration


async def attached(protocol: MagicMock, daemon: FakeDaemon) -> Conn:
    announced_before = protocol.calls.count("status:True")
    conn = await daemon.next_connection()
    await attach(conn)
    await until(lambda: protocol.calls.count("status:True") > announced_before)
    return conn


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


class TestConfiguration:
    def test_not_available_until_configured(self, protocol):
        manager = GatewayManager(protocol, device_id="d")
        assert not manager.is_available()
        manager.configure()
        assert manager.is_available()

    async def test_start_requires_configure(self, protocol):
        manager = GatewayManager(protocol, device_id="d")
        with pytest.raises(TransportError):
            await manager.start()
        assert manager.state is TransportState.STOPPED

    @pytest.mark.parametrize(
        "text, expected",
        [
            ("localhost:4242", ("localhost", 4242)),
            ("10.0.0.7:5000", ("10.0.0.7", 5000)),
            ("gateway.local", ("gateway.local", 4242)),
            ("[::1]:9000", ("::1", 9000)),
            ("[fe80::1]", ("fe80::1", 4242)),
            ("", ("localhost", 4242)),
        ],
    )
    def test_daemon_address_forms(self, text, expected):
        assert parse_daemon_address(text) == expected

    @pytest.mark.parametrize("bad", ["host:port", "host:0", "host:70000"])
    def test_bad_ports_are_refused(self, bad):
        with pytest.raises(ValueError):
            parse_daemon_address(bad)

    def test_a_non_local_daemon_is_warned_about(self, protocol):
        manager = GatewayManager(protocol, device_id="d")
        diagnostics: list[tuple[str, str]] = []
        manager.set_delegate(on_diagnostic=lambda level, msg, ctx: diagnostics.append((level, msg)))
        manager.configure(daemon_address="10.0.0.7:4242")
        assert any(level == "warning" and "unencrypted" in msg for level, msg in diagnostics)
        diagnostics.clear()
        manager.configure(daemon_address="127.0.0.1:4242")
        assert not any(level == "warning" for level, _ in diagnostics)


# ---------------------------------------------------------------------------
# Attach
# ---------------------------------------------------------------------------


class TestAttach:
    async def test_identify_carries_the_address_and_the_version(self, protocol, daemon, manager):
        conn = await daemon.next_connection()
        identify = await conn.read_frame()
        assert identify == {"type": "Identify", "device_id": OUR_ADDRESS, "protocol_version": 1}

    async def test_identify_falls_back_to_the_device_id_without_an_address(self, daemon, fast):
        protocol = make_protocol(address=None)
        manager = make_manager(protocol, daemon)
        await manager.start()
        try:
            conn = await daemon.next_connection()
            assert (await conn.read_frame())["device_id"] == "test-device"
        finally:
            await manager.stop()

    async def test_the_declaration_comes_from_the_core(self, protocol, daemon, manager):
        conn = await daemon.next_connection()
        declaration = await attach(conn)
        protocol.gateway_address_declaration.assert_called_once_with(list(CHALLENGE))
        assert declaration["address"] == OUR_ADDRESS
        assert base64.b64decode(declaration["public_key"]) == PUBLIC_KEY
        assert base64.b64decode(declaration["signature"]) == SIGNATURE

    async def test_the_carrier_is_announced_after_the_bind_and_after_capabilities(
        self, protocol, daemon, manager
    ):
        conn = await daemon.next_connection()
        await attach(conn, capabilities=["gateway_v1", "backbone_reticulum_v1"])
        await until(lambda: "status:True" in protocol.calls)
        assert protocol.calls == ["declared", "capabilities", "status:True"]
        protocol.reticulum_gateway_capabilities.assert_called_once_with(
            capabilities=["gateway_v1", "backbone_reticulum_v1"]
        )
        protocol.reticulum_address_declared.assert_called_once_with(address=OUR_ADDRESS)
        assert manager.bound
        assert manager.state is TransportState.RUNNING

    async def test_capabilities_are_bounded_before_reaching_the_core(self, protocol, daemon, manager):
        conn = await daemon.next_connection()
        await attach(conn, capabilities=[f"t{i}" for i in range(70)])
        await until(lambda: "status:True" in protocol.calls)
        (kwargs,) = [c.kwargs for c in protocol.reticulum_gateway_capabilities.call_args_list]
        assert len(kwargs["capabilities"]) == 64

    async def test_connected_before_bound_closes_the_connection(self, protocol, daemon, manager):
        conn = await daemon.next_connection()
        await conn.read_frame()  # Identify
        await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
        await conn.read_frame()  # DeclareAddress
        # Announced before the declaration is answered: not the contract.
        await conn.send({"type": "Capabilities", "tokens": ["gateway_v1"]})
        await conn.send({"type": "StatusUpdate", "status": "connected"})
        assert await conn.closed_by_client()
        assert "status:True" not in protocol.calls
        assert not manager.bound

    async def test_a_mismatched_echo_is_reported_and_closes(self, protocol, daemon, manager):
        conn = await daemon.next_connection()
        await attach(conn, echo=OTHER)
        assert await conn.closed_by_client()
        # The core owns the security warning, so it still hears the echo.
        protocol.reticulum_address_declared.assert_called_once_with(address=OTHER)
        assert "status:True" not in protocol.calls
        assert not manager.bound

    async def test_an_echo_with_no_local_address_closes(self, daemon, fast):
        protocol = make_protocol(address=None)
        manager = make_manager(protocol, daemon)
        await manager.start()
        try:
            conn = await daemon.next_connection()
            await attach(conn)
            assert await conn.closed_by_client()
            assert "status:True" not in protocol.calls
        finally:
            await manager.stop()

    async def test_an_over_long_echo_is_ignored_and_the_attach_times_out(
        self, protocol, daemon, manager, monkeypatch
    ):
        monkeypatch.setattr(policy, "ATTACH_TIMEOUT", 0.1)
        # Re-armed per connection; force a fresh one so the short deadline applies.
        conn = await daemon.next_connection()
        await conn.read_frame()
        await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
        await conn.read_frame()
        await conn.send({"type": "AddressDeclared", "address": "x" * 129})
        await asyncio.sleep(0.02)
        protocol.reticulum_address_declared.assert_not_called()
        assert not manager.bound

    async def test_an_address_error_is_reported_and_closes(self, protocol, daemon, manager):
        conn = await daemon.next_connection()
        await conn.read_frame()
        await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
        await conn.read_frame()
        await conn.send({"type": "AddressError", "reason": "signature does not verify"})
        assert await conn.closed_by_client()
        protocol.reticulum_address_declaration_refused.assert_called_once_with(
            reason="signature does not verify"
        )
        assert "status:True" not in protocol.calls

    async def test_a_challenge_of_the_wrong_size_closes_without_signing(
        self, protocol, daemon, manager
    ):
        conn = await daemon.next_connection()
        await conn.read_frame()
        await conn.send({"type": "Challenge", "challenge": base64.b64encode(bytes(16)).decode()})
        assert await conn.closed_by_client()
        protocol.gateway_address_declaration.assert_not_called()

    async def test_a_core_that_cannot_sign_closes(self, protocol, daemon, manager):
        protocol.gateway_address_declaration.side_effect = RuntimeError("MlsNotInitialized")
        conn = await daemon.next_connection()
        await conn.read_frame()
        await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
        assert await conn.closed_by_client()
        assert "status:True" not in protocol.calls

    async def test_a_silent_gateway_costs_one_attach_timeout(
        self, protocol, daemon, fast, monkeypatch
    ):
        monkeypatch.setattr(policy, "ATTACH_TIMEOUT", 0.1)
        manager = make_manager(protocol, daemon)
        await manager.start()
        try:
            conn = await daemon.next_connection()
            await conn.read_frame()  # Identify, then nothing
            assert await conn.closed_by_client(timeout=1.0)
            assert "status:False" in protocol.calls
            # And the ladder took over.
            second = await daemon.next_connection()
            assert (await second.read_frame())["type"] == "Identify"
        finally:
            await manager.stop()

    async def test_a_bound_but_never_announced_session_still_times_out(
        self, protocol, daemon, fast, monkeypatch
    ):
        monkeypatch.setattr(policy, "ATTACH_TIMEOUT", 0.1)
        manager = make_manager(protocol, daemon)
        await manager.start()
        try:
            conn = await daemon.next_connection()
            await conn.read_frame()
            await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
            await conn.read_frame()
            await conn.send({"type": "AddressDeclared", "address": OUR_ADDRESS})
            await until(lambda: manager.bound)
            # Bound, and the gateway never says `connected`.
            assert await conn.closed_by_client(timeout=1.0)
            assert not manager.bound
            assert "status:True" not in protocol.calls
        finally:
            await manager.stop()

    async def test_only_connected_completes_the_attach(self, protocol, daemon, manager):
        conn = await daemon.next_connection()
        await conn.read_frame()
        await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
        await conn.read_frame()
        await conn.send({"type": "AddressDeclared", "address": OUR_ADDRESS})
        await conn.send({"type": "Capabilities", "tokens": ["gateway_v1"]})
        await until(lambda: manager.bound)
        # Advisory statuses complete nothing.
        await conn.send({"type": "StatusUpdate", "status": "degraded"})
        await conn.send({"type": "StatusUpdate", "status": "disconnected"})
        await asyncio.sleep(0.05)
        assert "status:True" not in protocol.calls
        await conn.send({"type": "StatusUpdate", "status": "connected"})
        await until(lambda: "status:True" in protocol.calls)

    async def test_nothing_is_sent_before_the_session_is_announced(self, protocol, daemon, manager):
        queue_message(protocol, "early")
        manager.on_messages_available()
        conn = await daemon.next_connection()
        await conn.read_frame()  # Identify
        assert await conn.no_frame_for(0.15)
        await conn.send({"type": "Challenge", "challenge": base64.b64encode(CHALLENGE).decode()})
        await conn.read_frame()  # DeclareAddress
        await conn.send({"type": "AddressDeclared", "address": OUR_ADDRESS})
        await conn.send({"type": "Capabilities", "tokens": []})
        await conn.send({"type": "StatusUpdate", "status": "connected"})
        # The announce itself drains what queued during the attach.
        frame = await conn.read_frame()
        assert frame["type"] == "SendMessage" and frame["message_id"] == "early"


# ---------------------------------------------------------------------------
# Submit and verdict
# ---------------------------------------------------------------------------


class TestSubmitAndVerdict:
    async def test_the_frame_shape(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        protocol.outbox.append(
            ReticulumMessage(message_id="m1", recipient_id=PEER, data=list(b"\x00\x01"), reply_to_msg="parent")
        )
        manager.on_messages_available()
        frame = await conn.read_frame()
        assert frame == {
            "type": "SendMessage",
            "recipient": PEER,
            "content": base64.b64encode(b"\x00\x01").decode(),
            "encoding": "base64",
            "message_id": "m1",
            "reply_to_msg": "parent",
        }
        # The write is not the outcome.
        protocol.reticulum_confirm_sent.assert_not_called()

    async def test_verdicts_are_correlated_by_id_not_order(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1", recipient=PEER)
        queue_message(protocol, "m2", recipient=OTHER)
        manager.on_messages_available()
        first = await conn.read_frame()
        second = await conn.read_frame()
        assert {first["message_id"], second["message_id"]} == {"m1", "m2"}
        # Answered in the other order.
        await conn.send({"type": "MessageSent", "message_id": "m2", "recipient": OTHER})
        await conn.send(
            {
                "type": "DeliveryError",
                "message_id": "m1",
                "recipient": PEER,
                "reason": "recipient_unreachable: not attached here",
            }
        )
        await until(lambda: protocol.reticulum_send_failed_with_reason.called)
        protocol.reticulum_confirm_sent.assert_called_once_with(message_id="m2")
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason="recipient_unreachable: not attached here"
        )
        protocol.reticulum_peer_presence.assert_called_once_with(
            peer_id=PEER, online=False, last_seen_ms=None
        )

    async def test_a_duplicate_verdict_settles_nothing_twice(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        manager.on_messages_available()
        await conn.read_frame()
        await conn.send({"type": "MessageSent", "message_id": "m1"})
        await conn.send({"type": "MessageSent", "message_id": "m1"})
        await conn.send({"type": "DeliveryError", "message_id": "m1", "reason": "late"})
        await until(lambda: protocol.reticulum_confirm_sent.called)
        await asyncio.sleep(0.05)
        protocol.reticulum_confirm_sent.assert_called_once_with(message_id="m1")
        protocol.reticulum_send_failed_with_reason.assert_not_called()

    async def test_an_unsolicited_verdict_is_ignored(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        await conn.send({"type": "MessageSent", "message_id": "never-sent"})
        await conn.send({"type": "DeliveryError", "message_id": "never-sent", "reason": "x"})
        await conn.send({"type": "MessageSent"})
        await asyncio.sleep(0.05)
        protocol.reticulum_confirm_sent.assert_not_called()
        protocol.reticulum_send_failed_with_reason.assert_not_called()

    async def test_a_non_unreachable_error_is_a_plain_failure(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        manager.on_messages_available()
        await conn.read_frame()
        await conn.send(
            {"type": "DeliveryError", "message_id": "m1", "recipient": PEER, "reason": "budget_exceeded"}
        )
        await until(lambda: protocol.reticulum_send_failed_with_reason.called)
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason="budget_exceeded"
        )
        protocol.reticulum_peer_presence.assert_not_called()

    async def test_self_is_never_watched(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1", recipient=OUR_ADDRESS)
        manager.on_messages_available()
        await conn.read_frame()
        await conn.send(
            {
                "type": "DeliveryError",
                "message_id": "m1",
                "recipient": OUR_ADDRESS,
                "reason": "recipient_unreachable",
            }
        )
        await until(lambda: protocol.reticulum_send_failed_with_reason.called)
        protocol.reticulum_peer_presence.assert_not_called()


class TestStoredAndPushed:
    """The first client on this carrier to read the two flags."""

    async def _one_verdict(self, protocol, daemon, manager, verdict: dict[str, Any]) -> None:
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1", recipient=PEER)
        manager.on_messages_available()
        await conn.read_frame()
        await conn.send({"message_id": "m1", "recipient": PEER, **verdict})
        await until(
            lambda: protocol.reticulum_send_failed_with_reason.called
            or protocol.reticulum_confirm_sent.called
        )

    async def test_stored_delivery_error_is_relay_stored(self, protocol, daemon, manager):
        await self._one_verdict(
            protocol, daemon, manager,
            {"type": "DeliveryError", "reason": "recipient_unreachable: asleep", "stored": True},
        )
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason="relay_stored"
        )
        protocol.reticulum_peer_presence.assert_called_once_with(
            peer_id=PEER, online=False, last_seen_ms=None
        )

    async def test_pushed_message_sent_is_relay_pushed(self, protocol, daemon, manager):
        await self._one_verdict(protocol, daemon, manager, {"type": "MessageSent", "pushed": True})
        protocol.reticulum_confirm_sent.assert_not_called()
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason="relay_pushed"
        )
        protocol.reticulum_peer_presence.assert_called_once_with(
            peer_id=PEER, online=False, last_seen_ms=None
        )

    async def test_pushed_and_stored_is_relay_pushed_stored(self, protocol, daemon, manager):
        await self._one_verdict(
            protocol, daemon, manager, {"type": "MessageSent", "pushed": True, "stored": True}
        )
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason="relay_pushed_stored"
        )

    async def test_absent_flags_read_as_false(self, protocol, daemon, manager):
        await self._one_verdict(protocol, daemon, manager, {"type": "MessageSent", "stored": False})
        protocol.reticulum_confirm_sent.assert_called_once_with(message_id="m1")
        protocol.reticulum_send_failed_with_reason.assert_not_called()


class TestInFlight:
    async def test_at_most_eight_frames_are_unanswered(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        for i in range(10):
            queue_message(protocol, f"m{i}")
        manager.on_messages_available()
        sent = [await conn.read_frame() for _ in range(8)]
        assert len({f["message_id"] for f in sent}) == 8
        assert await conn.no_frame_for(0.2)
        # A verdict frees a slot, and the next frame goes out on it.
        await conn.send({"type": "MessageSent", "message_id": sent[0]["message_id"]})
        ninth = await conn.read_frame()
        assert ninth["message_id"] not in {f["message_id"] for f in sent}

    async def test_an_id_already_in_flight_is_not_sent_twice(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        queue_message(protocol, "m1")  # the core re-queued it after its own ack timeout
        manager.on_messages_available()
        assert (await conn.read_frame())["message_id"] == "m1"
        assert await conn.no_frame_for(0.2)
        await conn.send({"type": "MessageSent", "message_id": "m1"})
        await until(lambda: protocol.reticulum_confirm_sent.called)
        # Settled, so a re-queue is sent again.
        queue_message(protocol, "m1")
        manager.on_messages_available()
        assert (await conn.read_frame())["message_id"] == "m1"

    async def test_a_silent_gateway_is_failed_at_the_verdict_timeout(
        self, protocol, daemon, manager, monkeypatch
    ):
        monkeypatch.setattr(policy, "VERDICT_TIMEOUT", 0.05)
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        manager.on_messages_available()
        await conn.read_frame()
        await until(lambda: protocol.reticulum_send_failed_with_reason.called, timeout=2.0)
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason=policy.GATEWAY_SILENT
        )
        # A verdict that arrives after the sweep settles nothing.
        await conn.send({"type": "MessageSent", "message_id": "m1"})
        await asyncio.sleep(0.05)
        protocol.reticulum_confirm_sent.assert_not_called()

    async def test_an_id_the_gateway_would_refuse_is_settled_locally(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "has space")
        manager.on_messages_available()
        await until(lambda: protocol.reticulum_send_failed_with_reason.called)
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="has space", reason="Unserializable frame"
        )
        assert await conn.no_frame_for(0.1)


# ---------------------------------------------------------------------------
# Losing the connection
# ---------------------------------------------------------------------------


class TestClose:
    async def test_a_dropped_connection_fails_what_it_carried_and_reconnects(
        self, protocol, daemon, manager
    ):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        manager.on_messages_available()
        await conn.read_frame()
        conn.close()
        await until(lambda: protocol.reticulum_send_failed_with_reason.called)
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason="Connection lost"
        )
        await until(lambda: "status:False" in protocol.calls)
        assert not manager.bound
        assert manager.get_metrics()["in_flight"] == 0
        # The ladder brings a fresh session, attached from scratch.
        second = await daemon.next_connection()
        await attach(second)
        await until(lambda: protocol.calls.count("status:True") == 2)
        assert manager.bound

    async def test_the_ladder_resets_only_on_a_bound_announced_session(
        self, protocol, daemon, manager
    ):
        first = await daemon.next_connection()
        first.close()
        second = await daemon.next_connection()
        await second.read_frame()  # a TCP open is not a reset
        second.close()
        await until(lambda: manager._reconnect_attempts == 2)
        assert manager._current_reconnect_delay > gm.RECONNECT_INITIAL_DELAY
        third = await daemon.next_connection()
        await attach(third)
        await until(lambda: "status:True" in protocol.calls)
        assert manager._reconnect_attempts == 0
        assert manager._current_reconnect_delay == gm.RECONNECT_INITIAL_DELAY

    async def test_max_reconnect_attempts_stops_the_transport(self, protocol, daemon, fast):
        manager = make_manager(protocol, daemon, max_reconnect_attempts=2)
        await manager.start()
        try:
            for _ in range(3):
                conn = await daemon.next_connection()
                conn.close()
            await until(lambda: manager.state is TransportState.STOPPED)
            assert await daemon.no_connection_for(0.2)
        finally:
            await manager.stop()

    async def test_stop_fails_in_flight_and_tells_the_core(self, protocol, daemon, fast):
        manager = make_manager(protocol, daemon)
        await manager.start()
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        manager.on_messages_available()
        await conn.read_frame()
        await manager.stop()
        protocol.reticulum_send_failed_with_reason.assert_called_once_with(
            message_id="m1", reason="Disconnected"
        )
        assert protocol.calls[-1] == "status:False"
        assert manager.state is TransportState.STOPPED
        assert not manager.bound
        assert await conn.closed_by_client()
        assert await daemon.no_connection_for(0.15)

    async def test_stop_is_idempotent_and_start_again_works(self, protocol, daemon, fast):
        manager = make_manager(protocol, daemon)
        await manager.start()
        await attached(protocol, daemon)
        await manager.stop()
        await manager.stop()
        await manager.start()
        try:
            await attached(protocol, daemon)
            assert protocol.calls.count("status:True") == 2
        finally:
            await manager.stop()

    async def test_an_over_long_line_closes_the_connection(
        self, protocol, daemon, fast, monkeypatch
    ):
        monkeypatch.setattr(policy, "MAX_LINE_BYTES", 1024)
        manager = make_manager(protocol, daemon)
        await manager.start()
        try:
            conn = await attached(protocol, daemon)
            await conn.send_raw(b"x" * 2048)
            assert await conn.closed_by_client()
            await until(lambda: "status:False" in protocol.calls)
        finally:
            await manager.stop()

    async def test_repeated_write_failures_reconnect(self, protocol, daemon, manager, monkeypatch):
        conn = await attached(protocol, daemon)

        async def failing_write(text, generation):
            return False

        monkeypatch.setattr(manager, "_write_line", failing_write)
        for i in range(gm.MAX_CONSECUTIVE_FAILURES):
            queue_message(protocol, f"m{i}")
        manager.on_messages_available()
        await until(
            lambda: protocol.reticulum_send_failed_with_reason.call_count == gm.MAX_CONSECUTIVE_FAILURES
        )
        for call in protocol.reticulum_send_failed_with_reason.call_args_list:
            assert call.kwargs["reason"] == "Write failed"
        assert await conn.closed_by_client()


# ---------------------------------------------------------------------------
# Deliver, presence, pause
# ---------------------------------------------------------------------------


class TestInbound:
    async def test_a_delivered_frame_is_decoded_and_handed_to_the_core(
        self, protocol, daemon, manager
    ):
        conn = await attached(protocol, daemon)
        await conn.send(
            {
                "type": "MessageReceived",
                "sender": PEER,
                "content": base64.b64encode(b"hi").decode(),
                "encoding": "base64",
            }
        )
        await until(lambda: protocol.reticulum_message_received.called)
        protocol.reticulum_message_received.assert_called_once_with(sender_id=PEER, data=list(b"hi"))

    async def test_text_content_without_an_encoding_is_utf8(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        await conn.send({"type": "MessageReceived", "sender": PEER, "content": "plain"})
        await until(lambda: protocol.reticulum_message_received.called)
        protocol.reticulum_message_received.assert_called_once_with(
            sender_id=PEER, data=list(b"plain")
        )

    async def test_malformed_frames_are_skipped_without_closing(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        await conn.send_raw(b"not json\n")
        await conn.send({"type": "MessageReceived", "content": "no sender"})
        await conn.send({"type": "MessageReceived", "sender": PEER, "content": "@@", "encoding": "base64"})
        await conn.send({"type": "Whatever"})
        await conn.send({"notype": 1})
        await conn.send({"type": "PresenceStatus", "peer": PEER})
        await asyncio.sleep(0.05)
        protocol.reticulum_message_received.assert_not_called()
        protocol.reticulum_peer_presence.assert_not_called()
        assert manager.bound
        # Still speaking.
        await conn.send({"type": "MessageReceived", "sender": PEER, "content": "ok"})
        await until(lambda: protocol.reticulum_message_received.called)

    async def test_presence_answers_reach_the_core(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        await conn.send({"type": "PresenceStatus", "peer": PEER, "online": True, "last_seen_ms": 42})
        await until(lambda: protocol.reticulum_peer_presence.called)
        protocol.reticulum_peer_presence.assert_called_once_with(
            peer_id=PEER, online=True, last_seen_ms=42
        )

    async def test_the_watch_asks_about_the_core_watchlist_and_unreachable_recipients(
        self, protocol, daemon, manager
    ):
        conn = await attached(protocol, daemon)
        protocol.reticulum_presence_watchlist.return_value = [OTHER, OUR_ADDRESS, "test-device", ""]
        queue_message(protocol, "m1", recipient=PEER)
        manager.on_messages_available()
        await conn.read_frame()
        await conn.send(
            {"type": "DeliveryError", "message_id": "m1", "recipient": PEER, "reason": "recipient_unreachable"}
        )
        frame = await conn.read_frame()
        assert frame["type"] == "CheckPresence"
        assert set(frame["peers"]) == {PEER, OTHER}
        # An online answer un-watches the peer; the core's list keeps OTHER.
        await conn.send({"type": "PresenceStatus", "peer": PEER, "online": True})
        await asyncio.sleep(0.01)
        frame = await conn.read_frame()
        assert frame["peers"] == [OTHER]

    async def test_no_presence_frame_when_there_is_nobody_to_ask(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        assert await conn.no_frame_for(0.2)


class TestPause:
    async def test_a_paused_manager_does_not_drain_on_wake(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        await manager.pause()
        queue_message(protocol, "m1")
        manager.on_messages_available()
        assert await conn.no_frame_for(0.2)
        await manager.resume()
        assert (await conn.read_frame())["message_id"] == "m1"

    async def test_a_paused_manager_stops_asking_about_presence(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        protocol.reticulum_presence_watchlist.return_value = [OTHER]
        assert (await conn.read_frame())["type"] == "CheckPresence"
        await manager.pause()
        await asyncio.sleep(0.01)
        assert await conn.no_frame_for(0.2)

    async def test_a_verdict_while_paused_does_not_drain(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        manager.on_messages_available()
        await conn.read_frame()
        await manager.pause()
        queue_message(protocol, "m2")
        await conn.send({"type": "MessageSent", "message_id": "m1"})
        await until(lambda: protocol.reticulum_confirm_sent.called)
        assert await conn.no_frame_for(0.2)


class TestWake:
    async def test_a_wake_from_another_thread_drains_on_the_loop(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        thread = threading.Thread(target=manager.on_messages_available)
        thread.start()
        thread.join()
        assert (await conn.read_frame())["message_id"] == "m1"

    async def test_wakes_that_land_mid_drain_run_another_pass(self, protocol, daemon, manager):
        conn = await attached(protocol, daemon)
        queue_message(protocol, "m1")
        manager.on_messages_available()
        manager.on_messages_available()
        queue_message(protocol, "m2")
        manager.on_messages_available()
        ids = {(await conn.read_frame())["message_id"], (await conn.read_frame())["message_id"]}
        assert ids == {"m1", "m2"}


# ---------------------------------------------------------------------------
# ProtocolManager wiring
# ---------------------------------------------------------------------------


class TestProtocolManagerWiring:
    def _config(self, **overrides):
        from offline_protocol_sdk.offline_protocol import OverflowPolicy, ProtocolConfig

        defaults = dict(
            app_id="test-app",
            profile="test-user",
            ble_enabled=False,
            wifi_direct_enabled=False,
            internet_enabled=True,
            reticulum_enabled=True,
            nostr_enabled=False,
            prefer_online=True,
            initial_ttl=3,
            encryption_enabled=False,
            auto_key_exchange=False,
            store_pending=True,
            require_encryption=False,
            max_pending_per_peer=100,
            max_pending_global=1000,
            pending_ttl_ms=60000,
            overflow_policy=OverflowPolicy.DROP_OLDEST,
        )
        defaults.update(overrides)
        return ProtocolConfig(**defaults)

    def test_the_gateway_exists_only_when_the_slot_is_enabled(self):
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(self._config(reticulum_enabled=True))
        assert isinstance(pm.gateway, GatewayManager)
        assert pm.gateway.state is TransportState.STOPPED, "started by the caller, after start()"
        assert not pm.gateway.is_available(), "configured by the caller"
        assert ProtocolManager(self._config(reticulum_enabled=False)).gateway is None

    async def test_the_core_wake_reaches_the_manager_and_stop_stops_it(self):
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(self._config())
        await pm.start()
        try:
            assert pm._reticulum_cb is not None
            pm.gateway.on_messages_available = MagicMock()
            pm._reticulum_cb.on_messages_available()
            pm.gateway.on_messages_available.assert_called_once()
            pm.gateway.stop = MagicMock(side_effect=pm.gateway.stop)
        finally:
            await pm.stop()
        pm.gateway.stop.assert_called_once()
