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
from typing import Any, Callable

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
    HELLO_CHAR_UUID,
    HELLO_MAX_LEN,
    HELLO_MIN_LEN,
    IDENTITY_CHAR_UUID,
    MESSAGE_CHAR_UUID,
    SERVICE_UUID,
)
from .offline_protocol import verify_identity_assertion
from .transport_manager import TransportError, TransportManager, TransportState

logger = logging.getLogger(__name__)

# Inter-frame delay when sending multiple notification fragments (matches
# the 5 ms delay used by the M5StickC firmware in ble_mesh.h).
_INTER_FRAME_DELAY = 0.005

# How often to poll the bless delegate for subscription changes.
_PEER_MONITOR_INTERVAL = 1.0

# How long building the GATT server may take. bless's CoreBluetooth delegate
# blocks in its constructor, with no deadline, until the adapter reports
# powered on, which never happens when the process is not authorised to use
# Bluetooth. The local API server gives each transport 30 s to start; this is
# shorter so the refusal is this transport's own, with a reason.
_SERVER_INIT_TIMEOUT = 20.0

# ATT error codes a peripheral answers a refused write with (Core spec, Vol 3
# Part F, 3.4.1.1). CoreBluetooth's CBATTError values are these numbers.
_ATT_SUCCESS = 0
_ATT_INVALID_OFFSET = 0x07
_ATT_INVALID_ATTRIBUTE_VALUE_LENGTH = 0x0D

# A notification CoreBluetooth refused because its transmit queue was full is
# retried once the stack says it is ready again, this many times, waiting at
# most this long each. bless drops the refusal on the floor.
_NOTIFY_RETRIES = 4
_NOTIFY_READY_TIMEOUT = 1.0

# A central that said hello but is not subscribed is given this long to
# subscribe before its binding is dropped and its peer reported lost. Without
# it a central that writes a hello and disconnects without ever subscribing
# (the monitor tracks subscribers only) left its binding, and a route to the
# address it proved, behind for the life of the process.
_UNSUBSCRIBED_HELLO_GRACE = 5.0

_ATTRIBUTING_DELEGATE: Any = None


def _prepared_hello_refusal(writes: list[tuple[str, int]]) -> int | None:
    """The ATT code that refuses a batch carrying a hello, or None.

    ``writes`` is each request's ``(characteristic UUID, offset)``.
    CoreBluetooth hands a prepared (long) write to the delegate as one batch
    of requests at rising offsets, and the chapter requires a peripheral to
    refuse a prepared or offset hello before verifying anything. Handled
    request by request, the batch's first chunk (offset 0, a plausible
    length) reached the verifier before the second chunk's offset refused it.
    """
    hello = HELLO_CHAR_UUID.lower()
    for char_uuid, offset in writes:
        if char_uuid.lower() == hello and (len(writes) > 1 or offset != 0):
            return _ATT_INVALID_OFFSET
    return None
_DELEGATE_SWAP_LOCK = threading.Lock()
# How long a construction waits for another one's delegate swap. A swap is
# held for a whole construction, and one that wedged (see
# _SERVER_INIT_TIMEOUT) never releases it, so a later start says so instead
# of waiting out the init deadline and blaming the adapter.
_DELEGATE_SWAP_WAIT = 2.0


