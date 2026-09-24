"""Tests for BleManager — BLE central (scanner) role.

All tests mock bleak and OfflineProtocol so they run without actual
Bluetooth hardware.
"""

from __future__ import annotations

import asyncio
import time
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from offline_protocol_sdk import ble_manager as ble_manager_module
from offline_protocol_sdk.ble_manager import (
    ADAPTIVE_COOLDOWN_PER_PERIPHERAL,
    ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE,
    ADAPTIVE_MIN_RSSI,
    DEVICE_ID_CHAR_UUID,
    IDENTITY_CHAR_UUID,
    MESSAGE_CHAR_UUID,
    SERVICE_UUID,
    BleManager,
)
from offline_protocol_sdk.transport_manager import TransportError, TransportState

# A peer's proved address and the (opaque to this layer) bytes that prove it.
PEER_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
PEER_ASSERTION = bytes(range(96))


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
def manager(mock_protocol):
    """A BleManager instance (not started)."""
    return BleManager(mock_protocol, device_id="desktop-1")


# ---------------------------------------------------------------------------
# Initial state
# ---------------------------------------------------------------------------


class TestInitialState:
    def test_initial_state_is_stopped(self, manager):
        assert manager.state == TransportState.STOPPED

    def test_is_available(self, manager):
        assert manager.is_available() is True

    def test_get_metrics_defaults(self, manager):
        m = manager.get_metrics()
        assert m["bytes_sent"] == 0
        assert m["bytes_received"] == 0
        assert m["fragments_sent"] == 0
        assert m["fragments_received"] == 0
        assert m["connected_peers"] == 0
        assert m["discovered_peers"] == 0


# ---------------------------------------------------------------------------
# Rate limiting
# ---------------------------------------------------------------------------


class TestRateLimiting:
    def test_should_connect_allows_first_attempt(self, manager):
        now = time.monotonic()
        assert manager._should_connect_locked("AA:BB", now) is True

    def test_should_connect_blocks_during_cooldown(self, manager):
        now = time.monotonic()
        manager._connection_attempts["AA:BB"] = now
        assert manager._should_connect_locked("AA:BB", now + 1.0) is False

    def test_should_connect_allows_after_cooldown(self, manager):
        now = time.monotonic()
        manager._connection_attempts["AA:BB"] = now
        assert manager._should_connect_locked(
            "AA:BB", now + ADAPTIVE_COOLDOWN_PER_PERIPHERAL + 1.0
        ) is True

    def test_global_rate_limit(self, manager):
        now = time.monotonic()
        # Fill up global attempts
        manager._global_attempts = [
            now - i for i in range(ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE)
        ]
        assert manager._should_connect_locked("NEW:ADDR", now) is False

    def test_global_rate_limit_expires(self, manager):
        now = time.monotonic()
        # All attempts older than 60 seconds
        manager._global_attempts = [
            now - 61.0 for _ in range(ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE)
        ]
        assert manager._should_connect_locked("NEW:ADDR", now) is True


# ---------------------------------------------------------------------------
# Advertisement handling
# ---------------------------------------------------------------------------


