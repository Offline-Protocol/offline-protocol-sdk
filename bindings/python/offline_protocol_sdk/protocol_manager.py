"""High-level protocol manager for desktop platforms.

Mirrors ``OfflineProtocolModule.swift`` / ``OfflineProtocolModule.kt`` —
wraps the UniFFI ``OfflineProtocol`` instance with transport managers, a
100 ms processing loop, event dispatch, and lifecycle management.
"""

from __future__ import annotations

import asyncio
import json
import logging
from typing import Any, Callable

from .offline_protocol import (
    BleTransportCallback,
    EventCallback,
    MessagePriority,
    NostrTransportCallback,
    OfflineProtocol,
    ProtocolConfig,
    ReticulumTransportCallback,
    TelemetryConfig,
    TelemetrySink,
    WifiDirectTransportCallback,
)
from .ble_manager import BleManager
from .ble_peripheral import BlePeripheral
from .internet_manager import InternetManager
from .secure_storage import SecureStorage
from .state_storage import AppStateStorage

logger = logging.getLogger(__name__)

# Processing loop interval (matches iOS/Android: 100 ms)
_PROCESS_INTERVAL = 0.1
# Maximum messages to drain per process tick (matches iOS maxMessagesPerProcessTick)
_MAX_MESSAGES_PER_TICK = 100


class _EventCallbackImpl(EventCallback):
    """Routes protocol events to Python callbacks."""

    def __init__(self, handler: Callable[[dict[str, Any]], None] | None) -> None:
        self._handler = handler

    def on_event(self, event_json: str) -> None:
        if self._handler is None:
            return
        try:
            event = json.loads(event_json)
        except json.JSONDecodeError:
            event = {"raw": event_json}
        self._handler(event)


class _BleTransportCallbackImpl(BleTransportCallback):
    """Bridges Rust BLE fragment-ready notifications to BLE managers."""

    def __init__(
        self,
        ble_manager: BleManager | None,
        ble_peripheral: BlePeripheral | None,
    ) -> None:
        self._ble = ble_manager
        self._peripheral = ble_peripheral

    def on_fragments_available(self) -> None:
        # The scanner and peripheral share a single fragment queue.
        # Only let the scanner drain if it has connected clients;
        # otherwise the scanner pops fragments it can't deliver,
        # starving the peripheral.
        scanner_has_clients = (
            self._ble is not None
            and bool(self._ble._clients)
        )
        if scanner_has_clients:
            self._ble.on_fragments_available()
        if self._peripheral is not None:
            self._peripheral.on_fragments_available()


class _WifiDirectTransportCallbackImpl(WifiDirectTransportCallback):
    """Stub — WiFi Direct is not implemented on desktop."""

    def on_messages_available(self) -> None:
        pass


class _NostrTransportCallbackImpl(NostrTransportCallback):
    """Stub — no desktop Nostr manager; apps driving Nostr manually must
    call ``protocol.set_nostr_transport_callback()`` with their own impl."""

    def on_messages_available(self) -> None:
        pass


class _ReticulumTransportCallbackImpl(ReticulumTransportCallback):
    """Stub — no desktop Reticulum manager; apps driving Reticulum manually
    must call ``protocol.set_reticulum_transport_callback()`` with their own
    impl."""

    def on_messages_available(self) -> None:
        pass


