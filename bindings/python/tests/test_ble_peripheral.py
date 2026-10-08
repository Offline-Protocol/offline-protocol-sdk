"""Tests for BlePeripheral — BLE GATT server (peripheral role).

All tests mock the bless server and OfflineProtocol so they run without
actual Bluetooth hardware.
"""

from __future__ import annotations

import asyncio
import inspect
import sys
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from offline_protocol_sdk.ble_peripheral import BlePeripheral
from offline_protocol_sdk.transport_manager import TransportError, TransportState

# What the core would answer for this peripheral's identity. The assertion is
# opaque to the binding: 96 bytes of key and signature and nothing after,
# because the peripheral signs nothing (see `_resolve_identity`).
LOCAL_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
ASSERTION = bytes(range(96))


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def mock_protocol():
    """A mock OfflineProtocol with BLE methods stubbed."""
    p = MagicMock()
    p.ble_fragment_received = MagicMock()
    p.ble_get_next_fragment = MagicMock(return_value=None)
    p.ble_return_fragment = MagicMock()
    p.ble_peer_discovered = MagicMock()
    p.ble_peer_lost = MagicMock()
    p.ble_status_changed = MagicMock()
    p.local_address = MagicMock(return_value=LOCAL_ADDRESS)
    p.identity_assertion = MagicMock(return_value=list(ASSERTION))
    return p


@pytest.fixture
def peripheral(mock_protocol):
    """A BlePeripheral instance (not started)."""
    return BlePeripheral(mock_protocol, device_id="coordinator")


# ---------------------------------------------------------------------------
# Basic state
# ---------------------------------------------------------------------------

class TestInitialState:
    def test_initial_state_is_stopped(self, peripheral):
        assert peripheral.state == TransportState.STOPPED

    def test_is_available_on_supported_platform(self, peripheral):
        # Should be True on macOS and Linux
        assert peripheral.is_available() == (sys.platform in ("darwin", "linux"))

    def test_get_metrics_defaults(self, peripheral):
        m = peripheral.get_metrics()
        assert m["bytes_sent"] == 0
        assert m["bytes_received"] == 0
        assert m["fragments_sent"] == 0
        assert m["fragments_received"] == 0
        assert m["connected_centrals"] == 0
        assert m["is_advertising"] is False


# ---------------------------------------------------------------------------
# Read handler
# ---------------------------------------------------------------------------

class TestReadHandler:
    def test_read_device_id_is_the_address_not_the_label(self, peripheral):
        """A phone compares the verified identity to this string, exactly.

        The label passed to the constructor is the advertised name and
        nothing else; serving it here is what made every Python peripheral
        unverifiable before.
        """
        from offline_protocol_sdk.ble_manager import DEVICE_ID_CHAR_UUID
        peripheral._resolve_identity()
        result = peripheral._on_read(DEVICE_ID_CHAR_UUID)
        assert result == bytearray(LOCAL_ADDRESS.encode("utf-8"))
        assert b"coordinator" not in result

    def test_read_device_id_case_insensitive(self, peripheral):
        from offline_protocol_sdk.ble_manager import DEVICE_ID_CHAR_UUID
        peripheral._resolve_identity()
        result = peripheral._on_read(DEVICE_ID_CHAR_UUID.upper())
        assert result == bytearray(LOCAL_ADDRESS.encode("utf-8"))

    def test_read_identity_is_the_core_built_assertion_verbatim(
        self, peripheral, mock_protocol
    ):
        """The binding assembles nothing: bytes in from the core, bytes out.

        The assertion is `public_key(32) || signature(64) || signed_data`
        and the one place that layout is written is the Rust core. A JSON
        document here, which is what this characteristic used to serve, is
        refused by every central before any cryptography runs.
        """
        from offline_protocol_sdk.ble_manager import IDENTITY_CHAR_UUID
        peripheral._resolve_identity()
        result = peripheral._on_read(IDENTITY_CHAR_UUID)
        assert bytes(result) == ASSERTION
        assert len(result) >= 96
        mock_protocol.identity_assertion.assert_called_once_with([])

    def test_reads_serve_nothing_before_the_identity_is_resolved(self, peripheral):
        """Unstarted, there is no address and no assertion to serve."""
        from offline_protocol_sdk.ble_manager import DEVICE_ID_CHAR_UUID, IDENTITY_CHAR_UUID
        assert peripheral._on_read(DEVICE_ID_CHAR_UUID) == bytearray(b"")
        assert peripheral._on_read(IDENTITY_CHAR_UUID) == bytearray(b"")

    def test_read_unknown_char_returns_empty(self, peripheral):
        result = peripheral._on_read("00000000-0000-0000-0000-000000000000")
        assert result == bytearray(b"")