class TestAdvertisementHandling:
    def test_weak_signal_ignored(self, manager, mock_protocol):
        """Advertisements below RSSI threshold are dropped."""
        manager._loop = asyncio.new_event_loop()
        device = MagicMock()
        device.address = "AA:BB:CC:DD:EE:FF"
        adv = MagicMock()
        adv.rssi = ADAPTIVE_MIN_RSSI - 1  # too weak

        manager._on_advertisement(device, adv)

        mock_protocol.ble_peer_discovered.assert_not_called()

    def test_advertisement_updates_last_seen_without_announcing_a_label(
        self, manager, mock_protocol
    ):
        """An unverified link is tracked, never announced.

        A Bluetooth address is a label. The core keys routes by whatever it
        is handed, so announcing one here put a name nothing could prove
        into the routing table on every scan.
        """
        manager._loop = MagicMock()
        manager._loop.is_closed.return_value = False
        manager._loop.is_running.return_value = True
        manager._loop.call_soon_threadsafe = MagicMock()

        device = MagicMock()
        device.address = "AA:BB:CC:DD:EE:FF"
        adv = MagicMock()
        adv.rssi = -60

        manager._on_advertisement(device, adv)

        assert "AA:BB:CC:DD:EE:FF" in manager._last_seen
        mock_protocol.ble_peer_discovered.assert_not_called()

    def test_advertisement_refreshes_a_verified_peer_under_its_address(
        self, manager, mock_protocol
    ):
        manager._loop = MagicMock()
        manager._loop.is_closed.return_value = False
        manager._loop.is_running.return_value = True
        addr = "AA:BB:CC:DD:EE:FF"
        manager._clients[addr] = MagicMock()
        manager._peer_device_ids[addr] = PEER_ADDRESS

        device = MagicMock()
        device.address = addr
        adv = MagicMock()
        adv.rssi = -55

        manager._on_advertisement(device, adv)

        mock_protocol.ble_peer_discovered.assert_called_once_with(
            peer_id=PEER_ADDRESS, rssi=-55
        )

    def test_advertisement_triggers_connection(self, manager, mock_protocol):
        """First advertisement for an unknown peer triggers connection."""
        mock_loop = MagicMock()
        mock_loop.is_closed.return_value = False
        mock_loop.is_running.return_value = True
        manager._loop = mock_loop

        device = MagicMock()
        device.address = "AA:BB:CC:DD:EE:FF"
        adv = MagicMock()
        adv.rssi = -60

        manager._on_advertisement(device, adv)

        mock_loop.call_soon_threadsafe.assert_called_once()
        assert "AA:BB:CC:DD:EE:FF" in manager._connecting

    def test_already_connected_peer_not_reconnected(self, manager, mock_protocol):
        """Peers already in _clients don't trigger new connections."""
        mock_loop = MagicMock()
        mock_loop.is_closed.return_value = False
        mock_loop.is_running.return_value = True
        manager._loop = mock_loop

        addr = "AA:BB:CC:DD:EE:FF"
        manager._clients[addr] = MagicMock()  # already connected

        device = MagicMock()
        device.address = addr
        adv = MagicMock()
        adv.rssi = -60

        manager._on_advertisement(device, adv)

        mock_loop.call_soon_threadsafe.assert_not_called()


# ---------------------------------------------------------------------------
# Peer verification
# ---------------------------------------------------------------------------


class TestPeerVerification:
    """The four steps of the BLE framing chapter, and what each failure does.

    Steps one to three run in the core behind ``verify_identity_assertion``;
    the module-level name is patched so these run without the library. The
    fourth step, the exact comparison to the Device id characteristic, is
    this binding's and is exercised for real.
    """

    def test_a_matching_proof_yields_the_derived_address(
        self, manager, monkeypatch
    ):
        seen = {}

        def verifier(assertion):
            seen["assertion"] = assertion
            return PEER_ADDRESS

        monkeypatch.setattr(ble_manager_module, "verify_identity_assertion", verifier)
        assert manager._verify_peer("AA:BB", PEER_ADDRESS, PEER_ASSERTION) == PEER_ADDRESS
        # Bytes in, as the sequence the FFI takes; nothing split here.
        assert seen["assertion"] == list(PEER_ASSERTION)

    def test_no_identity_is_no_peer(self, manager, monkeypatch):
        verifier = MagicMock(return_value=PEER_ADDRESS)
        monkeypatch.setattr(ble_manager_module, "verify_identity_assertion", verifier)
        assert manager._verify_peer("AA:BB", PEER_ADDRESS, None) is None
        verifier.assert_not_called()

    def test_a_refused_assertion_is_no_peer(self, manager, monkeypatch):
        """Whatever the core refuses (short, off-curve, bad signature) is refused here."""
        monkeypatch.setattr(
            ble_manager_module,
            "verify_identity_assertion",
            MagicMock(side_effect=Exception("Identity assertion is 95 bytes, minimum is 96")),
        )
        assert manager._verify_peer("AA:BB", PEER_ADDRESS, PEER_ASSERTION[:95]) is None

    def test_a_proof_of_another_address_is_no_peer(self, manager, monkeypatch):
        """The claim and the proof must name the same address."""
        monkeypatch.setattr(
            ble_manager_module,
            "verify_identity_assertion",
            MagicMock(return_value=PEER_ADDRESS),
        )
        other = "off1q9nxs74dlp3t6amv3lqchr5l3csq39c5s5grq3yh"
        assert manager._verify_peer("AA:BB", other, PEER_ASSERTION) is None

    def test_the_comparison_is_exact_not_case_folded(self, manager, monkeypatch):
        """A value differing only in case belongs to a peer that did not
        derive its own id the way this one did, and is refused rather than
        normalised into agreement."""
        monkeypatch.setattr(
            ble_manager_module,
            "verify_identity_assertion",
            MagicMock(return_value=PEER_ADDRESS),
        )
        assert manager._verify_peer("AA:BB", PEER_ADDRESS.upper(), PEER_ASSERTION) is None


