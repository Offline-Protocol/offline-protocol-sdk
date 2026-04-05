"""BLE transport manager — peripheral (advertiser/GATT server) role.

Complements :class:`BleManager` (central/scanner role) by advertising a
GATT service so that phones and hardware devices can discover this desktop
node, connect, and exchange mesh messages over Bluetooth Low Energy.

Uses the ``bless`` library for GATT server functionality.  On macOS this
is backed by CoreBluetooth; on Linux by BlueZ/D-Bus.

Typical usage::

    peripheral = BlePeripheral(protocol, device_id="coordinator")
    await peripheral.start()   # begins advertising
    # phones now discover "coordinator" and connect
    await peripheral.stop()
"""

from __future__ import annotations

import asyncio
import json
import logging
import sys
import threading
import time
from typing import Any

from bless import (
    BlessServer,
    GATTAttributePermissions,
    GATTCharacteristicProperties,
)

from .ble_manager import (
    DEVICE_ID_CHAR_UUID,
    IDENTITY_CHAR_UUID,
    MESSAGE_CHAR_UUID,
    SERVICE_UUID,
)
from .transport_manager import TransportError, TransportManager, TransportState

logger = logging.getLogger(__name__)

# Inter-frame delay when sending multiple notification fragments (matches
# the 5 ms delay used by the M5StickC firmware in ble_mesh.h).
_INTER_FRAME_DELAY = 0.005

# How often to poll the bless delegate for subscription changes.
_PEER_MONITOR_INTERVAL = 1.0


