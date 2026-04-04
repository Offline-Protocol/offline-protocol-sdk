"""BLE transport manager using the ``bleak`` library.

Mirrors the central (scanner) role from ``BleManager.swift`` /
``BleManager.kt``.  Connects to nearby mesh peers over Bluetooth Low
Energy, reads their device IDs, subscribes to message notifications, and
drives the UniFFI ``OfflineProtocol`` BLE transport methods.

**Known limitation:** ``bleak`` only supports the *central* (scanner/client)
role.  Peripheral (advertiser/GATT server) support requires the ``bless``
library or platform-specific code and is not included here.  This means a
desktop node can *discover and receive from* mobile peers, but mobile peers
cannot discover the desktop unless the desktop also runs a GATT server via
``bless`` or an external tool.
"""

from __future__ import annotations

import asyncio
import logging
import time
from typing import Any

from bleak import BleakClient, BleakScanner
from bleak.backends.device import BLEDevice
from bleak.backends.scanner import AdvertisementData

from .transport_manager import TransportError, TransportManager, TransportState

logger = logging.getLogger(__name__)

# -- BLE constants (matching iOS/Android) -------------------------------------

SERVICE_UUID = "6e400001-b5a3-f393-e0a9-e50e24dcca9e"
MESSAGE_CHAR_UUID = "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
DEVICE_ID_CHAR_UUID = "6e400003-b5a3-f393-e0a9-e50e24dcca9e"
IDENTITY_CHAR_UUID = "6e400004-b5a3-f393-e0a9-e50e24dcca9e"

MAX_FRAGMENT_SIZE = 185
CONNECTION_TIMEOUT = 10.0  # seconds
ADAPTIVE_MIN_RSSI = -85
ADAPTIVE_LOW_DENSITY_THRESHOLD = 10
ADAPTIVE_HIGH_DENSITY_THRESHOLD = 50
ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE = 6
ADAPTIVE_COOLDOWN_PER_PERIPHERAL = 30.0  # seconds
PEER_LOST_TIMEOUT = 30.0  # seconds since last seen
SCAN_RESTART_INTERVAL = 30.0