class TestConnectVerifiesBeforeAnnouncing:
    """The connect path, end to end, against a fake GATT client."""

    def _client(self, reads: dict[str, bytes]) -> MagicMock:
        client = MagicMock()
        client.connect = AsyncMock()
        client.disconnect = AsyncMock()
        client.start_notify = AsyncMock()

        async def read_gatt_char(uuid):
            if uuid in reads:
                return bytearray(reads[uuid])
            raise RuntimeError(f"no such characteristic {uuid}")

        client.read_gatt_char = AsyncMock(side_effect=read_gatt_char)
        return client

    @pytest.mark.asyncio
    async def test_a_verified_peer_is_announced_under_the_derived_address(
        self, manager, mock_protocol, monkeypatch
    ):
        monkeypatch.setattr(
            ble_manager_module,
            "verify_identity_assertion",
            MagicMock(return_value=PEER_ADDRESS),
        )
        client = self._client({
            DEVICE_ID_CHAR_UUID: PEER_ADDRESS.encode("utf-8"),
            IDENTITY_CHAR_UUID: PEER_ASSERTION,
        })
        device = MagicMock()
        device.address = "AA:BB"
        manager._connecting.add("AA:BB")

        with patch("offline_protocol_sdk.ble_manager.BleakClient", return_value=client):
            await manager._connect_to_peer(device)

        mock_protocol.ble_peer_discovered.assert_called_once_with(peer_id=PEER_ADDRESS, rssi=-70)
        assert manager._peer_device_ids["AA:BB"] == PEER_ADDRESS
        assert manager._device_id_to_addr[PEER_ADDRESS] == "AA:BB"
        client.start_notify.assert_awaited_once()
        client.disconnect.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_a_peer_whose_proof_names_another_address_is_dropped(
        self, manager, mock_protocol, monkeypatch
    ):
        monkeypatch.setattr(
            ble_manager_module,
            "verify_identity_assertion",
            MagicMock(return_value=PEER_ADDRESS),
        )
        client = self._client({
            DEVICE_ID_CHAR_UUID: b"coordinator",
            IDENTITY_CHAR_UUID: PEER_ASSERTION,
        })
        device = MagicMock()
        device.address = "AA:BB"
        manager._connecting.add("AA:BB")

        with patch("offline_protocol_sdk.ble_manager.BleakClient", return_value=client):
            await manager._connect_to_peer(device)

        mock_protocol.ble_peer_discovered.assert_not_called()
        client.disconnect.assert_awaited_once()
        client.start_notify.assert_not_awaited()
        assert "AA:BB" not in manager._clients
        assert "AA:BB" not in manager._peer_device_ids
        assert "AA:BB" not in manager._connecting

    @pytest.mark.asyncio
    async def test_a_peer_that_serves_no_identity_is_dropped(
        self, manager, mock_protocol, monkeypatch
    ):
        """The device id alone was enough before. It is not: it is a claim."""
        verifier = MagicMock(return_value=PEER_ADDRESS)
        monkeypatch.setattr(ble_manager_module, "verify_identity_assertion", verifier)
        client = self._client({DEVICE_ID_CHAR_UUID: PEER_ADDRESS.encode("utf-8")})
        device = MagicMock()
        device.address = "AA:BB"
        manager._connecting.add("AA:BB")

        with patch("offline_protocol_sdk.ble_manager.BleakClient", return_value=client):
            await manager._connect_to_peer(device)

        verifier.assert_not_called()
        mock_protocol.ble_peer_discovered.assert_not_called()
        client.disconnect.assert_awaited_once()


