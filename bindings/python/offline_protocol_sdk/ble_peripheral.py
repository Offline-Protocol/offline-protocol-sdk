"""BLE transport manager — peripheral (advertiser/GATT server) role.

Complements :class:`BleManager` (central/scanner role) by advertising a
GATT service so that phones and hardware devices can discover this desktop
node, connect, and exchange mesh messages over Bluetooth Low Energy.

Uses the ``bless`` library for GATT server functionality.  On macOS this
is backed by CoreBluetooth; on Linux by BlueZ/D-Bus.

Typical usage::

    peripheral = BlePeripheral(protocol, device_id="coordinator")
    await peripheral.start()   # begins advertising
    # phones now discover this device, read and verify its address, connect
    await peripheral.stop()

What a central reads from this peripheral is the pair the Bluetooth LE
framing chapter requires: the Device id characteristic carries this device's
``off1...`` address, and the Identity characteristic carries the identity
assertion the core builds over that address's key. A phone verifies the
second and compares it to the first, exactly, before it surfaces the peer.
Neither value exists before MLS has been initialised, so ``start()`` refuses
to advertise without an address rather than advertise something no central
can verify.
"""

from __future__ import annotations

import asyncio
import logging
import sys
import threading
import time
from typing import Any

# bless is not installed on Windows (pyproject.toml leaves it out there: it
# has no Windows backend), and this module is imported by the package's
# __init__, so a bare import here is `import offline_protocol_sdk` failing
# on Windows. The names are only reached from start(), behind
# is_available(), which answers False without them.
try:
    from bless import (
        BlessServer,
        GATTAttributePermissions,
        GATTCharacteristicProperties,
    )
except ImportError:  # pragma: no cover - the Windows wheel test covers it
    BlessServer = None  # type: ignore[assignment, misc]
    GATTAttributePermissions = None  # type: ignore[assignment, misc]
    GATTCharacteristicProperties = None  # type: ignore[assignment, misc]

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

_BLUEZ_SERVICE = "org.bluez"
_BLUEZ_DEVICE_IFACE = "org.bluez.Device1"
_BLUEZ_CHAR_IFACE = "org.bluez.GattCharacteristic1"


def _central_id_from_device_path(path: Any) -> str | None:
    """``/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`` -> ``AA:BB:CC:DD:EE:FF``."""
    if not isinstance(path, str):
        return None
    leaf = path.rsplit("/", 1)[-1]
    if not leaf.startswith("dev_") or len(leaf) <= 4:
        return None
    return leaf[4:].replace("_", ":")


def _device_connected_query(path: str) -> Any:
    """A ``Properties.Get(Device1, Connected)`` call for ``path``.

    dbus_next is imported here, not at module load: it is bless's Linux
    dependency and is absent everywhere else.
    """
    from dbus_next import Message  # type: ignore[import-not-found]

    return Message(
        destination=_BLUEZ_SERVICE,
        path=path,
        interface="org.freedesktop.DBus.Properties",
        member="Get",
        signature="ss",
        body=[_BLUEZ_DEVICE_IFACE, "Connected"],
    )