def _attributing_delegate_class() -> Any:
    """bless's CoreBluetooth delegate, made to say WHICH central did what.

    bless hands a write to its callback as ``(characteristic, value)``,
    dropping the ``CBCentral`` the request came from, and responds success
    to every write. With two centrals connected (a phone, and a Mac's own
    central, or the two addresses of one rotating peer) a write could not be
    attributed, and the core refused the writer's key package because its
    sender did not match the link. This subclass passes each request's
    central, offset and value to ``attributed_write_func``, answers with the
    ATT code that returns, keeps the ``CBCentral`` objects of subscribers so
    a notification can be addressed to one of them, and reports the stack's
    "ready to update subscribers" to ``ready_func``.

    Built once per process: an Objective-C class name can be registered only
    once. None where bless's CoreBluetooth backend is not importable.
    """
    global _ATTRIBUTING_DELEGATE
    if _ATTRIBUTING_DELEGATE is not None:
        return _ATTRIBUTING_DELEGATE
    try:
        import objc  # type: ignore[import-not-found]
        from bless.backends.corebluetooth.peripheral_manager_delegate import (  # type: ignore[import-not-found]
            PeripheralManagerDelegate,
        )
    except Exception:  # pragma: no cover - only reachable off macOS
        return None

    class OfflineProtocolPeripheralDelegate(PeripheralManagerDelegate):  # type: ignore[misc, valid-type]
        def peripheralManager_didReceiveWriteRequests_(  # noqa: N802
            self, peripheral_manager: Any, requests: Any
        ) -> None:
            handler = getattr(self, "attributed_write_func", None)
            result = _ATT_SUCCESS
            if handler is not None:
                try:
                    refusal = _prepared_hello_refusal([
                        (str(r.characteristic().UUID().UUIDString()), int(r.offset()))
                        for r in requests
                    ])
                except Exception:  # never let a raise skip the response
                    refusal = None
                if refusal is not None:
                    peripheral_manager.respondToRequest_withResult_(requests[0], refusal)
                    return
            for request in requests:
                char_uuid = str(request.characteristic().UUID().UUIDString())
                raw = request.value()
                value = bytes(raw) if raw is not None else b""
                if handler is None:
                    self.write_request_func(char_uuid, value)
                    continue
                central = str(request.central().identifier().UUIDString())
                try:
                    result = int(handler(central, char_uuid, value, int(request.offset())))
                except Exception:  # never let a raise skip the response
                    result = _ATT_SUCCESS
                if result != _ATT_SUCCESS:
                    break
            peripheral_manager.respondToRequest_withResult_(requests[0], result)

        def peripheralManager_central_didSubscribeToCharacteristic_(  # noqa: N802
            self, peripheral_manager: Any, central: Any, characteristic: Any
        ) -> None:
            objc.super(
                OfflineProtocolPeripheralDelegate, self
            ).peripheralManager_central_didSubscribeToCharacteristic_(
                peripheral_manager, central, characteristic
            )
            centrals = getattr(self, "subscribed_centrals", None)
            if centrals is not None:
                centrals[str(central.identifier().UUIDString())] = central

        def peripheralManager_central_didUnsubscribeFromCharacteristic_(  # noqa: N802
            self, peripheral_manager: Any, central: Any, characteristic: Any
        ) -> None:
            central_id = str(central.identifier().UUIDString())
            try:
                objc.super(
                    OfflineProtocolPeripheralDelegate, self
                ).peripheralManager_central_didUnsubscribeFromCharacteristic_(
                    peripheral_manager, central, characteristic
                )
            except (KeyError, ValueError):
                pass
            centrals = getattr(self, "subscribed_centrals", None)
            if centrals is not None and central_id not in self._central_subscriptions:
                centrals.pop(central_id, None)

        def peripheralManagerIsReadyToUpdateSubscribers_(  # noqa: N802
            self, peripheral_manager: Any
        ) -> None:
            ready = getattr(self, "ready_func", None)
            if ready is not None:
                try:
                    ready()
                except Exception:
                    pass

    _ATTRIBUTING_DELEGATE = OfflineProtocolPeripheralDelegate
    return _ATTRIBUTING_DELEGATE