# ---------------------------------------------------------------------------
# Identity resolution
# ---------------------------------------------------------------------------

class TestIdentityResolution:
    def test_refuses_to_start_without_an_address(self, mock_protocol):
        """No MLS, no address, no advertising.

        Serving a label in place of an address is something every
        conforming central refuses, so advertising would only cost the
        phones a connect-read-refuse cycle each.
        """
        mock_protocol.local_address = MagicMock(return_value=None)
        peripheral = BlePeripheral(mock_protocol, device_id="coordinator")
        with pytest.raises(TransportError, match="initialise MLS"):
            peripheral._resolve_identity()
        mock_protocol.identity_assertion.assert_not_called()

    def test_wraps_a_failed_assertion_build(self, mock_protocol):
        mock_protocol.identity_assertion = MagicMock(side_effect=RuntimeError("no key"))
        peripheral = BlePeripheral(mock_protocol, device_id="coordinator")
        with pytest.raises(TransportError, match="identity assertion"):
            peripheral._resolve_identity()

    @pytest.mark.asyncio
    async def test_start_refuses_before_creating_a_server(self, mock_protocol):
        """The refusal happens before bless is touched and leaves the state STOPPED."""
        mock_protocol.local_address = MagicMock(return_value=None)
        peripheral = BlePeripheral(mock_protocol, device_id="coordinator")
        with patch("offline_protocol_sdk.ble_peripheral.BlessServer") as server_cls, patch.object(
            BlePeripheral, "is_available", return_value=True
        ):
            with pytest.raises(TransportError):
                await peripheral.start()
            server_cls.assert_not_called()
        assert peripheral.state == TransportState.STOPPED
        mock_protocol.ble_status_changed.assert_not_called()

    def test_the_json_identity_parameter_is_gone(self):
        """The JSON form was the divergent copy; nothing may reintroduce it."""
        params = inspect.signature(BlePeripheral.__init__).parameters
        assert "identity_json" not in params


# ---------------------------------------------------------------------------
# Write handler
# ---------------------------------------------------------------------------

class TestWriteHandler:
    def test_write_feeds_protocol(self, peripheral, mock_protocol):
        from offline_protocol_sdk.ble_manager import MESSAGE_CHAR_UUID
        fragment_data = bytearray(b"\x03\x00hello")
        peripheral._on_write(MESSAGE_CHAR_UUID, fragment_data)

        mock_protocol.ble_fragment_received.assert_called_once_with(
            sender_id="ble-peer",
            fragment=list(b"\x03\x00hello"),
        )

    def test_write_updates_metrics(self, peripheral, mock_protocol):
        from offline_protocol_sdk.ble_manager import MESSAGE_CHAR_UUID
        peripheral._on_write(MESSAGE_CHAR_UUID, bytearray(b"\x03\x00hi"))
        assert peripheral._bytes_received == 4
        assert peripheral._fragments_received == 1

    def test_write_to_wrong_char_ignored(self, peripheral, mock_protocol):
        from offline_protocol_sdk.ble_manager import DEVICE_ID_CHAR_UUID
        peripheral._on_write(DEVICE_ID_CHAR_UUID, bytearray(b"data"))
        mock_protocol.ble_fragment_received.assert_not_called()

    def test_write_with_single_central_uses_its_uuid(self, peripheral, mock_protocol):
        from offline_protocol_sdk.ble_manager import MESSAGE_CHAR_UUID
        # Simulate one connected central
        peripheral._connected_centrals["AAAA-BBBB"] = 1234.0
        peripheral._on_write(MESSAGE_CHAR_UUID, bytearray(b"\x03\x00x"))

        mock_protocol.ble_fragment_received.assert_called_once()
        call_args = mock_protocol.ble_fragment_received.call_args
        assert call_args.kwargs["sender_id"] == "AAAA-BBBB"

    def test_write_with_multiple_centrals_uses_generic(self, peripheral, mock_protocol):
        from offline_protocol_sdk.ble_manager import MESSAGE_CHAR_UUID
        peripheral._connected_centrals["AAAA"] = 1.0
        peripheral._connected_centrals["BBBB"] = 2.0
        peripheral._on_write(MESSAGE_CHAR_UUID, bytearray(b"\x03\x00x"))

        call_args = mock_protocol.ble_fragment_received.call_args
        assert call_args.kwargs["sender_id"] == "ble-peer"