class BleManager(TransportManager):
    """BLE transport — central (scanner) role.

    Discovers nearby mesh peripherals advertising ``SERVICE_UUID``, connects
    to them, reads their device ID, subscribes to the message characteristic,
    and forwards incoming BLE fragments to the Rust protocol core.

    Outgoing fragments are sent event-driven: the protocol calls
    ``BleTransportCallback.on_fragments_available()`` which triggers
    :meth:`_drain_outgoing_fragments`.

    Parameters
    ----------
    protocol:
        The ``OfflineProtocol`` UniFFI instance.
    device_id:
        Local device identifier.
    """

    transport_id = "ble"
    transport_name = "Bluetooth Low Energy"

    def __init__(self, protocol: Any, device_id: str) -> None:
        super().__init__()
        self._protocol = protocol
        self._device_id = device_id

        # Scanner
        self._scanner: BleakScanner | None = None
        self._scan_task: asyncio.Task[None] | None = None

        # Connected peers: address -> BleakClient
        self._clients: dict[str, BleakClient] = {}
        # address -> device_id mapping
        self._peer_device_ids: dict[str, str] = {}
        # address -> last-seen timestamp
        self._last_seen: dict[str, float] = {}
        # address -> last connection attempt timestamp (rate limiting)
        self._connection_attempts: dict[str, float] = {}
        # Global connection attempt timestamps (rate limiting)
        self._global_attempts: list[float] = []
        # Addresses currently being connected to (prevent concurrent attempts)
        self._connecting: set[str] = set()

        # Metrics
        self._bytes_sent: int = 0
        self._bytes_received: int = 0
        self._fragments_sent: int = 0
        self._fragments_received: int = 0

        # Tasks
        self._peer_cleanup_task: asyncio.Task[None] | None = None

    # -- TransportManager interface -------------------------------------------

    def is_available(self) -> bool:
        # bleak should be importable; actual adapter availability is checked
        # when scanning starts.
        return True

    async def start(self) -> None:
        if self._state == TransportState.RUNNING:
            raise TransportError("BLE transport is already running")

        self._update_state(TransportState.STARTING)
        self._emit_diagnostic("info", "Starting BLE transport", {
            "device_id": self._device_id,
        })

        try:
            self._scanner = BleakScanner(
                detection_callback=self._on_advertisement,
                service_uuids=[SERVICE_UUID],
            )
            await self._scanner.start()
        except Exception as exc:
            self._update_state(TransportState.STOPPED)
            raise TransportError(f"BLE scan start failed: {exc}") from exc

        # Notify protocol
        try:
            self._protocol.ble_status_changed(is_available=True)
        except Exception:
            pass

        self._update_state(TransportState.RUNNING)

        # Start background tasks
        self._peer_cleanup_task = asyncio.ensure_future(
            self._peer_cleanup_loop()
        )

        self._emit_diagnostic("info", "BLE transport started")

    async def stop(self) -> None:
        if self._state not in (TransportState.RUNNING, TransportState.STARTING):
            return

        self._update_state(TransportState.STOPPING)

        # Stop scanner
        if self._scanner is not None:
            try:
                await self._scanner.stop()
            except Exception:
                pass
            self._scanner = None

        # Cancel background tasks
        if self._peer_cleanup_task is not None and not self._peer_cleanup_task.done():
            self._peer_cleanup_task.cancel()
        self._peer_cleanup_task = None

        # Disconnect all clients
        for addr, client in list(self._clients.items()):
            try:
                await client.disconnect()
            except Exception:
                pass
        self._clients.clear()
        self._peer_device_ids.clear()
        self._last_seen.clear()
        self._connecting.clear()

        try:
            self._protocol.ble_status_changed(is_available=False)
        except Exception:
            pass

        self._update_state(TransportState.STOPPED)
        self._emit_diagnostic("info", "BLE transport stopped")

    def get_metrics(self) -> dict[str, Any]:
        return {
            "bytes_sent": self._bytes_sent,
            "bytes_received": self._bytes_received,
            "fragments_sent": self._fragments_sent,
            "fragments_received": self._fragments_received,
            "connected_peers": len(self._clients),
            "discovered_peers": len(self._last_seen),
        }

    # -- scanning / discovery -------------------------------------------------

    def _on_advertisement(
        self, device: BLEDevice, adv_data: AdvertisementData
    ) -> None:
        """Called by bleak for each BLE advertisement detected."""
        addr = device.address
        rssi = adv_data.rssi if adv_data.rssi is not None else -100

        # Filter weak signals
        if rssi < ADAPTIVE_MIN_RSSI:
            return

        now = time.monotonic()
        self._last_seen[addr] = now

        # Notify protocol of discovery (using address as provisional peer ID
        # until we read the device ID characteristic)
        peer_id = self._peer_device_ids.get(addr, addr)
        try:
            self._protocol.ble_peer_discovered(peer_id=peer_id, rssi=rssi)
        except Exception:
            pass

        # Attempt connection if not already connected / connecting
        if addr not in self._clients and addr not in self._connecting:
            if self._should_connect(addr, now):
                asyncio.ensure_future(self._connect_to_peer(device))

    def _should_connect(self, addr: str, now: float) -> bool:
        """Adaptive rate-limiting (mirrors iOS adaptive scan logic)."""
        # Per-peripheral cooldown
        last_attempt = self._connection_attempts.get(addr, 0)
        if now - last_attempt < ADAPTIVE_COOLDOWN_PER_PERIPHERAL:
            return False

        # Global rate limiting
        cutoff = now - 60.0
        self._global_attempts = [t for t in self._global_attempts if t > cutoff]
        if len(self._global_attempts) >= ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE:
            return False

        return True

    # -- connection management ------------------------------------------------

    async def _connect_to_peer(self, device: BLEDevice) -> None:
        addr = device.address
        self._connecting.add(addr)
        now = time.monotonic()
        self._connection_attempts[addr] = now
        self._global_attempts.append(now)

        try:
            client = BleakClient(
                device,
                timeout=CONNECTION_TIMEOUT,
                disconnected_callback=lambda c: asyncio.ensure_future(
                    self._on_peer_disconnected(addr)
                ),
            )
            await client.connect()

            # Read device ID characteristic
            device_id = await self._read_device_id(client)
            if device_id is None:
                await client.disconnect()
                return

            self._clients[addr] = client
            self._peer_device_ids[addr] = device_id

            # Re-notify protocol with the real device ID
            try:
                rssi = -70  # approximate; bleak doesn't expose RSSI post-connect
                self._protocol.ble_peer_discovered(peer_id=device_id, rssi=rssi)
            except Exception:
                pass

            # Subscribe to message notifications
            await self._subscribe_to_messages(client, addr, device_id)

            self._emit_diagnostic("info", "Connected to peer", {
                "address": addr,
                "device_id": device_id,
            })

        except Exception as exc:
            self._emit_diagnostic("debug", f"Connection to {addr} failed: {exc}")
        finally:
            self._connecting.discard(addr)

    async def _read_device_id(self, client: BleakClient) -> str | None:
        """Read the device ID characteristic from a connected peripheral."""
        try:
            data = await client.read_gatt_char(DEVICE_ID_CHAR_UUID)
            return data.decode("utf-8").strip("\x00")
        except Exception as exc:
            self._emit_diagnostic("debug", f"Failed to read device ID: {exc}")
            return None

    async def _subscribe_to_messages(
        self, client: BleakClient, addr: str, device_id: str
    ) -> None:
        """Subscribe to the message characteristic for incoming fragments."""
        try:
            def on_notification(_sender: int, data: bytearray) -> None:
                self._on_fragment_received(device_id, bytes(data))

            await client.start_notify(MESSAGE_CHAR_UUID, on_notification)
        except Exception as exc:
            self._emit_diagnostic(
                "warning", f"Failed to subscribe to messages from {device_id}: {exc}"
            )

    def _on_fragment_received(self, sender_id: str, fragment: bytes) -> None:
        """Handle an incoming BLE fragment from a peer."""
        self._bytes_received += len(fragment)
        self._fragments_received += 1
        try:
            self._protocol.ble_fragment_received(
                sender_id=sender_id, fragment=list(fragment)
            )
        except Exception as exc:
            self._emit_diagnostic("error", f"Error feeding fragment to protocol: {exc}")

    async def _on_peer_disconnected(self, addr: str) -> None:
        """Called when a peer disconnects."""
        device_id = self._peer_device_ids.pop(addr, addr)
        self._clients.pop(addr, None)

        try:
            self._protocol.ble_peer_lost(peer_id=device_id)
        except Exception:
            pass

        self._emit_diagnostic("info", f"Peer disconnected: {device_id}")

    # -- outgoing fragment handling -------------------------------------------

    def on_fragments_available(self) -> None:
        """Called by ``BleTransportCallback.on_fragments_available()``.

        Drains outgoing fragments from the protocol core and writes them
        to connected peers via GATT.
        """
        asyncio.ensure_future(self._drain_outgoing_fragments())

    async def _drain_outgoing_fragments(self) -> None:
        """Poll and send all queued outgoing fragments."""
        while True:
            try:
                frag = self._protocol.ble_get_next_fragment()
            except Exception:
                break
            if frag is None:
                break

            recipient = frag.recipient_id
            data = bytes(frag.data)

            # Find the client for this recipient
            client = self._find_client_for_peer(recipient)
            if client is not None and client.is_connected:
                try:
                    await client.write_gatt_char(
                        MESSAGE_CHAR_UUID,
                        data,
                        response=False,
                    )
                    self._bytes_sent += len(data)
                    self._fragments_sent += 1
                except Exception as exc:
                    self._emit_diagnostic(
                        "warning", f"Failed to send fragment to {recipient}: {exc}"
                    )

            # Return fragment to pool regardless of send outcome
            try:
                self._protocol.ble_return_fragment()
            except Exception:
                pass

    def _find_client_for_peer(self, peer_id: str) -> BleakClient | None:
        """Find the BleakClient for a given device ID."""
        for addr, did in self._peer_device_ids.items():
            if did == peer_id:
                return self._clients.get(addr)
        return None

    # -- background tasks -----------------------------------------------------

    async def _peer_cleanup_loop(self) -> None:
        """Periodically remove stale peers that haven't been seen recently."""
        try:
            while True:
                await asyncio.sleep(PEER_LOST_TIMEOUT / 2)
                now = time.monotonic()
                stale = [
                    addr
                    for addr, ts in self._last_seen.items()
                    if now - ts > PEER_LOST_TIMEOUT
                    and addr not in self._clients
                ]
                for addr in stale:
                    self._last_seen.pop(addr, None)
                    device_id = self._peer_device_ids.pop(addr, addr)
                    try:
                        self._protocol.ble_peer_lost(peer_id=device_id)
                    except Exception:
                        pass
        except asyncio.CancelledError:
            return