class ProtocolManager:
    """Manages the full protocol lifecycle for desktop platforms.

    Typical usage::

        async with ProtocolManager(config) as pm:
            pm.on_event(lambda e: print(e))
            pm.internet.configure(server_url="ws://relay.example.com")
            await pm.internet.start()
            pm.send_message("peer-id", "hello")

    Or without ``async with``::

        pm = ProtocolManager(config)
        await pm.start()
        ...
        await pm.stop()

    Parameters
    ----------
    config:
        ``ProtocolConfig`` dict/object (UniFFI-generated).
    event_handler:
        Optional callback invoked for every protocol event (parsed JSON dict).
    storage:
        Optional ``MlsStorageProvider`` for MLS key material.  Defaults to
        :class:`SecureStorage` backed by the platform keyring.
    state_storage:
        Optional ``ProtocolStateStorageProvider`` for restartable delivery
        state. Defaults to :class:`AppStateStorage` outside the credential
        store.
    """

    def __init__(
        self,
        config: ProtocolConfig,
        event_handler: Callable[[dict[str, Any]], None] | None = None,
        storage: Any | None = None,
        state_storage: Any | None = None,
    ) -> None:
        self._config = config
        self._event_handler = event_handler
        self._storage = storage or SecureStorage()
        self._state_storage = state_storage or AppStateStorage()

        # Create the core protocol instance
        self._protocol = OfflineProtocol(config)

        # Transport managers
        device_id = config.user_id  # type: ignore[union-attr]
        app_id = getattr(config, "app_id", "offline-messenger")

        self.ble: BleManager | None = None
        if getattr(config, "ble_enabled", False):
            self.ble = BleManager(self._protocol, device_id)

        self.internet: InternetManager | None = None
        if getattr(config, "internet_enabled", False):
            self.internet = InternetManager(self._protocol, device_id, app_id=app_id)

        self.ble_peripheral: BlePeripheral | None = None
        if getattr(config, "ble_enabled", False):
            self.ble_peripheral = BlePeripheral(self._protocol, device_id)

        # Processing loop task
        self._process_task: asyncio.Task[None] | None = None
        self._running = False

        # Registry of objects whose pointers are held by the Rust/UniFFI side.
        # Prevents garbage collection while the protocol is alive.
        self._prevent_gc: list[Any] = []

    @property
    def protocol(self) -> OfflineProtocol:
        """Direct access to the underlying UniFFI ``OfflineProtocol``."""
        return self._protocol

    # -- lifecycle ------------------------------------------------------------

    async def start(self) -> None:
        """Wire callbacks, initialise MLS, and start the processing loop."""
        if self._running:
            return

        # Set event callback
        self._event_cb = _EventCallbackImpl(self._event_handler)
        self._protocol.set_event_callback(self._event_cb)

        # Set transport callbacks
        self._ble_cb = _BleTransportCallbackImpl(self.ble, self.ble_peripheral)
        self._protocol.set_ble_transport_callback(self._ble_cb)

        self._wifi_cb = _WifiDirectTransportCallbackImpl()
        self._protocol.set_wifi_direct_transport_callback(self._wifi_cb)

        # Mirror the RN iOS/Android modules: only wire the nostr/reticulum
        # callbacks when the transport is enabled in config. Installing a
        # stub unconditionally would silently swallow `on_messages_available`
        # on desktop apps that enable the transport but drive it themselves.
        self._nostr_cb: _NostrTransportCallbackImpl | None = None
        if getattr(self._config, "nostr_enabled", False):
            self._nostr_cb = _NostrTransportCallbackImpl()
            self._protocol.set_nostr_transport_callback(self._nostr_cb)

        self._reticulum_cb: _ReticulumTransportCallbackImpl | None = None
        if getattr(self._config, "reticulum_enabled", False):
            self._reticulum_cb = _ReticulumTransportCallbackImpl()
            self._protocol.set_reticulum_transport_callback(self._reticulum_cb)

        # Keep strong references to all objects passed to the Rust/UniFFI
        # side so that Python's GC cannot collect them while Rust holds
        # raw callback pointers.
        self._prevent_gc = [
            self._event_cb,
            self._ble_cb,
            self._wifi_cb,
            self._storage,
            self._state_storage,
        ]
        if self._nostr_cb is not None:
            self._prevent_gc.append(self._nostr_cb)
        if self._reticulum_cb is not None:
            self._prevent_gc.append(self._reticulum_cb)

        # Initialise MLS encryption
        try:
            self._protocol.initialize_mls(self._storage, self._state_storage)
        except Exception as exc:
            logger.warning("MLS initialisation failed (non-fatal): %s", exc)

        # Start the protocol engine
        self._protocol.start()
        self._running = True

        # Start the 100 ms processing loop
        self._process_task = asyncio.ensure_future(self._process_loop())

        logger.info("ProtocolManager started (user_id=%s)", self._config.user_id)  # type: ignore[union-attr]

    async def stop(self) -> None:
        """Stop all transports and the processing loop."""
        if not self._running:
            return

        self._running = False

        # Cancel processing loop
        if self._process_task is not None and not self._process_task.done():
            self._process_task.cancel()
            try:
                await self._process_task
            except asyncio.CancelledError:
                pass
        self._process_task = None

        # Stop transports
        if self.ble is not None:
            await self.ble.stop()
        if self.ble_peripheral is not None:
            await self.ble_peripheral.stop()
        if self.internet is not None:
            await self.internet.stop()

        # Detach any installed telemetry sink before we drop GC pins.
        # Rust retains the sink handle until this is called, so skipping
        # it would leak the sink for the lifetime of the manager.
        # Idempotent on the Rust side — safe to call without a prior install.
        teardown_clean = True
        try:
            self.uninstall_telemetry_sink()
        except Exception:
            teardown_clean = False
            logger.debug(
                "uninstall_telemetry_sink raised during stop (non-fatal)",
                exc_info=True,
            )

        # Stop protocol engine
        try:
            self._protocol.stop()
        except Exception:
            teardown_clean = False
            logger.debug("protocol.stop() raised (non-fatal)", exc_info=True)

        # Only release GC pins once Rust has confirmed it no longer holds
        # callback handles. If either teardown step raised, Rust may still
        # hold pointers to the pinned objects — dropping the last strong
        # reference here would create a use-after-free risk on any future
        # callback fire.
        if teardown_clean:
            self._prevent_gc.clear()
        elif self._prevent_gc:
            logger.warning(
                "ProtocolManager stop() encountered errors; retaining %d GC "
                "pins so Rust callbacks cannot dangle.",
                len(self._prevent_gc),
            )

        logger.info("ProtocolManager stopped")

    def __del__(self) -> None:
        if self._running:
            logger.error(
                "ProtocolManager garbage-collected while still running! "
                "Call await stop() before discarding the manager to avoid "
                "dangling callback pointers on the Rust side."
            )

    async def __aenter__(self) -> ProtocolManager:
        await self.start()
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.stop()

    # -- event handling -------------------------------------------------------

    def on_event(self, handler: Callable[[dict[str, Any]], None]) -> None:
        """Register an event handler (replaces the current one)."""
        self._event_handler = handler
        # Update the live callback
        if hasattr(self, "_event_cb"):
            self._event_cb._handler = handler

    # -- convenience wrappers -------------------------------------------------

    def send_message(
        self,
        recipient: str,
        content: str,
        *,
        priority: MessagePriority = MessagePriority.MEDIUM,
        reply_to: str | None = None,
    ) -> str:
        """Send a text message and return its message ID.

        ``recipient == "*"`` is a **BLE-only convenience** in this
        wrapper: the message is fanned out one-per-peer to every known
        BLE peer (``get_known_ble_peers``). The Rust core's
        ``BleTransport::send()`` treats ``"*"`` as a literal peer-ID
        lookup which always fails, so the fan-out is required for BLE.

        This wrapper does **not** attempt cross-transport broadcast:
        other transports (Internet, Nostr, Reticulum, Wi-Fi Direct)
        accept ``"*"`` into their send queue but their broadcast
        semantics are platform-layer concerns and are not routed
        automatically by ``send_message``. If you need Nostr or
        Internet broadcast, drive those transports directly.

        Raises
        ------
        ValueError
            If ``recipient == "*"`` and no BLE peers are currently known.
            To tolerate an empty BLE peer set, query
            ``get_known_ble_peers`` first or catch this exception.
        """
        if recipient == "*":
            peers = self.get_known_ble_peers()
            if not peers:
                raise ValueError(
                    "ProtocolManager's '*' broadcast fans out over BLE "
                    "peers only; no BLE peers are currently known. "
                    "Use get_known_ble_peers() to check before sending, "
                    "or address a specific peer directly."
                )
            last_id = ""
            for peer_id in peers:
                last_id = self._protocol.send_message(
                    recipient=peer_id,
                    content=content,
                    priority=priority,
                    reply_to_msg=reply_to,
                )
            return last_id

        return self._protocol.send_message(
            recipient=recipient,
            content=content,
            priority=priority,
            reply_to_msg=reply_to,
        )

    def get_known_ble_peers(self) -> list[str]:
        """Return user_ids of all BLE-discovered peers (deduplicated).

        Public because ``send_message("*")`` documents it as the
        escape hatch for callers that want to check before a broadcast.
        Union of central-mode (``self.ble``) and peripheral-mode
        (``self.ble_peripheral``) peer maps.
        """
        peers: list[str] = []
        if self.ble is not None:
            try:
                with self.ble._lock:
                    peers.extend(self.ble._peer_device_ids.values())
            except Exception:
                logger.debug("ble peer lookup failed", exc_info=True)
        if self.ble_peripheral is not None:
            try:
                with self.ble_peripheral._lock:
                    peers.extend(self.ble_peripheral._central_to_user_id.values())
            except Exception:
                logger.debug("ble_peripheral peer lookup failed", exc_info=True)
        return list(set(peers))

    def get_state(self) -> Any:
        """Return the current protocol state."""
        return self._protocol.get_state()

    # -- telemetry ------------------------------------------------------------

    def install_telemetry_sink(
        self,
        sink: TelemetrySink,
        config: TelemetryConfig | None = None,
    ) -> None:
        """Install a ``TelemetrySink`` on the underlying protocol.

        The sink reference is retained in ``_prevent_gc`` so Python's GC
        cannot collect it while Rust holds a raw callback pointer.
        Re-installing replaces the previous sink on the Rust side; the
        prior sink's GC pin is released here so repeated installs do not
        accumulate references.

        All ``TelemetryConfig`` fields are optional — passing ``None`` (or
        a field left as ``None``) uses the Rust-side defaults:
        ``scrub_ids=True``, ``mls_verbosity=Lifecycle``,
        ``metrics_cadence_ms=5000``, ``routing_diagnostic=False``,
        ``enable_poll_queue=True``, ``mls_sampling_bypass=False``.
        """
        effective = config if config is not None else TelemetryConfig(
            scrub_ids=None,
            mls_verbosity=None,
            metrics_cadence_ms=None,
            routing_diagnostic=None,
            enable_poll_queue=None,
            mls_sampling_bypass=None,
        )
        # Pin the new sink BEFORE the FFI call so a successful Rust-side
        # swap never observes an unpinned new sink. If the FFI call raises
        # the lock is acquired before the swap on the Rust side
        # (lib.rs::install_telemetry_sink), so the previous sink is still
        # the live one — unpin the new sink and let the caller see the
        # exception with state unchanged.
        self._prevent_gc.append(sink)
        try:
            self._protocol.install_telemetry_sink(sink, effective)
        except Exception:
            self._prevent_gc.remove(sink)
            raise

        # Rust has now dropped its handle on the prior sink; release the
        # corresponding Python pin. ``list.remove`` drops the first match
        # only, which correctly leaves the new pin in place even when the
        # caller re-installs the same sink instance.
        prev = getattr(self, "_telemetry_sink", None)
        if prev is not None:
            try:
                self._prevent_gc.remove(prev)
            except ValueError:
                logger.debug(
                    "prior telemetry sink missing from _prevent_gc",
                    exc_info=True,
                )
        self._telemetry_sink = sink

    def uninstall_telemetry_sink(self) -> None:
        """Detach the currently-installed telemetry sink, if any.

        Idempotent — safe to call without a prior install. If the
        underlying Rust call raises, the Python-side bookkeeping is left
        untouched so a retry sees the same state.
        """
        self._protocol.uninstall_telemetry_sink()
        sink = getattr(self, "_telemetry_sink", None)
        if sink is not None:
            try:
                self._prevent_gc.remove(sink)
            except ValueError:
                pass
            self._telemetry_sink = None

    def poll_telemetry_frame(self) -> str | None:
        """Pull the next queued telemetry frame as JSON, or ``None`` if the
        queue is empty.

        New records only enter the queue while a sink is installed with
        ``TelemetryConfig.enable_poll_queue`` left at its default (or
        explicitly ``True``); after ``uninstall_telemetry_sink`` or under a
        push-only sink, fresh emissions do not enqueue. Records queued
        prior to a re-install remain readable in FIFO order — call
        ``uninstall_telemetry_sink`` between sinks for a clean slate. The
        queue is bounded at 1024 slots (drop-oldest on overflow). Pair
        with the typed ``TelemetrySink`` push callbacks if you need
        guaranteed delivery.
        """
        return self._protocol.poll_telemetry_frame()

    def telemetry_install_id(self) -> str | None:
        """Stable, opaque per-install telemetry identifier (32 hex chars),
        derived from the SDK-managed persistent scrub secret. The secret
        itself never crosses the FFI and cannot be recovered from the id,
        so the id is safe to attach to telemetry as a device-grain key.

        Returns ``None`` until the persistent secret is available — i.e.
        before :meth:`start` wires secure storage via MLS initialization,
        or when persisting the secret failed this session (the id would
        not be stable across launches, so none is exposed). Unaffected by
        installing a telemetry sink or an app-supplied scrub secret.

        Note: while the id reveals nothing about the user or device, it
        is still a persistent per-install identifier — using it may need
        to be declared under your app's privacy disclosures.
        """
        return self._protocol.telemetry_install_id()

    # -- processing loop ------------------------------------------------------

    async def _process_loop(self) -> None:
        """Call ``protocol.process()`` every 100 ms (matches iOS/Android)."""
        try:
            while self._running:
                try:
                    self._protocol.process()
                    self._drain_incoming_messages()
                except Exception as exc:
                    logger.error("Process error: %s", exc)
                await asyncio.sleep(_PROCESS_INTERVAL)
        except asyncio.CancelledError:
            return

    def _drain_incoming_messages(self) -> None:
        """Drain up to 100 messages per tick and dispatch to the event handler.

        ``receive_message()`` returns JSON strings.  Each message is parsed
        and forwarded as a ``message_received`` event so that users only need
        to subscribe to the event handler (matching the iOS/Android pattern).

        Note: because the processing loop consumes this queue, calling
        ``protocol.receive_message()`` directly while ProtocolManager is
        running will always return ``None``.
        """
        handler = self._event_handler
        drained = 0
        while drained < _MAX_MESSAGES_PER_TICK:
            raw = self._protocol.receive_message()
            if raw is None:
                break
            drained += 1

            try:
                event = json.loads(raw)
                if isinstance(event, dict):
                    event.setdefault("type", "message_received")
                else:
                    event = {"type": "message_received", "raw": raw}
            except json.JSONDecodeError:
                event = {"type": "message_received", "raw": raw}

            # Resolve BLE peripheral peer identity from the sender field.
            # The BLE central path reads the DEVICE_ID characteristic to
            # learn the peer's user_id; the peripheral has no equivalent
            # mechanism, so we extract it from the first received message.
            if (
                self.ble_peripheral is not None
                and isinstance(event, dict)
                and event.get("type") == "message_received"
            ):
                sender = event.get("sender") or event.get("sender_id")
                if sender:
                    self.ble_peripheral.resolve_sender_identity(sender)

            if handler is not None:
                try:
                    handler(event)
                except Exception:
                    logger.debug("Event handler error for message", exc_info=True)
        if drained == _MAX_MESSAGES_PER_TICK:
            logger.warning(
                "Capped receiveMessage drain at %d for this tick",
                _MAX_MESSAGES_PER_TICK,
            )