def _bluetooth_authorization_refusal() -> str | None:
    """Why this process may not use Bluetooth on macOS, or None if it may.

    A process macOS has denied (or one that cannot ask, such as a service
    started over ssh) would otherwise block in bless's constructor forever.
    ``NotDetermined`` is allowed through: building the manager is what makes
    macOS ask, and the construction deadline bounds the wait for an answer.
    """
    if sys.platform != "darwin":
        return None
    try:
        from CoreBluetooth import (  # type: ignore[import-not-found]
            CBManager,
            CBManagerAuthorizationDenied,
            CBManagerAuthorizationRestricted,
        )
    except Exception:
        return None
    try:
        state = CBManager.authorization()
    except Exception:
        return None
    if state == CBManagerAuthorizationDenied:
        return (
            "Bluetooth is denied to this process: allow it in System Settings > "
            "Privacy & Security > Bluetooth, or start the service from a GUI "
            "session (macOS cannot ask a process started over ssh)"
        )
    if state == CBManagerAuthorizationRestricted:
        return "Bluetooth is restricted on this Mac by a profile or parental control"
    return None
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
        #: a write handler reading this sees its own caller. That holds only
        #: because bless's ReadValue and WriteValue are plain methods, which
        #: dbus_next runs inline after the hook; a coroutine method would be
        #: scheduled, and the next call's hook would run first.
        #: ``test_bless_dispatches_gatt_calls_synchronously`` pins it.
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
        # Central id -> the address its hello proved. A link bound here feeds
        # its fragments to the core under that address, which is what lets a
        # key package from it through the core's transport-sender check.
        # Never rebound: a hello on a bound link is ignored.
        self._hello_bindings: dict[str, str] = {}
        # Central id -> when its bound hello was first seen without a
        # subscription (see _UNSUBSCRIBED_HELLO_GRACE). Monitor loop only.
        self._unsubscribed_since: dict[str, float] = {}
        # Set by ProtocolManager: whether another link (our own central's)
        # already announced an address, so a hello does not announce it twice
        # and a central leaving does not report a peer lost that is still up.
        self.peer_linked_elsewhere: Any = None
        # CoreBluetooth only: the attributing delegate's subscriber map and
        # the stack's "ready to update subscribers" signal.
        self._subscribed_centrals: dict[str, Any] = {}
        self._notify_ready: asyncio.Event | None = None
        # The monitor loop's clock. A test substitutes one it advances itself:
        # on Windows time.monotonic() ticks every 15.6 ms, and asyncio runs a
        # timer due within that at once, so a test sleeping in milliseconds
        # never saw a grace period elapse.
        self._clock: Callable[[], float] = time.monotonic
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
            refusal = _bluetooth_authorization_refusal()
            if refusal is not None:
                raise TransportError(refusal)
            # Truncate name — CoreBluetooth limits advertisement data to ~28
            # bytes, and service UUIDs also consume space.
            name = self._device_id[:10]
            self._server = await self._build_server(name)
            self._install_attribution()

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
            self._hello_bindings.clear()
            self._unsubscribed_since.clear()
            self._last_known_central = None
        self._subscribed_centrals = {}
        self._notify_ready = None
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

    async def _build_server(self, name: str) -> Any:
        """Construct the bless server off the event loop, with a deadline.

        The CoreBluetooth constructor can block indefinitely (see
        ``_SERVER_INIT_TIMEOUT``), and a block on the loop thread freezes the
        whole service: no API socket, no other transport, and no deadline of
        the caller's can fire. The thread is a daemon, so one that never
        returns cannot hold the process open at exit. It is handed the running
        loop, since bless otherwise looks one up in a thread that has none.

        BlueZ's constructor does not block, and it schedules its setup as a
        task on the loop, which is safe only from the loop's own thread, so
        there it runs here.
        """
        loop = asyncio.get_running_loop()
        if sys.platform != "darwin":
            return self._construct_server(name, loop)
        done: asyncio.Future[Any] = loop.create_future()

        def settle(result: Any, exc: BaseException | None) -> None:
            if done.done():
                return
            if exc is not None:
                done.set_exception(exc)
            else:
                done.set_result(result)

        def build() -> None:
            try:
                server = self._construct_server(name, loop)
            except BaseException as exc:  # noqa: BLE001 - handed to the loop
                result, error = None, exc
            else:
                result, error = server, None
            try:
                loop.call_soon_threadsafe(settle, result, error)
            except RuntimeError:
                pass  # the loop closed while the stack was deciding

        threading.Thread(target=build, name="ble-peripheral-init", daemon=True).start()
        try:
            return await asyncio.wait_for(done, _SERVER_INIT_TIMEOUT)
        except asyncio.TimeoutError as exc:
            raise TransportError(
                f"the Bluetooth stack did not come up within {_SERVER_INIT_TIMEOUT:.0f} s "
                "(not authorised, or the adapter is off)"
            ) from exc

    @staticmethod
    def _construct_server(name: str, loop: Any = None) -> Any:
        """``BlessServer(name)``, with the attributing delegate on macOS.

        bless's CoreBluetooth server builds its delegate from a name in its
        own module; it is pointed at the subclass for this one construction,
        under a lock, and put back.
        """
        if sys.platform != "darwin":
            return BlessServer(name=name, loop=loop)
        # The class is built under the lock too: an Objective-C class name can
        # be registered once per process, and two constructions racing to
        # build it would both try.
        if not _DELEGATE_SWAP_LOCK.acquire(timeout=_DELEGATE_SWAP_WAIT):
            raise TransportError(
                "an earlier Bluetooth stack construction in this process never "
                "returned; restart the service once Bluetooth is allowed"
            )
        try:
            delegate_cls = _attributing_delegate_class()
            if delegate_cls is None:
                return BlessServer(name=name, loop=loop)
            import bless.backends.corebluetooth.server as cb_server  # type: ignore[import-not-found]

            original = cb_server.PeripheralManagerDelegate
            cb_server.PeripheralManagerDelegate = delegate_cls
            try:
                return BlessServer(name=name, loop=loop)
            finally:
                cb_server.PeripheralManagerDelegate = original
        finally:
            _DELEGATE_SWAP_LOCK.release()

    def _install_attribution(self) -> None:
        """Point the attributing delegate, when present, at this peripheral."""
        delegate = getattr(self._server, "peripheral_manager_delegate", None)
        if delegate is None or not hasattr(delegate, "peripheralManagerIsReadyToUpdateSubscribers_"):
            return
        if _ATTRIBUTING_DELEGATE is None or not isinstance(delegate, _ATTRIBUTING_DELEGATE):
            return
        loop = self._loop
        self._notify_ready = asyncio.Event()
        ready = self._notify_ready
        self._subscribed_centrals = {}
        delegate.subscribed_centrals = self._subscribed_centrals
        delegate.attributed_write_func = self._on_attributed_write

        def on_ready() -> None:
            if loop is not None and not loop.is_closed():
                loop.call_soon_threadsafe(ready.set)

        delegate.ready_func = on_ready

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

        # HELLO characteristic: a central writes its assertion here, once,
        # so its Message writes can be attributed to the address it proved.
        await self._server.add_new_characteristic(
            SERVICE_UUID,
            HELLO_CHAR_UUID,
            GATTCharacteristicProperties.write,
            None,
            GATTAttributePermissions.writeable,
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
            self._handle_write(
                self._resolve_sender(),
                self._resolve_char_uuid(char_uuid),
                bytes(value) if value is not None else b"",
                0,
            )
        except Exception as exc:
            logger.exception("_on_write failed: %s", exc)

    def _on_attributed_write(
        self, central: str, char_uuid: str, value: bytes, offset: int
    ) -> int:
        """A write the attributing delegate names the central of (macOS).

        Returns the ATT code to answer the request with. Runs on
        CoreBluetooth's queue, so it never raises.
        """
        try:
            with self._lock:
                self._last_known_central = central
            return self._handle_write(central, char_uuid.lower(), value, offset)
        except Exception as exc:
            logger.exception("attributed write failed: %s", exc)
            return _ATT_SUCCESS

    def _handle_write(
        self, central: str, char_uuid: str, value: bytes, offset: int
    ) -> int:
        """One write from ``central``: a hello, or a Message fragment."""
        if char_uuid == HELLO_CHAR_UUID.lower():
            return self._on_hello(central, value, offset)
        if char_uuid != MESSAGE_CHAR_UUID.lower():
            logger.debug("ignoring a write to %s", char_uuid)
            return _ATT_SUCCESS
        with self._lock:
            sender_id = self._hello_bindings.get(central, central)
            self._bytes_received += len(value)
            self._fragments_received += 1
        logger.debug("fragment of %d bytes from %s as %s", len(value), central, sender_id)
        self._protocol.ble_fragment_received(sender_id=sender_id, fragment=list(value))
        return _ATT_SUCCESS

    def _on_hello(self, central: str, value: bytes, offset: int) -> int:
        """Bind ``central``'s link to the address its hello proves.

        The chapter's order: refuse a prepared or offset write and a value out
        of bounds before verifying anything; verify with the one verifier; bind
        to the derived address only; never rebind; announce the peer only if no
        other link has. A value that fails is ignored, and the link stays as it
        would have been without one.
        """
        if offset != 0:
            return _ATT_INVALID_OFFSET
        if not HELLO_MIN_LEN <= len(value) <= HELLO_MAX_LEN:
            return _ATT_INVALID_ATTRIBUTE_VALUE_LENGTH
        if central == "ble-peer":
            # The writer could not be named, so there is no link to bind.
            return _ATT_SUCCESS
        try:
            address = verify_identity_assertion(list(value))
        except Exception as exc:
            self._emit_diagnostic("warning", "A hello did not verify", {
                "central": central,
                "error": str(exc),
            })
            return _ATT_SUCCESS
        with self._lock:
            bound = self._hello_bindings.get(central)
            if bound is not None:
                if bound != address:
                    logger.info("ignoring a hello naming %s on a link bound to %s", address, bound)
                return _ATT_SUCCESS
            self._hello_bindings[central] = address
            self._central_to_user_id[central] = address
            already = any(
                other == address
                for c, other in self._hello_bindings.items()
                if c != central
            )
        self._emit_diagnostic("info", "Central said hello", {
            "central": central,
            "address": address,
        })
        if not already and not self._linked_elsewhere(address):
            try:
                self._protocol.ble_peer_discovered(peer_id=address, rssi=-50)
            except Exception:
                logger.debug("ble_peer_discovered failed for %s", address)
        return _ATT_SUCCESS

    def holds_peer(self, address: str) -> bool:
        """Whether a subscribed central of ours carries ``address``.

        ``BleManager`` asks this before reporting a peer lost when its client
        link goes: a notification addressed to ``address`` still reaches that
        central, so the core keeps a route it would otherwise drop.
        """
        with self._lock:
            return any(
                self._central_to_user_id.get(central) == address
                for central in self._connected_centrals
            )

    def _linked_elsewhere(self, address: str) -> bool:
        probe = self.peer_linked_elsewhere
        if probe is None:
            return False
        try:
            return bool(probe(address))
        except Exception:
            return False
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
            # A hello proved this link's address; a message's sender field
            # proves nothing, so it never replaces one.
            if central_uuid in self._hello_bindings:
                return
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
            recipient = getattr(frag, "recipient_id", None)
            logger.debug("outgoing fragment: %d bytes, recipient=%s", len(data), recipient)
            await self.notify_fragment(data, recipient)
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

    async def notify_fragment(self, data: bytes, recipient: str | None = None) -> bool:
        """Notify one fragment on the Message characteristic.

        On macOS, a fragment for a recipient whose central said hello goes to
        that central alone; anything else goes to every subscribed central.
        BlueZ offers no per-central notification, so there every subscriber
        receives every fragment.
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
            if self._notify_ready is not None:
                await self._notify_corebluetooth(server, char, data, recipient)
            else:
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

    def _centrals_for(self, recipient: str | None) -> list[Any] | None:
        """The subscribed ``CBCentral`` objects bound to ``recipient``, or
        None to notify every subscriber."""
        if not recipient:
            return None
        with self._lock:
            ids = [c for c, a in self._hello_bindings.items() if a == recipient]
        # .get, not a membership test then an index: CoreBluetooth's queue
        # removes an unsubscribing central between the two.
        subscribed = self._subscribed_centrals
        targets = [t for t in (subscribed.get(c) for c in ids) if t is not None]
        return targets or None

    async def _notify_corebluetooth(
        self, server: Any, char: Any, data: bytes, recipient: str | None
    ) -> None:
        """``updateValue`` with CoreBluetooth's back-pressure honoured.

        ``updateValue`` returns NO when the transmit queue is full, and the
        fragment is not sent. bless ignores that, so a burst (a message, its
        receipt, a key package) lost fragments silently and the message
        waited for the core's next retry. Here a refusal waits for the
        stack's "ready to update subscribers" and tries again.
        """
        manager = server.peripheral_manager_delegate.peripheral_manager
        ready = self._notify_ready
        targets = self._centrals_for(recipient)
        for _ in range(_NOTIFY_RETRIES + 1):
            if ready is not None:
                ready.clear()
            # PyObjC passes bytes where an NSData is expected.
            if manager.updateValue_forCharacteristic_onSubscribedCentrals_(data, char.obj, targets):
                return
            if ready is None:
                break
            try:
                await asyncio.wait_for(ready.wait(), _NOTIFY_READY_TIMEOUT)
            except asyncio.TimeoutError:
                pass
        self._emit_diagnostic("warning", "CoreBluetooth kept refusing a notification", {
            "recipient": recipient,
            "length": len(data),
        })

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
                now = self._clock()
                added: list[str] = []
                rejected: list[str] = []
                removed: list[tuple[str, str | None]] = []
                dropped: list[tuple[str, str]] = []

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
                        self._hello_bindings.pop(central_uuid, None)
                        if resolved_uid is not None and resolved_uid in self._hello_bindings.values():
                            resolved_uid = None  # another central of ours still carries it
                        removed.append((central_uuid, resolved_uid))

                    # A central that said hello and never subscribed is not in
                    # `known`, so the sweep above never sees it leave.
                    orphans: list[str] = []
                    for central_uuid in list(self._hello_bindings):
                        if central_uuid in current or central_uuid in self._connected_centrals:
                            self._unsubscribed_since.pop(central_uuid, None)
                            continue
                        since = self._unsubscribed_since.setdefault(central_uuid, now)
                        if now - since >= _UNSUBSCRIBED_HELLO_GRACE:
                            orphans.append(central_uuid)
                    for central_uuid in orphans:
                        self._unsubscribed_since.pop(central_uuid, None)
                        address = self._hello_bindings.pop(central_uuid)
                        self._central_to_user_id.pop(central_uuid, None)
                        if address in self._hello_bindings.values():
                            continue  # another central of ours still carries it
                        dropped.append((central_uuid, address))
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
                    if resolved_uid and not self._linked_elsewhere(resolved_uid):
                        try:
                            self._protocol.ble_peer_lost(peer_id=resolved_uid)
                        except Exception:
                            logger.debug(
                                "ble_peer_lost failed for resolved %s", resolved_uid
                            )
                    self._emit_diagnostic(
                        "info", "Central disconnected", {"central": central_uuid}
                    )

                for central_uuid, address in dropped:
                    self._emit_diagnostic("info", "Dropped a hello from a central that never subscribed", {
                        "central": central_uuid,
                        "address": address,
                    })
                    if self._linked_elsewhere(address):
                        continue
                    try:
                        self._protocol.ble_peer_lost(peer_id=address)
                    except Exception:
                        logger.debug("ble_peer_lost failed for %s", address)

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