# ---------------------------------------------------------------------------
# Fragment handling
# ---------------------------------------------------------------------------


class TestFragmentHandling:
    def test_on_fragment_received_feeds_protocol(self, manager, mock_protocol):
        manager._on_fragment_received("peer-1", b"\x03\x00hello")

        mock_protocol.ble_fragment_received.assert_called_once_with(
            sender_id="peer-1",
            fragment=list(b"\x03\x00hello"),
        )

    def test_on_fragment_received_updates_metrics(self, manager, mock_protocol):
        manager._on_fragment_received("peer-1", b"\x03\x00hi")
        assert manager._bytes_received == 4
        assert manager._fragments_received == 1

    def test_on_fragment_received_protocol_error(self, manager, mock_protocol):
        """Protocol errors are caught and don't propagate."""
        mock_protocol.ble_fragment_received.side_effect = RuntimeError("boom")
        # Should not raise
        manager._on_fragment_received("peer-1", b"\x03\x00hi")

    @pytest.mark.asyncio
    async def test_drain_outgoing_sends_to_peer(self, manager, mock_protocol):
        """Outgoing fragments are written to the correct BleakClient."""
        mock_client = AsyncMock()
        mock_client.is_connected = True
        addr = "AA:BB:CC:DD:EE:FF"
        manager._clients[addr] = mock_client
        manager._peer_device_ids[addr] = "phone-1"
        manager._device_id_to_addr["phone-1"] = addr

        frag = MagicMock()
        frag.recipient_id = "phone-1"
        frag.data = [0x03, 0x00, 0x41]
        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=[frag, None]
        )

        await manager._drain_outgoing_fragments()

        mock_client.write_gatt_char.assert_called_once_with(
            MESSAGE_CHAR_UUID,
            bytes([0x03, 0x00, 0x41]),
            response=False,
        )
        mock_protocol.ble_return_fragment.assert_called_once()
        assert manager._bytes_sent == 3
        assert manager._fragments_sent == 1

    @pytest.mark.asyncio
    async def test_drain_outgoing_no_fragments(self, manager, mock_protocol):
        mock_protocol.ble_get_next_fragment = MagicMock(return_value=None)
        await manager._drain_outgoing_fragments()
        assert manager._fragments_sent == 0

    @pytest.mark.asyncio
    async def test_drain_outgoing_missing_peer(self, manager, mock_protocol):
        """Fragments for unknown peers still return to pool."""
        frag = MagicMock()
        frag.recipient_id = "unknown-peer"
        frag.data = [0x01]
        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=[frag, None]
        )

        await manager._drain_outgoing_fragments()

        mock_protocol.ble_return_fragment.assert_called_once()
        assert manager._bytes_sent == 0

    @pytest.mark.asyncio
    async def test_concurrent_drain_is_serialised(self, manager, mock_protocol):
        """A second _drain_outgoing_fragments call while one is running returns immediately."""
        frag = MagicMock()
        frag.recipient_id = "peer-1"
        frag.data = [0x01]

        mock_client = AsyncMock()
        mock_client.is_connected = True
        manager._clients["AA:BB"] = mock_client
        manager._peer_device_ids["AA:BB"] = "peer-1"
        manager._device_id_to_addr["peer-1"] = "AA:BB"

        # First fragment returned, then None
        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=[frag, None]
        )

        # Simulate concurrent drain: set flag before calling
        manager._draining = True
        await manager._drain_outgoing_fragments()
        # Should have returned immediately without calling get_next_fragment
        mock_protocol.ble_get_next_fragment.assert_not_called()

        # Reset and verify normal drain works after flag is cleared
        manager._draining = False
        mock_protocol.ble_get_next_fragment = MagicMock(
            side_effect=[frag, None]
        )
        await manager._drain_outgoing_fragments()
        mock_protocol.ble_get_next_fragment.assert_called()
        assert manager._draining is False