class _BlueZCentrals:
    """Which devices are centrals of this GATT server, on BlueZ.

    bless's BlueZ backend tracks none: its characteristic drops the
    ``options`` BlueZ passes to ``ReadValue`` and ``WriteValue``, and
    ``StartNotify`` carries no device at all. Those options name the
    calling device (``options["device"]``), so this observes the calls on
    the bus before bless dispatches them, through dbus_next's message
    handler hook, and never answers or alters one.

    A device is a central of ours once it has read or written one of this
    application's characteristics, and stops being one when BlueZ reports
    its ``Device1.Connected`` false or the object is gone. That is how a
    central is told apart from a peripheral our own central role connected
    to: both are ``Device1`` objects with ``Connected`` true, but only a
    central of ours calls into our application. Two limits follow. A
    central that subscribes and never reads or writes is not seen (ours
    always read the Device id and Identity first). And BlueZ reports a
    subscription per characteristic, not per device, so with several
    centrals connected a subscribed one cannot be named; ``connected()``
    lists the devices only while the Message characteristic has a
    subscriber.
    """

    def __init__(self, base_path: str) -> None:
        self._base = base_path.rstrip("/")
        self._bus: Any = None
        self._seen: dict[str, str] = {}  # device path -> central id
        #: The central behind the read or write being dispatched right now.
        #: The hook and the dispatch run back to back on the bus's loop, so
        #: a write handler reading this sees its own caller.
        self.current: str | None = None

    def attach(self, bus: Any) -> None:
        bus.add_message_handler(self._observe)
        self._bus = bus

    def detach(self) -> None:
        bus, self._bus = self._bus, None
        if bus is not None:
            try:
                bus.remove_message_handler(self._observe)
            except Exception:
                logger.debug("remove_message_handler failed", exc_info=True)
        self._seen.clear()
        self.current = None

    def _observe(self, msg: Any) -> None:
        try:
            if getattr(msg.message_type, "name", "") != "METHOD_CALL":
                return None
            if msg.interface != _BLUEZ_CHAR_IFACE:
                return None
            if msg.member not in ("ReadValue", "WriteValue"):
                return None
            path = msg.path or ""
            if not path.startswith(self._base + "/"):
                return None
            body = list(msg.body or ())
            options = body[0] if msg.member == "ReadValue" else body[1]
            device = options.get("device") if isinstance(options, dict) else None
            device_path = getattr(device, "value", device)
            central = _central_id_from_device_path(device_path)
            self.current = central
            if central is not None:
                self._seen[device_path] = central
        except Exception:
            self.current = None
            logger.debug("could not read the caller of a GATT call", exc_info=True)
        # Never handled here: bless dispatches the call as it always has.
        return None

    async def connected(self, subscribed: bool) -> set[str]:
        """The centrals still connected, or none while nobody subscribes."""
        bus = self._bus
        if bus is None:
            return set()
        gone: list[str] = []
        for path in list(self._seen):
            try:
                reply = await bus.call(_device_connected_query(path))
                alive = (
                    reply is not None
                    and getattr(reply.message_type, "name", "") == "METHOD_RETURN"
                    and bool(getattr(reply.body[0], "value", False))
                )
            except Exception:
                logger.debug("Device1.Connected query failed for %s", path, exc_info=True)
                alive = False
            if not alive:
                gone.append(path)
        for path in gone:
            self._seen.pop(path, None)
        return set(self._seen.values()) if subscribed else set()

    def any_seen(self) -> bool:
        """Whether a central of ours was connected at the last poll."""
        return bool(self._seen)


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
        A label for this peripheral, used only as the advertised local name
        (truncated to ten characters). It is **not** what a central learns as
        this device's id: the Device id characteristic carries the derived
        ``off1...`` address, resolved from the core when ``start()`` runs.
    max_connections:
        Maximum number of simultaneous central connections (default 4).
    """

    transport_id = "ble-peripheral"
    transport_name = "Bluetooth Low Energy (Peripheral)"

    def __init__(
        self,
        protocol: Any,
        device_id: str,
        *,
        max_connections: int = 4,
    ) -> None:
        super().__init__()
        self._protocol = protocol
        self._device_id = device_id
        self._max_connections = max_connections

        # What the two identity characteristics serve. Both come from the
        # core at start(): the address is derived from the identity key MLS
        # initialisation minted or loaded, and the assertion is signed by
        # that key. Before then there is nothing to serve, and _on_read
        # answers with nothing rather than with a label a central would
        # refuse anyway.
        self._address: str | None = None
        self._identity_bytes: bytes = b""

        # Server (created on start)
        self._server: BlessServer | None = None

        # Connected centrals: uuid -> connection timestamp
        self._connected_centrals: dict[str, float] = {}

        # Central UUID -> resolved user_id mapping.  Populated when the
        # protocol core emits a message_received event containing a
        # ``sender`` field.  Used to re-register the peer with its real
        # user_id so the Rust BLE transport can route outgoing messages.
        self._central_to_user_id: dict[str, str] = {}

        # Sticky "last known" central for single-peer sender attribution —
        # see _resolve_sender. Held to survive brief windows where the
        # peer monitor loop has not yet populated _connected_centrals.
        self._last_known_central: str | None = None

        # Event loop — captured in start() for thread-safe callback scheduling
        self._loop: asyncio.AbstractEventLoop | None = None

        # Lock protecting state shared between bless's delegate thread
        # (_on_write / _resolve_sender) and the asyncio event loop
        # (_peer_monitor_loop, resolve_sender_identity, stop). Guards:
        # metrics counters, _connected_centrals, _central_to_user_id,
        # _last_known_central. Follow "snapshot under lock, FFI outside"
        # so threading.Lock is never held across an await.
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

        # BlueZ only: who the centrals of our GATT server are (see
        # _BlueZCentrals). None on CoreBluetooth, whose bless delegate
        # tracks them itself.
        self._bluez_centrals: _BlueZCentrals | None = None

    # -- TransportManager interface -------------------------------------------

    def is_available(self) -> bool:
        """Return True on platforms where bless is supported (macOS, Linux),
        and only where it is installed."""
        return sys.platform in ("darwin", "linux") and BlessServer is not None

    def _resolve_identity(self) -> None:
        """Fetch the address and the assertion this peripheral will serve.

        The assertion is built by the core (``identity_assertion``): the
        whole ``public_key(32) || signature(64) || signed_data`` layout,
        so this binding assembles nothing and serves the same bytes for the
        same key as a peripheral on any other platform. ``signed_data`` is
        empty here. The chapter gives those bytes no meaning and a central
        never reads them; the mobile peripherals put their mesh
        advertisement there only because they have one.

        Raises :class:`TransportError` when the core has no address yet,
        which means MLS has not been initialised. A peripheral serving a
        label in place of an address is one every conforming central
        refuses, so refusing to start is the honest answer.
        """
        address = self._protocol.local_address()
        if not address:
            raise TransportError(
                "BLE peripheral has no address to serve: initialise MLS "
                "(ProtocolManager.start()) before starting the peripheral"
            )
        try:
            assertion = bytes(self._protocol.identity_assertion([]))
        except Exception as exc:
            raise TransportError(
                f"BLE peripheral could not build its identity assertion: {exc}"
            ) from exc
        self._address = address
        self._identity_bytes = assertion

    async def start(self) -> None:
        """Set up the GATT server, register characteristics, and advertise."""
        if self._state == TransportState.RUNNING:
            raise TransportError("BLE peripheral is already running")

        if not self.is_available():
            raise TransportError(
                f"BLE peripheral not available on {sys.platform}"
            )

        self._resolve_identity()

        self._loop = asyncio.get_running_loop()
        self._update_state(TransportState.STARTING)
        self._emit_diagnostic("info", "Starting BLE peripheral", {
            "device_id": self._device_id,
            "address": self._address,
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
            if sys.platform == "linux":
                self._attach_bluez_centrals()
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
        # STOPPING too: a stop() cancelled part-way (a shutdown deadline)
        # leaves the transport there, and a retry that returned at once would
        # leave it half torn down for good. Every step below is idempotent.
        if self._state not in (
            TransportState.RUNNING,
            TransportState.STARTING,
            TransportState.STOPPING,
        ):
            return

        self._update_state(TransportState.STOPPING)

        # Cancel background tasks
        if self._peer_monitor_task is not None and not self._peer_monitor_task.done():
            self._peer_monitor_task.cancel()
            # Waited on, not awaited: catching CancelledError here would also
            # swallow a cancel of this stop() itself, so a caller's shutdown
            # deadline would be ignored and the teardown would run past it.
            await asyncio.wait({self._peer_monitor_task})
        self._peer_monitor_task = None

        if self._bluez_centrals is not None:
            self._bluez_centrals.detach()
            self._bluez_centrals = None

        # Stop GATT server
        if self._server is not None:
            try:
                await self._server.stop()
            except Exception:
                pass
            self._server = None
        self._is_advertising = False

        # Snapshot under the lock so the delegate thread can't mutate
        # these maps concurrently, then emit FFI notifications outside
        # the lock (never hold threading.Lock across FFI).
        with self._lock:
            centrals_snapshot = list(self._connected_centrals.keys())
            resolved_snapshot = dict(self._central_to_user_id)
            self._connected_centrals.clear()
            self._central_to_user_id.clear()
            self._last_known_central = None

        for central_uuid in centrals_snapshot:
            try:
                self._protocol.ble_peer_lost(peer_id=central_uuid)
            except Exception:
                logger.debug("ble_peer_lost failed for %s", central_uuid)
            resolved_uid = resolved_snapshot.get(central_uuid)
            if resolved_uid:
                try:
                    self._protocol.ble_peer_lost(peer_id=resolved_uid)
                except Exception:
                    logger.debug("ble_peer_lost failed for resolved %s", resolved_uid)

        try:
            self._protocol.ble_status_changed(is_available=False)
        except Exception:
            logger.debug("ble_status_changed(False) failed", exc_info=True)

        self._update_state(TransportState.STOPPED)
        self._emit_diagnostic("info", "BLE peripheral stopped")

    def _attach_bluez_centrals(self) -> None:
        """Start observing who calls our GATT application on the bus.

        Reads bless's own bus and application (``server.bus``,
        ``server.app.base_path``, bless 0.3.x). Without them the peripheral
        still carries data, attributed as it was before this existed.
        """
        try:
            bus = self._server.bus  # type: ignore[union-attr]
            base_path = self._server.app.base_path  # type: ignore[union-attr]
        except AttributeError:
            logger.warning(
                "bless exposes no bus or application here: centrals of this "
                "peripheral cannot be told apart"
            )
            return
        tracker = _BlueZCentrals(base_path)
        tracker.attach(bus)
        self._bluez_centrals = tracker

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

        # DEVICE_ID characteristic — this device's address, as UTF-8
        await self._server.add_new_characteristic(
            SERVICE_UUID,
            DEVICE_ID_CHAR_UUID,
            GATTCharacteristicProperties.read,
            bytearray(self._device_id_bytes()),
            GATTAttributePermissions.readable,
        )

        # IDENTITY characteristic — the assertion that proves the address
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

    def _device_id_bytes(self) -> bytes:
        """The Device id characteristic's value: the address, or nothing."""
        return self._address.encode("utf-8") if self._address else b""

    def _on_read(self, char_uuid: Any) -> bytearray:
        """Handle incoming GATT read requests."""
        uuid = self._resolve_char_uuid(char_uuid)
        if uuid == DEVICE_ID_CHAR_UUID.lower():
            return bytearray(self._device_id_bytes())
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

        Called from the bless delegate thread, so state reads/writes must
        take `self._lock` — the asyncio `_peer_monitor_loop` mutates
        `_connected_centrals` / `_last_known_central` from a different
        thread. Returns a stable sender ID so the Rust core can group
        fragments from the same source.
        """
        tracker = self._bluez_centrals
        if tracker is not None and tracker.current is not None:
            # BlueZ named the writer: exact, however many are connected.
            with self._lock:
                self._last_known_central = tracker.current
            return tracker.current
        with self._lock:
            count = len(self._connected_centrals)
            if count == 1:
                central = next(iter(self._connected_centrals))
                self._last_known_central = central
                return central
            if count > 1:
                return "ble-peer"
            # No centrals currently tracked — use last known if available.
            return self._last_known_central or "ble-peer"

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
        with self._lock:
            count = len(self._connected_centrals)
            if count == 0:
                return
            if count != 1:
                # With multiple centrals we can't attribute unambiguously.
                logger.debug(
                    "resolve_sender_identity: %d centrals connected, skipping",
                    count,
                )
                return
            central_uuid = next(iter(self._connected_centrals))
            # Avoid redundant re-registration.
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
        # The core's fragment queue is shared with the central role and
        # cannot be peeked or refilled: a fragment popped here is gone. With
        # nobody subscribed a notification reaches no one, so leave the
        # queue alone.
        if not self.has_subscriber():
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
            logger.info(
                "OUTGOING fragment: %d bytes, recipient=%s",
                len(data), getattr(frag, "recipient_id", None),
            )
            await self.notify_fragment(data)

            # Return fragment to pool regardless of send outcome
            try:
                self._protocol.ble_return_fragment()
            except Exception:
                logger.debug("ble_return_fragment failed", exc_info=True)

    def has_subscriber(self) -> bool:
        """Whether a notification sent now reaches any central.

        BlueZ: bless records the characteristics a central has subscribed
        to, and a central of ours must still be connected. bless forgets a
        subscription only on ``StopNotify``, which BlueZ sends on disconnect
        for an unpaired central but not for a bonded one, so the list alone
        can name a subscriber that left. CoreBluetooth: the delegate's
        per-central subscriptions.
        """
        server = self._server
        if server is None:
            return False
        tracker = self._bluez_centrals
        if tracker is not None:
            try:
                subscribed = server.app.subscribed_characteristics
            except AttributeError:
                with self._lock:
                    return bool(self._connected_centrals)
            return tracker.any_seen() and MESSAGE_CHAR_UUID.lower() in {
                str(u).lower() for u in subscribed
            }
        try:
            return bool(server.peripheral_manager_delegate._central_subscriptions)
        except (AttributeError, TypeError):
            with self._lock:
                return bool(self._connected_centrals)

    async def notify_fragment(self, data: bytes) -> bool:
        """Notify one fragment on the Message characteristic.

        Every subscribed central receives it: neither backend of bless
        addresses a notification to one central, so a peripheral with more
        than one central fans every fragment out to all of them.

        A notify that raises is reported and still returns True, as
        ``BleManager.write_fragment`` does for a failed write: the fragment
        was this role's, the core's retry re-sends the message, and a drain
        that stopped here would strand every fragment behind this one.
        """
        server = self._server
        if server is None:
            return False
        try:
            char = server.get_characteristic(MESSAGE_CHAR_UUID)
            if char is None:
                return False
            char.value = data
            server.update_value(SERVICE_UUID, MESSAGE_CHAR_UUID)
        except Exception as exc:
            self._emit_diagnostic("warning", f"Failed to notify a fragment: {exc}")
            return True
        with self._lock:
            self._bytes_sent += len(data)
            self._fragments_sent += 1
        # Inter-frame delay to avoid overwhelming the BLE stack
        await asyncio.sleep(_INTER_FRAME_DELAY)
        return True

    # -- background tasks -----------------------------------------------------

    async def _current_centrals(self) -> set[str] | None:
        """The centrals connected now, or None when they cannot be read."""
        server = self._server
        if server is None:
            return None
        tracker = self._bluez_centrals
        if tracker is not None:
            return await tracker.connected(self.has_subscriber())
        try:
            delegate = server.peripheral_manager_delegate
            # NOTE: _central_subscriptions is a bless-internal dict of its
            # CoreBluetooth backend, validated against bless 0.3.x. If it is
            # missing after a bless upgrade the monitor degrades (no peer
            # tracking, but data transfer still works).
            return set(delegate._central_subscriptions.keys())
        except (AttributeError, TypeError):
            logger.debug(
                "Cannot read bless _central_subscriptions: "
                "peer monitoring disabled (bless API may have changed)"
            )
            return None

    async def _peer_monitor_loop(self) -> None:
        """Poll for centrals arriving and leaving.

        CoreBluetooth: a central subscribed to any characteristic is a peer,
        and its unsubscribing is a disconnection. BlueZ: see
        :class:`_BlueZCentrals`.
        """
        known: set[str] = set()
        try:
            while True:
                await asyncio.sleep(_PEER_MONITOR_INTERVAL)

                if self._server is None:
                    continue

                current = await self._current_centrals()
                if current is None:
                    continue

                # Classify under the lock (the delegate thread races us on
                # _connected_centrals / _central_to_user_id). FFI + diagnostic
                # notifications happen afterward, outside the lock.
                now = time.monotonic()
                added: list[str] = []
                rejected: list[str] = []
                removed: list[tuple[str, str | None]] = []

                with self._lock:
                    capacity = self._max_connections
                    for central_uuid in current - known:
                        if len(self._connected_centrals) >= capacity:
                            rejected.append(central_uuid)
                            continue
                        self._connected_centrals[central_uuid] = now
                        added.append(central_uuid)

                    for central_uuid in known - current:
                        self._connected_centrals.pop(central_uuid, None)
                        resolved_uid = self._central_to_user_id.pop(
                            central_uuid, None
                        )
                        removed.append((central_uuid, resolved_uid))

                for central_uuid in rejected:
                    self._emit_diagnostic(
                        "warning",
                        f"Max connections ({self._max_connections}) reached, "
                        f"ignoring {central_uuid}",
                    )

                for central_uuid in added:
                    try:
                        self._protocol.ble_peer_discovered(
                            peer_id=central_uuid, rssi=-50
                        )
                    except Exception:
                        logger.debug("ble_peer_discovered failed for %s", central_uuid)
                    self._emit_diagnostic(
                        "info", "Central connected", {"central": central_uuid}
                    )

                for central_uuid, resolved_uid in removed:
                    try:
                        self._protocol.ble_peer_lost(peer_id=central_uuid)
                    except Exception:
                        logger.debug("ble_peer_lost failed for %s", central_uuid)
                    if resolved_uid:
                        try:
                            self._protocol.ble_peer_lost(peer_id=resolved_uid)
                        except Exception:
                            logger.debug(
                                "ble_peer_lost failed for resolved %s", resolved_uid
                            )
                    self._emit_diagnostic(
                        "info", "Central disconnected", {"central": central_uuid}
                    )

                known = current.copy()

                # Update cached advertising state (CoreBluetooth only; on
                # BlueZ the flag set at start() stands).
                if self._bluez_centrals is None:
                    try:
                        self._is_advertising = self._server.peripheral_manager_delegate.is_advertising()
                    except Exception:
                        pass

        except asyncio.CancelledError:
            return
