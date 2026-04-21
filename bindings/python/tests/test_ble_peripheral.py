"""Tests for BlePeripheral — BLE GATT server (peripheral role).

All tests mock the bless server and OfflineProtocol so they run without
actual Bluetooth hardware.
"""

from __future__ import annotations

import asyncio
import json
import sys
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from offline_protocol_sdk.ble_peripheral import BlePeripheral
from offline_protocol_sdk.transport_manager import TransportState


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
    def test_read_device_id(self, peripheral):
        from offline_protocol_sdk.ble_manager import DEVICE_ID_CHAR_UUID
        result = peripheral._on_read(DEVICE_ID_CHAR_UUID)
        assert result == bytearray(b"coordinator")

    def test_read_device_id_case_insensitive(self, peripheral):
        from offline_protocol_sdk.ble_manager import DEVICE_ID_CHAR_UUID
        result = peripheral._on_read(DEVICE_ID_CHAR_UUID.upper())
        assert result == bytearray(b"coordinator")

    def test_read_identity(self, peripheral):
        from offline_protocol_sdk.ble_manager import IDENTITY_CHAR_UUID
        result = peripheral._on_read(IDENTITY_CHAR_UUID)
        parsed = json.loads(result.decode("utf-8"))
        assert parsed["device_id"] == "coordinator"
        assert parsed["role"] == "coordinator"
        assert parsed["protocol"] == "offline-protocol"

    def test_read_unknown_char_returns_empty(self, peripheral):
        result = peripheral._on_read("00000000-0000-0000-0000-000000000000")
        assert result == bytearray(b"")


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