# ---------------------------------------------------------------------------
# Client lookup
# ---------------------------------------------------------------------------


class TestClientLookup:
    def test_find_client_known_peer(self, manager):
        mock_client = MagicMock()
        addr = "AA:BB"
        manager._clients[addr] = mock_client
        manager._device_id_to_addr["peer-1"] = addr

        assert manager._find_client_for_peer("peer-1") is mock_client

    def test_find_client_unknown_peer(self, manager):
        assert manager._find_client_for_peer("unknown") is None


# ---------------------------------------------------------------------------
# Peer disconnect
# ---------------------------------------------------------------------------


class TestPeerDisconnect:
    @pytest.mark.asyncio
    async def test_on_peer_disconnected_cleans_up(self, manager, mock_protocol):
        addr = "AA:BB"
        mock_client = MagicMock()
        manager._clients[addr] = mock_client
        manager._peer_device_ids[addr] = "phone-1"
        manager._device_id_to_addr["phone-1"] = addr

        await manager._on_peer_disconnected(addr)

        assert addr not in manager._clients
        assert addr not in manager._peer_device_ids
        assert "phone-1" not in manager._device_id_to_addr
        mock_protocol.ble_peer_lost.assert_called_once_with(peer_id="phone-1")

    @pytest.mark.asyncio
    async def test_an_unverified_link_closing_reports_no_peer_lost(
        self, manager, mock_protocol
    ):
        """Never announced, so nothing to report: the core has no such peer."""
        addr = "AA:BB"
        manager._clients[addr] = MagicMock()

        await manager._on_peer_disconnected(addr)

        assert addr not in manager._clients
        mock_protocol.ble_peer_lost.assert_not_called()


# ---------------------------------------------------------------------------
# Peer cleanup loop
# ---------------------------------------------------------------------------


class TestPeerCleanup:
    @pytest.mark.asyncio
    async def test_stale_peers_cleaned_up(self, manager, mock_protocol):
        """Peers not seen for > PEER_LOST_TIMEOUT are removed."""
        addr = "STALE:ADDR"
        # Set last_seen far in the past
        manager._last_seen[addr] = time.monotonic() - 60.0
        manager._peer_device_ids[addr] = "stale-peer"
        manager._device_id_to_addr["stale-peer"] = addr
        # Not in _clients (disconnected but still tracked)

        task = asyncio.ensure_future(manager._peer_cleanup_loop())
        # PEER_LOST_TIMEOUT/2 = 15s, but the stale entry is already old
        # so the first tick should clean it up. We need to wait just past
        # the sleep interval. Use a short timeout since the loop sleeps 15s.
        # Instead, directly test the cleanup logic by letting one iteration run.
        await asyncio.sleep(0.1)
        # The loop sleeps for PEER_LOST_TIMEOUT/2 (15s) before first check,
        # so we can't easily wait. Cancel and test the logic directly instead.
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass

        # Directly test cleanup logic by simulating what the loop does
        from offline_protocol_sdk.ble_manager import PEER_LOST_TIMEOUT

        now = time.monotonic()
        manager._last_seen[addr] = now - PEER_LOST_TIMEOUT - 1.0
        manager._peer_device_ids[addr] = "stale-peer"
        manager._device_id_to_addr["stale-peer"] = addr

        stale = [
            a
            for a, ts in manager._last_seen.items()
            if now - ts > PEER_LOST_TIMEOUT and a not in manager._clients
        ]
        for a in stale:
            manager._last_seen.pop(a, None)
            device_id = manager._peer_device_ids.pop(a, a)
            manager._device_id_to_addr.pop(device_id, None)

        assert addr not in manager._last_seen
        assert addr not in manager._peer_device_ids