class BlePeripheral(TransportManager):
    """BLE transport — peripheral (advertiser / GATT server) role.

    Advertises a GATT service with the standard Offline Protocol BLE UUIDs,
    accepts connections from phones and hardware, receives incoming fragments
    via GATT write requests, and sends outgoing fragments via notifications.

    The peripheral passes raw fragment bytes through to the
    :class:`OfflineProtocol` Rust core — the core handles reassembly and
    routing internally.

    Parameters
    ----------
    protocol:
        The ``OfflineProtocol`` UniFFI instance (shared with ``BleManager``).
    device_id:
        Device identifier written to the DEVICE_ID characteristic.  Phones
        that read ``"coordinator"`` auto-detect a training cluster.
    max_connections:
        Maximum number of simultaneous central connections (default 4).
    identity_json:
        Optional JSON string for the IDENTITY characteristic.  If ``None``,
        a default is generated from *device_id*.
    """

    transport_id = "ble-peripheral"
    transport_name = "Bluetooth Low Energy (Peripheral)"

    def __init__(
        self,
        protocol: Any,
        device_id: str,
        *,
        max_connections: int = 4,
        identity_json: str | None = None,
    ) -> None:
        super().__init__()
        self._protocol = protocol
        self._device_id = device_id
        self._max_connections = max_connections

        # Identity characteristic payload
        if identity_json is not None:
            self._identity_bytes = identity_json.encode("utf-8")
        else:
            self._identity_bytes = json.dumps({
                "device_id": device_id,
                "role": "coordinator",
                "protocol": "offline-protocol",
                "version": "0.1.0",
            }).encode("utf-8")

        # Server (created on start)
        self._server: BlessServer | None = None

        # Connected centrals: uuid -> connection timestamp
        self._connected_centrals: dict[str, float] = {}

        # Central UUID -> resolved user_id mapping.  Populated when the
        # protocol core emits a message_received event containing a
        # ``sender`` field.  Used to re-register the peer with its real
        # user_id so the Rust BLE transport can route outgoing messages.
        self._central_to_user_id: dict[str, str] = {}

        # Event loop — captured in start() for thread-safe callback scheduling
        self._loop: asyncio.AbstractEventLoop | None = None

        # Lock protecting metrics — _on_write runs on bless's delegate thread
        self._lock = threading.Lock()

        # Metrics
        self._bytes_sent: int = 0
        self._bytes_received: int = 0
        self._fragments_sent: int = 0
        self._fragments_received: int = 0
        self._is_advertising: bool = False

        # Serialisation flag — prevents concurrent _drain_outgoing_fragments
        self._draining: bool = False

        # Background tasks
        self._peer_monitor_task: asyncio.Task[None] | None = None

    # -- TransportManager interface -------------------------------------------

    def is_available(self) -> bool:
        """Return True on platforms where bless is supported (macOS, Linux)."""
        return sys.platform in ("darwin", "linux")

    async def start(self) -> None:
        """Set up the GATT server, register characteristics, and advertise."""
        if self._state == TransportState.RUNNING:
            raise TransportError("BLE peripheral is already running")

        if not self.is_available():
            raise TransportError(
                f"BLE peripheral not available on {sys.platform}"
            )

        self._loop = asyncio.get_running_loop()
        self._update_state(TransportState.STARTING)
        self._emit_diagnostic("info", "Starting BLE peripheral", {
            "device_id": self._device_id,
        })

        try:
            # Truncate name — CoreBluetooth limits advertisement data to ~28
            # bytes, and service UUIDs also consume space.
            name = self._device_id[:10]
            self._server = BlessServer(name=name)

            await self._setup_gatt_server()

            # Register read/write callbacks
            self._server.read_request_func = self._on_read
            self._server.write_request_func = self._on_write

            # Start advertising — prioritize service UUID over local name so
            # phones can discover us via SERVICE_UUID scanning.
            await self._server.start(prioritize_local_name=False)
            self._is_advertising = True
        except Exception as exc:
            self._update_state(TransportState.STOPPED)
            raise TransportError(
                f"BLE peripheral start failed: {exc}"
            ) from exc

        # Notify protocol
        try:
            self._protocol.ble_status_changed(is_available=True)
        except Exception:
            logger.debug("ble_status_changed(True) failed", exc_info=True)

        self._update_state(TransportState.RUNNING)

        # Start background peer monitor
        self._peer_monitor_task = asyncio.ensure_future(
            self._peer_monitor_loop()
        )

        self._emit_diagnostic("info", "BLE peripheral started — advertising")

    async def stop(self) -> None:
        """Stop advertising and disconnect all centrals."""
        if self._state not in (TransportState.RUNNING, TransportState.STARTING):
            return

        self._update_state(TransportState.STOPPING)

        # Cancel background tasks
        if self._peer_monitor_task is not None and not self._peer_monitor_task.done():
            self._peer_monitor_task.cancel()
            try:
                await self._peer_monitor_task
            except asyncio.CancelledError:
                pass
        self._peer_monitor_task = None

        # Stop GATT server
        if self._server is not None:
            try:
                await self._server.stop()
            except Exception:
                pass
            self._server = None
        self._is_advertising = False

        # Notify protocol about all lost peers (both raw UUID and resolved user_id)
        for central_uuid in list(self._connected_centrals):
            try:
                self._protocol.ble_peer_lost(peer_id=central_uuid)
            except Exception:
                logger.debug("ble_peer_lost failed for %s", central_uuid)
            resolved_uid = self._central_to_user_id.get(central_uuid)
            if resolved_uid:
                try:
                    self._protocol.ble_peer_lost(peer_id=resolved_uid)
                except Exception:
                    logger.debug("ble_peer_lost failed for resolved %s", resolved_uid)
        self._connected_centrals.clear()
        self._central_to_user_id.clear()

        try:
            self._protocol.ble_status_changed(is_available=False)
        except Exception:
            logger.debug("ble_status_changed(False) failed", exc_info=True)

        self._update_state(TransportState.STOPPED)
        self._emit_diagnostic("info", "BLE peripheral stopped")

    def get_metrics(self) -> dict[str, Any]:
        with self._lock:
            return {
                "bytes_sent": self._bytes_sent,
                "bytes_received": self._bytes_received,
                "fragments_sent": self._fragments_sent,
                "fragments_received": self._fragments_received,
                "connected_centrals": len(self._connected_centrals),
                "is_advertising": self._is_advertising,
            }

    # -- GATT server setup ----------------------------------------------------

    async def _setup_gatt_server(self) -> None:
        """Add the mesh service and its characteristics."""
        assert self._server is not None

        await self._server.add_new_service(SERVICE_UUID)

        # MESSAGE characteristic — bidirectional fragment transport
        # NOTE: ``write`` (with-response) is required on macOS because the
        # CoreBluetooth ``peripheralManager:didReceiveWriteRequests:``
        # delegate callback is only invoked for ATT Write Requests, **not**
        # ATT Write Commands (write-without-response).  We keep both so
        # peers that prefer write-without-response still see the property.
        msg_props = (
            GATTCharacteristicProperties.write
            | GATTCharacteristicProperties.write_without_response
            | GATTCharacteristicProperties.notify
        )
        msg_perms = (
            GATTAttributePermissions.writeable
            | GATTAttributePermissions.readable
        )
        await self._server.add_new_characteristic(
            SERVICE_UUID,
            MESSAGE_CHAR_UUID,
            msg_props,
            None,
            msg_perms,
        )

        # DEVICE_ID characteristic — readable identity
        await self._server.add_new_characteristic(
            SERVICE_UUID,
            DEVICE_ID_CHAR_UUID,
            GATTCharacteristicProperties.read,
            bytearray(self._device_id.encode("utf-8")),
            GATTAttributePermissions.readable,
        )

        # IDENTITY characteristic — readable JSON metadata
        await self._server.add_new_characteristic(
            SERVICE_UUID,
            IDENTITY_CHAR_UUID,
            GATTCharacteristicProperties.read,
            bytearray(self._identity_bytes),
            GATTAttributePermissions.readable,
        )

    # -- GATT callbacks -------------------------------------------------------

    @staticmethod
    def _resolve_char_uuid(char_uuid: Any) -> str:
        """Extract a lowercase UUID string from a characteristic or string.

        ``bless`` may pass either a plain UUID string or a
        ``BlessGATTCharacteristic*`` object depending on version/platform.
        """
        if isinstance(char_uuid, str):
            return char_uuid.lower()
        # BlessGATTCharacteristic objects expose a .uuid property (str)
        if hasattr(char_uuid, "uuid"):
            return str(char_uuid.uuid).lower()
        # CoreBluetooth-backed objects — try ObjC CBCharacteristic.UUID()
        if hasattr(char_uuid, "UUID"):
            return str(char_uuid.UUID().UUIDString()).lower()
        return str(char_uuid).lower()

    def _on_read(self, char_uuid: Any) -> bytearray:
        """Handle incoming GATT read requests."""
        uuid = self._resolve_char_uuid(char_uuid)
        if uuid == DEVICE_ID_CHAR_UUID.lower():
            return bytearray(self._device_id.encode("utf-8"))
        if uuid == IDENTITY_CHAR_UUID.lower():
            return bytearray(self._identity_bytes)
        return bytearray(b"")

    def _on_write(self, char_uuid: Any, value: Any) -> None:
        """Handle incoming GATT write requests (fragments from phones).

        This runs inside the bless/CoreBluetooth delegate callback.  Any
        unhandled exception here prevents ``respondToRequest:withResult:``
        from being called, which causes CoreBluetooth to **drop the
        connection**.  Therefore the entire body is wrapped in try/except.
        """
        try:
            resolved = self._resolve_char_uuid(char_uuid)
            logger.debug(
                "_on_write called: uuid=%s, value_type=%s, value_len=%d",
                resolved, type(value).__name__, len(value) if value else 0,
            )
            if resolved != MESSAGE_CHAR_UUID.lower():
                logger.debug("_on_write: ignoring write to %s", resolved)
                return

            fragment = bytes(value)
            logger.debug(
                "_on_write: %d bytes, sender=%s",
                len(fragment), self._resolve_sender(),
            )
            with self._lock:
                self._bytes_received += len(fragment)
                self._fragments_received += 1

            sender_id = self._resolve_sender()

            self._protocol.ble_fragment_received(
                sender_id=sender_id, fragment=list(fragment)
            )
            logger.debug("_on_write: fragment fed to protocol OK")
        except Exception as exc:
            logger.exception("_on_write failed: %s", exc)

    def _resolve_sender(self) -> str:
        """Best-effort identification of the writing central.

        Returns a stable sender ID so that the Rust core can group
        fragments from the same source.  If the peer monitor hasn't
        detected the subscription yet (``_connected_centrals`` is empty),
        fall back to the last known central UUID.  This prevents the
        sender flipping between ``"ble-peer"`` and a UUID within the
        same message's fragments, which breaks reassembly.
        """
        if len(self._connected_centrals) == 1:
            central = next(iter(self._connected_centrals))
            self._last_known_central = central
            return central
        if len(self._connected_centrals) > 1:
            return "ble-peer"
        # No centrals currently tracked — use last known if available
        return getattr(self, '_last_known_central', 'ble-peer')

    def resolve_sender_identity(self, sender_user_id: str) -> None:
        """Associate a sender's user_id with the currently connected central.

        Called by ``ProtocolManager`` when a reassembled message reveals the
        sender's real user_id (from the message ``sender`` field).  The BLE
        central (scanner) path learns the peer's user_id by reading the
        DEVICE_ID characteristic; the peripheral has no equivalent mechanism,
        so it relies on the first received message to learn the mapping.

        Once the mapping is established, the peer is re-registered with the
        Rust core under its real user_id so that ``send_message(user_id, ...)``
        can route back through the BLE peripheral.
        """
        if not self._connected_centrals:
            return

        # With one central connected the mapping is unambiguous.
        # With multiple, we can't reliably attribute — skip.
        if len(self._connected_centrals) != 1:
            logger.debug(
                "resolve_sender_identity: %d centrals connected, skipping",
                len(self._connected_centrals),
            )
            return

        central_uuid = next(iter(self._connected_centrals))

        # Avoid redundant re-registration
        if self._central_to_user_id.get(central_uuid) == sender_user_id:
            return

        self._central_to_user_id[central_uuid] = sender_user_id
        logger.info(
            "Resolved central %s -> user_id %s — re-registering peer",
            central_uuid, sender_user_id,
        )

        try:
            self._protocol.ble_peer_discovered(
                peer_id=sender_user_id, rssi=-50
            )
        except Exception:
            logger.debug(
                "ble_peer_discovered failed for resolved user_id %s",
                sender_user_id,
            )

    # -- outgoing fragment handling -------------------------------------------

    def on_fragments_available(self) -> None:
        """Called by ``BleTransportCallback.on_fragments_available()``.

        Drains outgoing fragments from the protocol core and sends them
        to subscribed centrals via GATT notifications.

        This is invoked from a Rust UniFFI callback thread, so we must use
        ``call_soon_threadsafe`` to schedule work on the asyncio event loop.
        """
        loop = self._loop
        if loop is not None and not loop.is_closed() and loop.is_running():
            loop.call_soon_threadsafe(asyncio.ensure_future, self._drain_outgoing_fragments())

    async def _drain_outgoing_fragments(self) -> None:
        """Send all queued outgoing fragments as notifications."""
        if self._server is None:
            return
        if self._draining:
            return
        self._draining = True
        try:
            await self._drain_outgoing_fragments_inner()
        finally:
            self._draining = False

    async def _drain_outgoing_fragments_inner(self) -> None:
        if self._server is None:
            return

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

            data = bytes(frag.data)

            char = self._server.get_characteristic(MESSAGE_CHAR_UUID)
            if char is not None:
                char.value = data
                self._server.update_value(SERVICE_UUID, MESSAGE_CHAR_UUID)
                with self._lock:
                    self._bytes_sent += len(data)
                    self._fragments_sent += 1

                # Inter-frame delay to avoid overwhelming the BLE stack
                await asyncio.sleep(_INTER_FRAME_DELAY)

            # Return fragment to pool regardless of send outcome
            try:
                self._protocol.ble_return_fragment()
            except Exception:
                logger.debug("ble_return_fragment failed", exc_info=True)

    # -- background tasks -----------------------------------------------------

    async def _peer_monitor_loop(self) -> None:
        """Poll the bless delegate's subscription dict for changes.

        When a central subscribes to the MESSAGE characteristic we treat it
        as a peer connection; when it unsubscribes, a disconnection.
        """
        known: set[str] = set()
        try:
            while True:
                await asyncio.sleep(_PEER_MONITOR_INTERVAL)

                if self._server is None:
                    continue

                try:
                    delegate = self._server.peripheral_manager_delegate
                    # NOTE: _central_subscriptions is a bless-internal dict.
                    # Validated against bless 0.3.x. If this attribute is
                    # missing after a bless upgrade, the except below logs a
                    # warning and the monitor degrades gracefully (no peer
                    # tracking, but data transfer still works).
                    current = set(delegate._central_subscriptions.keys())
                except (AttributeError, TypeError):
                    logger.debug(
                        "Cannot read bless _central_subscriptions — "
                        "peer monitoring disabled (bless API may have changed)"
                    )
                    continue

                # New centrals
                for central_uuid in current - known:
                    if len(self._connected_centrals) >= self._max_connections:
                        self._emit_diagnostic(
                            "warning",
                            f"Max connections ({self._max_connections}) reached, "
                            f"ignoring {central_uuid}",
                        )
                        continue

                    self._connected_centrals[central_uuid] = time.monotonic()
                    try:
                        self._protocol.ble_peer_discovered(
                            peer_id=central_uuid, rssi=-50
                        )
                    except Exception:
                        logger.debug("ble_peer_discovered failed for %s", central_uuid)
                    self._emit_diagnostic(
                        "info", "Central connected", {"central": central_uuid}
                    )

                # Lost centrals
                for central_uuid in known - current:
                    self._connected_centrals.pop(central_uuid, None)
                    # Notify peer lost for both the raw UUID and the
                    # resolved user_id (if any) so the Rust core removes
                    # both routing entries.
                    resolved_uid = self._central_to_user_id.pop(central_uuid, None)
                    try:
                        self._protocol.ble_peer_lost(peer_id=central_uuid)
                    except Exception:
                        logger.debug("ble_peer_lost failed for %s", central_uuid)
                    if resolved_uid:
                        try:
                            self._protocol.ble_peer_lost(peer_id=resolved_uid)
                        except Exception:
                            logger.debug("ble_peer_lost failed for resolved %s", resolved_uid)
                    self._emit_diagnostic(
                        "info", "Central disconnected", {"central": central_uuid}
                    )

                known = current.copy()

                # Update cached advertising state
                try:
                    self._is_advertising = self._server.peripheral_manager_delegate.is_advertising()
                except Exception:
                    pass

        except asyncio.CancelledError:
            return