# ---------------------------------------------------------------------------
# Outgoing fragment drain
# ---------------------------------------------------------------------------

class TestOutgoingFragments:
    @pytest.fixture
    def started_peripheral(self, peripheral):
        """Peripheral with a mocked server to test drain logic."""
        mock_server = MagicMock()
        mock_char = MagicMock()
        mock_server.get_characteristic = MagicMock(return_value=mock_char)
        mock_server.update_value = MagicMock(return_value=True)
        peripheral._server = mock_server
        return peripheral, mock_server, mock_char

    @pytest.mark.asyncio
    async def test_drain_sends_fragments(self, started_peripheral, mock_protocol):
        peripheral, mock_server, mock_char = started_peripheral

        frag1 = MagicMock()
        frag1.data = [0x03, 0x00, 0x41, 0x42]  # "AB"
        frag1.recipient_id = "phone-1"

        # Return one fragment then None
        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=[frag1, None]
        )

        await peripheral._drain_outgoing_fragments()

        assert mock_char.value == bytes([0x03, 0x00, 0x41, 0x42])
        mock_server.update_value.assert_called_once()
        mock_protocol.ble_return_fragment.assert_called_once()
        assert peripheral._fragments_sent == 1
        assert peripheral._bytes_sent == 4

    @pytest.mark.asyncio
    async def test_drain_no_fragments(self, started_peripheral, mock_protocol):
        peripheral, mock_server, _ = started_peripheral
        mock_protocol.ble_get_next_fragment = MagicMock(return_value=None)

        await peripheral._drain_outgoing_fragments()

        mock_server.update_value.assert_not_called()
        assert peripheral._fragments_sent == 0

    @pytest.mark.asyncio
    async def test_concurrent_drain_is_serialised(self, started_peripheral, mock_protocol):
        """A second drain call while one is running returns immediately."""
        peripheral, mock_server, mock_char = started_peripheral

        frag = MagicMock()
        frag.data = [0x01]
        frag.recipient_id = "phone-1"

        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=[frag, None]
        )

        # Simulate concurrent drain: set flag before calling
        peripheral._draining = True
        await peripheral._drain_outgoing_fragments()
        mock_protocol.ble_get_next_fragment.assert_not_called()

        # Reset and verify normal drain works
        peripheral._draining = False
        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=[frag, None]
        )
        await peripheral._drain_outgoing_fragments()
        mock_protocol.ble_get_next_fragment.assert_called()
        assert peripheral._draining is False


# ---------------------------------------------------------------------------
# Peer monitoring
# ---------------------------------------------------------------------------

class TestPeerMonitoring:
    @pytest.mark.asyncio
    async def test_detects_new_central(self, peripheral, mock_protocol):
        """Simulate a central appearing in _central_subscriptions."""
        mock_delegate = MagicMock()
        mock_delegate._central_subscriptions = {}
        mock_delegate.is_advertising = MagicMock(return_value=True)

        mock_server = MagicMock()
        mock_server.peripheral_manager_delegate = mock_delegate
        peripheral._server = mock_server

        # Start the monitor loop
        task = asyncio.ensure_future(peripheral._peer_monitor_loop())

        # Wait for first tick (interval=1.0s) with generous margin
        await asyncio.sleep(1.5)
        assert len(peripheral._connected_centrals) == 0

        # Simulate a central subscribing
        mock_delegate._central_subscriptions["PHONE-UUID-1"] = ["6e400002"]

        await asyncio.sleep(1.5)
        assert "PHONE-UUID-1" in peripheral._connected_centrals
        mock_protocol.ble_peer_discovered.assert_called_with(
            peer_id="PHONE-UUID-1", rssi=-50
        )

        # Simulate central disconnecting
        mock_delegate._central_subscriptions.clear()

        await asyncio.sleep(1.5)
        assert len(peripheral._connected_centrals) == 0
        mock_protocol.ble_peer_lost.assert_called_with(peer_id="PHONE-UUID-1")

        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass

    @pytest.mark.asyncio
    async def test_respects_max_connections(self, peripheral, mock_protocol):
        """Connections beyond max_connections are ignored."""
        peripheral._max_connections = 1

        mock_delegate = MagicMock()
        mock_delegate._central_subscriptions = {
            "A": ["6e400002"],
            "B": ["6e400002"],
        }
        mock_delegate.is_advertising = MagicMock(return_value=True)

        mock_server = MagicMock()
        mock_server.peripheral_manager_delegate = mock_delegate
        peripheral._server = mock_server

        task = asyncio.ensure_future(peripheral._peer_monitor_loop())
        await asyncio.sleep(1.5)

        # Only 1 should be accepted
        assert len(peripheral._connected_centrals) == 1

        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


