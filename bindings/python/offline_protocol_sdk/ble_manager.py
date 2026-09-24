"""BLE transport manager using the ``bleak`` library.

Mirrors the central (scanner) role from ``BleManager.swift`` /
``BleManager.kt``.  Connects to nearby mesh peers over Bluetooth Low
Energy, verifies their identity, subscribes to message notifications, and
drives the UniFFI ``OfflineProtocol`` BLE transport methods.

A peer id handed to the core is a proved address, never a label. The
central reads the peer's Identity characteristic, hands the bytes to the
core's ``verify_identity_assertion`` (the one verifier of that layout), and
compares the address it returns to the Device id characteristic, exactly.
Only then is the peer announced, and it is announced under the derived
address rather than the string it served. A link whose peer cannot be
verified is disconnected and never announced, not even under its Bluetooth
address: an unverifiable identity and an absent one are the same thing.

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
import threading
import time
from typing import Any

from bleak import BleakClient, BleakScanner
from bleak.backends.device import BLEDevice
from bleak.backends.scanner import AdvertisementData

from .offline_protocol import verify_identity_assertion
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
# Ceiling on the per-peripheral cooldown after repeated identity refusals. A
# peer that cannot prove its address (an old peripheral, a phone before MLS
# init) fails the same way every time, and without a backoff each retry spends
# the global per-minute connection budget that verifiable peers need.
REFUSAL_BACKOFF_MAX = 600.0  # seconds
PEER_LOST_TIMEOUT = 30.0  # seconds since last seen
SCAN_RESTART_INTERVAL = 30.0


class BleManager(TransportManager):
    """BLE transport — central (scanner) role.

    Discovers nearby mesh peripherals advertising ``SERVICE_UUID``, connects
    to them, verifies the address they claim, subscribes to the message
    characteristic, and forwards incoming BLE fragments to the Rust protocol
    core.

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

        # Lock protecting shared state accessed from bleak's scanner thread
        # (_on_advertisement) AND the asyncio event loop. Guards: _last_seen,
        # _connection_attempts, _global_attempts, _connecting, _clients,
        # _peer_device_ids, _refusals.
        self._lock = threading.Lock()

        # Connected peers: address -> BleakClient
        self._clients: dict[str, BleakClient] = {}
        # address -> device_id mapping
        self._peer_device_ids: dict[str, str] = {}
        # device_id -> address reverse lookup (avoids linear scan)
        self._device_id_to_addr: dict[str, str] = {}
        # address -> last-seen timestamp
        self._last_seen: dict[str, float] = {}
        # address -> last connection attempt timestamp (rate limiting)
        self._connection_attempts: dict[str, float] = {}
        # Global connection attempt timestamps (rate limiting)
        self._global_attempts: list[float] = []
        # Addresses currently being connected to (prevent concurrent attempts)
        self._connecting: set[str] = set()
        # address -> consecutive identity refusals; each doubles the cooldown
        self._refusals: dict[str, int] = {}

        # Metrics
        self._bytes_sent: int = 0
        self._bytes_received: int = 0
        self._fragments_sent: int = 0
        self._fragments_received: int = 0

        # Event loop — captured in start() for thread-safe callback scheduling
        self._loop: asyncio.AbstractEventLoop | None = None

        # Serialisation flag — prevents concurrent _drain_outgoing_fragments
        self._draining: bool = False

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

        self._loop = asyncio.get_running_loop()
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
            logger.debug("ble_status_changed(True) failed", exc_info=True)

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
        with self._lock:
            clients_snapshot = list(self._clients.items())
        for addr, client in clients_snapshot:
            try:
                await client.disconnect()
            except Exception:
                pass
        with self._lock:
            self._clients.clear()
            self._peer_device_ids.clear()
            self._device_id_to_addr.clear()
            self._last_seen.clear()
            self._connecting.clear()

        try:
            self._protocol.ble_status_changed(is_available=False)
        except Exception:
            logger.debug("ble_status_changed(False) failed")

        self._update_state(TransportState.STOPPED)
        self._emit_diagnostic("info", "BLE transport stopped")

    def get_metrics(self) -> dict[str, Any]:
        with self._lock:
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
        """Called by bleak for each BLE advertisement detected.

        NOTE: This is invoked from bleak's scanner thread, not the asyncio
        event loop.  All shared-state mutations go through ``_lock``.
        """
        addr = device.address
        rssi = adv_data.rssi if adv_data.rssi is not None else -100

        # Filter weak signals
        if rssi < ADAPTIVE_MIN_RSSI:
            return

        now = time.monotonic()
        should_connect = False

        with self._lock:
            self._last_seen[addr] = now

            # A peer is announced only under an address this link proved.
            # Before that there is nothing to announce: a Bluetooth address
            # is a label, and the core keys routes by what it is handed.
            peer_id = self._peer_device_ids.get(addr)

            # Attempt connection if not already connected / connecting.
            if addr not in self._clients and addr not in self._connecting:
                if self._should_connect_locked(addr, now):
                    self._connecting.add(addr)
                    should_connect = True

        # Protocol calls outside the lock to avoid holding it during FFI
        if peer_id is not None:
            try:
                self._protocol.ble_peer_discovered(peer_id=peer_id, rssi=rssi)
            except Exception:
                logger.debug("ble_peer_discovered failed for %s", peer_id)

        if should_connect:
            loop = self._loop
            if loop is not None and not loop.is_closed() and loop.is_running():
                loop.call_soon_threadsafe(
                    asyncio.ensure_future,
                    self._connect_to_peer(device),
                )
            else:
                with self._lock:
                    self._connecting.discard(addr)

    def _should_connect_locked(self, addr: str, now: float) -> bool:
        """Adaptive rate-limiting. Caller must hold ``_lock``."""
        # Per-peripheral cooldown
        last_attempt = self._connection_attempts.get(addr, 0)
        if now - last_attempt < self._cooldown_locked(addr):
            return False

        # Global rate limiting
        cutoff = now - 60.0
        self._global_attempts = [t for t in self._global_attempts if t > cutoff]
        if len(self._global_attempts) >= ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE:
            return False

        return True

    def _cooldown_locked(self, addr: str) -> float:
        """Per-peripheral cooldown, doubled per consecutive identity refusal
        and capped at ``REFUSAL_BACKOFF_MAX``. Caller must hold ``_lock``."""
        refusals = self._refusals.get(addr, 0)
        if refusals == 0:
            return ADAPTIVE_COOLDOWN_PER_PERIPHERAL
        return min(
            ADAPTIVE_COOLDOWN_PER_PERIPHERAL * (2 ** min(refusals, 16)),
            REFUSAL_BACKOFF_MAX,
        )

    def _record_refusal(self, addr: str) -> None:
        with self._lock:
            self._refusals[addr] = self._refusals.get(addr, 0) + 1

    # -- connection management ------------------------------------------------

    async def _connect_to_peer(self, device: BLEDevice) -> None:
        addr = device.address
        # addr is already in self._connecting (added in _on_advertisement)
        now = time.monotonic()
        with self._lock:
            self._connection_attempts[addr] = now
            self._global_attempts.append(now)

        try:
            client = BleakClient(
                device,
                timeout=CONNECTION_TIMEOUT,
                disconnected_callback=lambda c, _addr=addr: (
                    self._loop.call_soon_threadsafe(
                        asyncio.ensure_future,
                        self._on_peer_disconnected(_addr),
                    )
                    if self._loop is not None
                    and not self._loop.is_closed()
                    and self._loop.is_running()
                    else None
                ),
            )
            await client.connect()

            # The claim, then the proof. Both reads must succeed and the
            # proof must name the claim, or the link is dropped unannounced.
            claimed = await self._read_device_id(client)
            if claimed is None:
                self._record_refusal(addr)
                await client.disconnect()
                return
            identity = await self._read_identity(client)
            device_id = self._verify_peer(addr, claimed, identity)
            if device_id is None:
                self._record_refusal(addr)
                await client.disconnect()
                return

            with self._lock:
                self._refusals.pop(addr, None)
                self._clients[addr] = client
                self._peer_device_ids[addr] = device_id
                self._device_id_to_addr[device_id] = addr

            # Announce the peer under the address it proved
            try:
                rssi = -70  # approximate; bleak doesn't expose RSSI post-connect
                self._protocol.ble_peer_discovered(peer_id=device_id, rssi=rssi)
            except Exception:
                logger.debug("ble_peer_discovered failed for %s", device_id)

            # Subscribe to message notifications
            await self._subscribe_to_messages(client, addr, device_id)

            self._emit_diagnostic("info", "Connected to peer", {
                "address": addr,
                "device_id": device_id,
            })

        except Exception as exc:
            self._emit_diagnostic("debug", f"Connection to {addr} failed: {exc}")
        finally:
            with self._lock:
                self._connecting.discard(addr)

    async def _read_device_id(self, client: BleakClient) -> str | None:
        """Read the device ID characteristic from a connected peripheral."""
        try:
            data = await client.read_gatt_char(DEVICE_ID_CHAR_UUID)
            return data.decode("utf-8").strip("\x00")
        except Exception as exc:
            self._emit_diagnostic("debug", f"Failed to read device ID: {exc}")
            return None

    async def _read_identity(self, client: BleakClient) -> bytes | None:
        """Read the Identity characteristic from a connected peripheral."""
        try:
            return bytes(await client.read_gatt_char(IDENTITY_CHAR_UUID))
        except Exception as exc:
            self._emit_diagnostic("debug", f"Failed to read identity: {exc}")
            return None

    def _verify_peer(
        self, addr: str, claimed: str, identity: bytes | None
    ) -> str | None:
        """Turn a claim and a proof into the one address to announce.

        Runs the four steps the Bluetooth LE framing chapter specifies. The
        first three (parse, verify the signature under the key, derive the
        address) are the core's, through ``verify_identity_assertion``; the
        fourth, the exact comparison against the Device id characteristic,
        is here because only this side read that characteristic. Every
        failure returns ``None`` and emits why, and the caller disconnects:
        there is no accept-but-flag state, because what this returns becomes
        ``Message.recipient`` on every outbound frame and the peer id the
        core matches ``Message.sender`` against.
        """
        if identity is None:
            self._emit_diagnostic("warning", "Peer served no identity", {
                "address": addr,
                "claimed": claimed,
            })
            return None
        try:
            derived = verify_identity_assertion(list(identity))
        except Exception as exc:
            self._emit_diagnostic("warning", "Peer identity did not verify", {
                "address": addr,
                "claimed": claimed,
                "error": str(exc),
            })
            return None
        if derived != claimed:
            # Exact, never normalised: a value differing only in case belongs
            # to a peer that did not derive its own id the way this one did.
            self._emit_diagnostic("warning", "Peer identity names another address", {
                "address": addr,
                "claimed": claimed,
                "derived": derived,
            })
            return None
        return derived

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
        with self._lock:
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
        with self._lock:
            device_id = self._peer_device_ids.pop(addr, None)
            self._clients.pop(addr, None)
            if device_id is not None:
                self._device_id_to_addr.pop(device_id, None)

        # A link that never proved a peer was never announced, so there is
        # nothing to report lost.
        if device_id is None:
            self._emit_diagnostic("debug", f"Unverified link closed: {addr}")
            return

        try:
            self._protocol.ble_peer_lost(peer_id=device_id)
        except Exception:
            logger.debug("ble_peer_lost failed for %s", device_id)

        self._emit_diagnostic("info", f"Peer disconnected: {device_id}")

    # -- outgoing fragment handling -------------------------------------------

    def on_fragments_available(self) -> None:
        """Called by ``BleTransportCallback.on_fragments_available()``.

        Drains outgoing fragments from the protocol core and writes them
        to connected peers via GATT.

        This is invoked from a Rust UniFFI callback thread, so we must use
        ``call_soon_threadsafe`` to schedule work on the asyncio event loop.
        """
        loop = self._loop
        if loop is not None and not loop.is_closed() and loop.is_running():
            loop.call_soon_threadsafe(asyncio.ensure_future, self._drain_outgoing_fragments())

    async def _drain_outgoing_fragments(self) -> None:
        """Poll and send all queued outgoing fragments."""
        if self._draining:
            return
        self._draining = True
        try:
            await self._drain_outgoing_fragments_inner()
        finally:
            self._draining = False

    async def _drain_outgoing_fragments_inner(self) -> None:
        while True:
            try:
                frag = self._protocol.ble_get_next_fragment()
            except (AttributeError, ReferenceError) as exc:
                logger.error("Protocol instance invalid: %s", exc)
                break
            except Exception as exc:
                logger.warning("Unexpected error getting next fragment: %s", exc)
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
                    with self._lock:
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
                logger.debug("ble_return_fragment failed", exc_info=True)

    def _find_client_for_peer(self, peer_id: str) -> BleakClient | None:
        """Find the BleakClient for a given device ID (O(1) via reverse map)."""
        with self._lock:
            addr = self._device_id_to_addr.get(peer_id)
            if addr is not None:
                return self._clients.get(addr)
            return None

    # -- background tasks -----------------------------------------------------

    async def _peer_cleanup_loop(self) -> None:
        """Periodically remove stale peers that haven't been seen recently."""
        try:
            while True:
                await asyncio.sleep(PEER_LOST_TIMEOUT / 2)
                now = time.monotonic()
                stale_device_ids: list[str] = []
                with self._lock:
                    stale = [
                        addr
                        for addr, ts in self._last_seen.items()
                        if now - ts > PEER_LOST_TIMEOUT
                        and addr not in self._clients
                    ]
                    for addr in stale:
                        self._last_seen.pop(addr, None)
                        self._refusals.pop(addr, None)
                        device_id = self._peer_device_ids.pop(addr, None)
                        if device_id is None:
                            continue  # never announced, nothing to report
                        self._device_id_to_addr.pop(device_id, None)
                        stale_device_ids.append(device_id)
                # Notify protocol outside the lock
                for device_id in stale_device_ids:
                    try:
                        self._protocol.ble_peer_lost(peer_id=device_id)
                    except Exception:
                        logger.debug("ble_peer_lost failed for %s", device_id)
        except asyncio.CancelledError:
            return