# ---------------------------------------------------------------------------
# Thread-safety — #4 in the PR review.
# `_on_write` / `_resolve_sender` run on bless's delegate thread while the
# asyncio loop mutates the same state from `_peer_monitor_loop`,
# `resolve_sender_identity`, and `stop`. Without the lock widening,
# concurrent iteration could fail mid-loop or return stale reads.
# ---------------------------------------------------------------------------

class TestConcurrentAccess:
    def test_resolve_sender_survives_concurrent_monitor_mutation(
        self, peripheral, mock_protocol
    ):
        """Hammering _resolve_sender from a worker thread while the event
        loop mutates _connected_centrals must not raise or return garbage."""
        import threading

        errors: list[BaseException] = []

        def reader() -> None:
            try:
                for _ in range(500):
                    peripheral._resolve_sender()
            except BaseException as exc:
                errors.append(exc)

        def writer() -> None:
            try:
                for i in range(500):
                    with peripheral._lock:
                        peripheral._connected_centrals[f"c{i}"] = 0.0
                    with peripheral._lock:
                        peripheral._connected_centrals.pop(f"c{i}", None)
            except BaseException as exc:
                errors.append(exc)

        threads = [threading.Thread(target=reader) for _ in range(3)] + [
            threading.Thread(target=writer) for _ in range(3)
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert errors == []

    def test_resolve_sender_identity_holds_lock_around_mutations(
        self, peripheral, mock_protocol
    ):
        """resolve_sender_identity must not leave _central_to_user_id in
        an inconsistent state under concurrent callers."""
        import threading

        peripheral._connected_centrals["only-central"] = 0.0
        errors: list[BaseException] = []

        def caller(uid: str) -> None:
            try:
                for _ in range(200):
                    peripheral.resolve_sender_identity(uid)
            except BaseException as exc:
                errors.append(exc)

        threads = [
            threading.Thread(target=caller, args=(f"user-{i}",))
            for i in range(4)
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert errors == []
        # Exactly one entry for the single central; value is whichever
        # thread won the last write.
        assert list(peripheral._central_to_user_id) == ["only-central"]

    def test_stop_clears_maps_under_lock(self, peripheral, mock_protocol):
        """stop() must snapshot-and-clear atomically: a delegate-thread
        reader running concurrently must never see a partially-mutated dict."""
        import threading

        peripheral._connected_centrals.update(
            {f"c{i}": 0.0 for i in range(50)}
        )
        peripheral._central_to_user_id.update(
            {f"c{i}": f"u{i}" for i in range(50)}
        )

        # Force stop to take the running-state branch.
        peripheral._state = TransportState.RUNNING
        peripheral._server = None  # skip the server.stop path

        errors: list[BaseException] = []
        stop_raised = threading.Event()

        def reader() -> None:
            try:
                while not stop_raised.is_set():
                    peripheral._resolve_sender()
            except BaseException as exc:
                errors.append(exc)

        t = threading.Thread(target=reader)
        t.start()

        # Run stop on a fresh loop — this test is sync so pytest-asyncio
        # isn't driving an event loop for us.
        loop = asyncio.new_event_loop()
        try:
            loop.run_until_complete(peripheral.stop())
        finally:
            loop.close()

        stop_raised.set()
        t.join()

        assert errors == []
        assert peripheral._connected_centrals == {}
        assert peripheral._central_to_user_id == {}


# ---------------------------------------------------------------------------
# A stop() cancelled part-way
# ---------------------------------------------------------------------------


class TestInterruptedStop:
    @pytest.mark.asyncio
    async def test_a_stop_cancelled_inside_the_server_can_be_finished(
        self, peripheral, mock_protocol
    ):
        """A shutdown deadline lands inside the GATT server's own stop, after
        the state moved to STOPPING. The retry has to run the teardown again:
        one that returned at once would leave the peripheral advertising,
        behind a transport that no `stop()` can reach."""
        release = asyncio.Event()

        async def a_server_that_does_not_let_go() -> None:
            await release.wait()

        server = MagicMock()
        server.stop = AsyncMock(side_effect=a_server_that_does_not_let_go)
        peripheral._server = server
        peripheral._is_advertising = True
        peripheral._connected_centrals["central-1"] = 0.0
        peripheral._state = TransportState.RUNNING

        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(peripheral.stop(), 0.05)
        assert peripheral.state == TransportState.STOPPING
        assert peripheral.get_metrics()["is_advertising"] is True

        release.set()
        await peripheral.stop()

        assert peripheral.state == TransportState.STOPPED
        assert server.stop.await_count == 2
        assert peripheral._server is None
        assert peripheral.get_metrics()["is_advertising"] is False
        assert peripheral._connected_centrals == {}
        mock_protocol.ble_peer_lost.assert_called_once_with(peer_id="central-1")
        mock_protocol.ble_status_changed.assert_called_once_with(is_available=False)

    @pytest.mark.asyncio
    async def test_stop_does_not_swallow_a_cancel_of_itself(self, peripheral):
        """`stop()` cancels its monitor task and waits for it to finish. A
        cancel of `stop()` that lands in that wait is the caller's shutdown
        deadline, not the monitor's cancellation, and has to come out of
        `stop()`. Caught there, the teardown ran on past the deadline."""

        async def a_monitor_slow_to_wind_down() -> None:
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                await asyncio.sleep(30)

        monitor = asyncio.ensure_future(a_monitor_slow_to_wind_down())
        await asyncio.sleep(0)
        peripheral._peer_monitor_task = monitor
        peripheral._state = TransportState.RUNNING

        stopping = asyncio.ensure_future(peripheral.stop())
        await asyncio.sleep(0.01)
        assert not stopping.done()
        stopping.cancel()
        with pytest.raises(asyncio.CancelledError):
            await stopping
        assert peripheral.state == TransportState.STOPPING

        await peripheral.stop()

        assert peripheral.state == TransportState.STOPPED
        assert monitor.done()
        assert peripheral._peer_monitor_task is None


# ---------------------------------------------------------------------------
# BlueZ: who the centrals are. bless's BlueZ backend tracks none, so the
# peripheral reads the caller BlueZ names in each ReadValue / WriteValue.
# A fake bus stands in for dbus_next: the hook and the Properties.Get calls
# are all the tracker uses.
# ---------------------------------------------------------------------------

from types import SimpleNamespace  # noqa: E402

from offline_protocol_sdk import ble_peripheral as ble_peripheral_module  # noqa: E402
from offline_protocol_sdk.ble_manager import (  # noqa: E402
    DEVICE_ID_CHAR_UUID,
    MESSAGE_CHAR_UUID,
)
from offline_protocol_sdk.ble_peripheral import _BlueZCentrals  # noqa: E402

APP_BASE = "/org/bluez/coordinato"
CHAR_PATH = APP_BASE + "/service0001/char0001"
PHONE = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01"
OTHER_PHONE = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_02"
OUR_PERIPHERAL = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_03"


def gatt_call(member: str, device: str | None, path: str = CHAR_PATH) -> SimpleNamespace:
    options = {} if device is None else {"device": SimpleNamespace(value=device)}
    body = [options] if member == "ReadValue" else [[1, 2, 3], options]
    return SimpleNamespace(
        message_type=SimpleNamespace(name="METHOD_CALL"),
        interface="org.bluez.GattCharacteristic1",
        member=member,
        path=path,
        body=body,
    )


class FakeBus:
    """``add_message_handler`` plus ``Device1.Connected`` answers."""

    def __init__(self) -> None:
        self.handlers: list = []
        self.connected: dict[str, bool] = {}
        self.queried: list[str] = []

    def add_message_handler(self, handler) -> None:
        self.handlers.append(handler)

    def remove_message_handler(self, handler) -> None:
        self.handlers.remove(handler)

    def deliver(self, msg) -> None:
        for handler in self.handlers:
            assert not handler(msg), "the hook must never handle a call"

    async def call(self, path: str):
        self.queried.append(path)
        if path not in self.connected:
            return SimpleNamespace(message_type=SimpleNamespace(name="ERROR"), body=[])
        return SimpleNamespace(
            message_type=SimpleNamespace(name="METHOD_RETURN"),
            body=[SimpleNamespace(value=self.connected[path])],
        )


@pytest.fixture
def fake_bus(monkeypatch):
    monkeypatch.setattr(ble_peripheral_module, "_device_connected_query", lambda path: path)
    return FakeBus()


class TestBlueZCentrals:
    def test_a_device_that_calls_our_application_is_a_central(self, fake_bus):
        tracker = _BlueZCentrals(APP_BASE)
        tracker.attach(fake_bus)
        fake_bus.deliver(gatt_call("ReadValue", PHONE))
        assert tracker.current == "AA:BB:CC:DD:EE:01"
        fake_bus.deliver(gatt_call("WriteValue", OTHER_PHONE))
        assert tracker.current == "AA:BB:CC:DD:EE:02"

    @pytest.mark.asyncio
    async def test_a_peripheral_our_central_holds_is_not_a_central(self, fake_bus):
        """Both are Device1 objects with Connected true; only a central of
        ours calls into our application."""
        tracker = _BlueZCentrals(APP_BASE)
        tracker.attach(fake_bus)
        fake_bus.connected = {PHONE: True, OUR_PERIPHERAL: True}
        fake_bus.deliver(gatt_call("ReadValue", PHONE))
        assert await tracker.connected(subscribed=True) == {"AA:BB:CC:DD:EE:01"}

    @pytest.mark.asyncio
    async def test_calls_to_another_application_are_not_ours(self, fake_bus):
        tracker = _BlueZCentrals(APP_BASE)
        tracker.attach(fake_bus)
        fake_bus.connected = {PHONE: True}
        fake_bus.deliver(gatt_call("WriteValue", PHONE, path="/org/bluez/other/service0001/char0001"))
        fake_bus.deliver(gatt_call("WriteValue", PHONE, path=APP_BASE + "x/service0001/char0001"))
        assert tracker.current is None
        assert await tracker.connected(subscribed=True) == set()

    def test_a_call_without_a_device_clears_the_current_caller(self, fake_bus):
        """A stale caller would attribute this write to the previous one."""
        tracker = _BlueZCentrals(APP_BASE)
        tracker.attach(fake_bus)
        fake_bus.deliver(gatt_call("WriteValue", PHONE))
        fake_bus.deliver(gatt_call("WriteValue", None))
        assert tracker.current is None

    @pytest.mark.asyncio
    async def test_a_disconnected_or_removed_device_stops_being_a_central(self, fake_bus):
        tracker = _BlueZCentrals(APP_BASE)
        tracker.attach(fake_bus)
        fake_bus.connected = {PHONE: True, OTHER_PHONE: True}
        fake_bus.deliver(gatt_call("ReadValue", PHONE))
        fake_bus.deliver(gatt_call("ReadValue", OTHER_PHONE))
        assert len(await tracker.connected(subscribed=True)) == 2
        fake_bus.connected[PHONE] = False
        del fake_bus.connected[OTHER_PHONE]
        assert await tracker.connected(subscribed=True) == set()
        fake_bus.queried.clear()
        await tracker.connected(subscribed=True)
        assert fake_bus.queried == []

    @pytest.mark.asyncio
    async def test_nobody_is_listed_while_nobody_subscribes(self, fake_bus):
        tracker = _BlueZCentrals(APP_BASE)
        tracker.attach(fake_bus)
        fake_bus.connected = {PHONE: True}
        fake_bus.deliver(gatt_call("ReadValue", PHONE))
        assert await tracker.connected(subscribed=False) == set()
        assert await tracker.connected(subscribed=True) == {"AA:BB:CC:DD:EE:01"}

    def test_detach_removes_the_hook(self, fake_bus):
        tracker = _BlueZCentrals(APP_BASE)
        tracker.attach(fake_bus)
        tracker.detach()
        assert fake_bus.handlers == []


def bluez_peripheral(mock_protocol, fake_bus, subscribed: list[str]) -> BlePeripheral:
    peripheral = BlePeripheral(mock_protocol, device_id="coordinator")
    server = MagicMock()
    server.bus = fake_bus
    server.app.base_path = APP_BASE
    server.app.subscribed_characteristics = subscribed
    peripheral._server = server
    peripheral._attach_bluez_centrals()
    return peripheral


class TestPeripheralOnBlueZ:
    @pytest.mark.asyncio
    async def test_the_monitor_announces_and_loses_a_central(self, mock_protocol, fake_bus, monkeypatch):
        monkeypatch.setattr(ble_peripheral_module, "_PEER_MONITOR_INTERVAL", 0.01)
        subscribed: list[str] = []
        peripheral = bluez_peripheral(mock_protocol, fake_bus, subscribed)
        fake_bus.connected = {PHONE: True}
        task = asyncio.ensure_future(peripheral._peer_monitor_loop())
        try:
            fake_bus.deliver(gatt_call("ReadValue", PHONE))
            peripheral._on_read(DEVICE_ID_CHAR_UUID)
            await asyncio.sleep(0.05)
            mock_protocol.ble_peer_discovered.assert_not_called()

            subscribed.append(MESSAGE_CHAR_UUID)
            await asyncio.sleep(0.05)
            mock_protocol.ble_peer_discovered.assert_called_once_with(
                peer_id="AA:BB:CC:DD:EE:01", rssi=-50
            )
            assert "AA:BB:CC:DD:EE:01" in peripheral._connected_centrals

            fake_bus.connected[PHONE] = False
            await asyncio.sleep(0.05)
            mock_protocol.ble_peer_lost.assert_called_with(peer_id="AA:BB:CC:DD:EE:01")
            assert peripheral._connected_centrals == {}
        finally:
            task.cancel()
            await asyncio.wait({task})

    def test_a_write_is_attributed_to_its_caller_among_several(self, mock_protocol, fake_bus):
        """Before, two connected centrals made every write ``ble-peer``."""
        peripheral = bluez_peripheral(mock_protocol, fake_bus, [MESSAGE_CHAR_UUID])
        peripheral._connected_centrals.update({"AA:BB:CC:DD:EE:01": 1.0, "AA:BB:CC:DD:EE:02": 2.0})
        fake_bus.deliver(gatt_call("WriteValue", OTHER_PHONE))
        peripheral._on_write(MESSAGE_CHAR_UUID, bytearray(b"\x03\x00x"))
        assert mock_protocol.ble_fragment_received.call_args.kwargs["sender_id"] == "AA:BB:CC:DD:EE:02"

    def test_subscription_is_read_from_the_message_characteristic(self, mock_protocol, fake_bus):
        peripheral = bluez_peripheral(mock_protocol, fake_bus, [])
        assert not peripheral.has_subscriber()
        peripheral._server.app.subscribed_characteristics = [DEVICE_ID_CHAR_UUID]
        assert not peripheral.has_subscriber()
        peripheral._server.app.subscribed_characteristics = [MESSAGE_CHAR_UUID.upper()]
        assert peripheral.has_subscriber()

    @pytest.mark.asyncio
    async def test_the_drain_takes_nothing_while_nobody_subscribes(self, mock_protocol, fake_bus):
        """A popped fragment is gone; with no subscriber it would reach no one."""
        peripheral = bluez_peripheral(mock_protocol, fake_bus, [])
        await peripheral._drain_outgoing_fragments()
        mock_protocol.ble_get_next_fragment.assert_not_called()

    @pytest.mark.asyncio
    async def test_stop_removes_the_hook(self, mock_protocol, fake_bus):
        peripheral = bluez_peripheral(mock_protocol, fake_bus, [])
        peripheral._state = TransportState.RUNNING
        peripheral._server.stop = AsyncMock()
        await peripheral.stop()
        assert fake_bus.handlers == []
        assert peripheral._bluez_centrals is None

    @pytest.mark.asyncio
    @pytest.mark.parametrize("platform, hooked", [("linux", 1), ("darwin", 0)])
    async def test_start_hooks_the_bus_on_linux_only(
        self, mock_protocol, fake_bus, monkeypatch, platform, hooked
    ):
        """The tests above build the tracker by hand; this is the wiring
        that makes start() build it, and only where bless runs on BlueZ."""
        monkeypatch.setattr(sys, "platform", platform)
        server = MagicMock()
        server.bus = fake_bus
        server.app.base_path = APP_BASE
        server.add_new_service = AsyncMock()
        server.add_new_characteristic = AsyncMock()
        server.start = AsyncMock()
        server.stop = AsyncMock()
        peripheral = BlePeripheral(mock_protocol, device_id="coordinator")
        # bless is not installed on Windows, where the module's GATT enums
        # are None: stand in for them too, so this runs on every platform.
        with patch.object(
            ble_peripheral_module, "BlessServer", return_value=server
        ), patch.object(
            ble_peripheral_module, "GATTCharacteristicProperties", MagicMock()
        ), patch.object(
            ble_peripheral_module, "GATTAttributePermissions", MagicMock()
        ), patch.object(BlePeripheral, "is_available", return_value=True):
            await peripheral.start()
        try:
            assert len(fake_bus.handlers) == hooked
            assert (peripheral._bluez_centrals is not None) == bool(hooked)
        finally:
            await peripheral.stop()
        assert fake_bus.handlers == []


# ---------------------------------------------------------------------------
# One drain for both roles: the core's fragment queue cannot be peeked or
# refilled, so each fragment is popped once and given to the role holding a
# link for it.
# ---------------------------------------------------------------------------

class TestSharedFragmentDrain:
    def fragments(self, *pairs):
        return [SimpleNamespace(recipient_id=r, data=list(d)) for r, d in pairs] + [None]

    def roles(self, mock_protocol, central_peers: set[str], subscribed: bool):
        from offline_protocol_sdk.protocol_manager import _BleTransportCallbackImpl

        central = MagicMock()
        central._protocol = mock_protocol
        central.sent = []

        async def write_fragment(recipient, data):
            if recipient not in central_peers:
                return False
            central.sent.append((recipient, data))
            return True

        central.write_fragment = write_fragment
        peripheral = MagicMock()
        peripheral.sent = []
        peripheral.has_subscriber = MagicMock(return_value=subscribed)

        async def notify_fragment(data):
            peripheral.sent.append(data)
            return True

        peripheral.notify_fragment = notify_fragment
        return _BleTransportCallbackImpl(central, peripheral), central, peripheral

    @pytest.mark.asyncio
    async def test_each_fragment_goes_to_the_role_that_holds_its_peer(self, mock_protocol):
        router, central, peripheral = self.roles(mock_protocol, {"off1central"}, subscribed=True)
        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=self.fragments(
                ("off1central", b"a1"), ("AA:BB:CC:DD:EE:01", b"b1"), ("off1central", b"a2")
            )
        )
        await router._drain_shared()
        assert central.sent == [("off1central", b"a1"), ("off1central", b"a2")]
        assert peripheral.sent == [b"b1"]
        assert mock_protocol.ble_get_next_fragment.call_count == 4
        assert router.fragments_dropped == 0

    @pytest.mark.asyncio
    async def test_a_fragment_nobody_can_carry_is_counted(self, mock_protocol):
        router, central, peripheral = self.roles(mock_protocol, set(), subscribed=False)
        mock_protocol.ble_get_next_fragment = MagicMock(side_effect=self.fragments(("x", b"z")))
        await router._drain_shared()
        assert central.sent == [] and peripheral.sent == []
        assert router.fragments_dropped == 1

    def test_with_both_roles_neither_drains_on_its_own(self, mock_protocol):
        """Two drains popping the one queue split a message across links."""
        router, central, peripheral = self.roles(mock_protocol, set(), subscribed=True)
        central._loop = None
        peripheral._loop = None
        router.on_fragments_available()
        central.on_fragments_available.assert_not_called()
        peripheral.on_fragments_available.assert_not_called()
