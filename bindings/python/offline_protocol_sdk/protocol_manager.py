"""High-level protocol manager for desktop platforms.

Mirrors ``OfflineProtocolModule.swift`` / ``OfflineProtocolModule.kt`` —
wraps the UniFFI ``OfflineProtocol`` instance with transport managers, a
100 ms processing loop, event dispatch, and lifecycle management.
"""

from __future__ import annotations

import asyncio
import json
import logging
import platform
import sys
from pathlib import Path
from typing import Any, Callable

from .offline_protocol import (
    BleTransportCallback,
    EventCallback,
    MessagePriority,
    MlsVerbosity,
    NostrTransportCallback,
    OfflineProtocol,
    ProtocolConfig,
    AppState,
    ReticulumTransportCallback,
    TelemetryConfig,
    TelemetryOs,
    TelemetryStats,
    WifiDirectTransportCallback,
)
from .ble_manager import BleManager
from .ble_peripheral import BlePeripheral
from .internet_manager import InternetManager
from .secure_storage import SecureStorage
from .state_storage import AppStateStorage
from .storage_namespace import account_storage_namespace

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

        async with ProtocolManager(config, state_root="/app/install/state") as pm:
            pm.on_event(lambda e: print(e))
            pm.internet.configure(server_url="ws://relay.example.com")
            await pm.internet.start()
            pm.send_message("peer-id", "hello")

    Or without ``async with``::

        pm = ProtocolManager(config, state_root="/app/install/state")
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
        state. When omitted, ``state_root`` or ``OFFLINE_PROTOCOL_STATE_ROOT``
        must select an application-owned directory that the installer removes
        with the application.
    state_root:
        Root directory for the built-in :class:`AppStateStorage`. Ignored when
        ``state_storage`` is supplied.
    """

    def __init__(
        self,
        config: ProtocolConfig,
        event_handler: Callable[[dict[str, Any]], None] | None = None,
        storage: Any | None = None,
        state_storage: Any | None = None,
        *,
        state_root: str | Path | None = None,
    ) -> None:
        self._config = config
        self._event_handler = event_handler

        profile = config.profile  # type: ignore[union-attr]
        app_id = getattr(config, "app_id", "offline-messenger")
        storage_namespace = account_storage_namespace(app_id, profile)
        self._storage = (
            storage
            if storage is not None
            else SecureStorage(namespace=storage_namespace)
        )
        self._state_storage = (
            state_storage
            if state_storage is not None
            else AppStateStorage(root=state_root, namespace=storage_namespace)
        )

        # Create the core protocol instance
        self._protocol = OfflineProtocol(config)

        # Transport managers still carry the profile rather than this device's
        # address: the address is derived when `start()` initialises MLS, which
        # is after this point. Moving the transport surfaces onto the address
        # is done as one piece with the rest of the transport identity work.
        device_id = profile

        # Transport managers
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

    @property
    def local_address(self) -> str | None:
        """This device's derived ``off1…`` address, or ``None`` before start.

        Derived from the identity key in this profile's storage rather than
        chosen by the caller, and stable across restarts of the same profile.
        """
        return self._protocol.local_address()

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

        logger.info("ProtocolManager started (address=%s)", self.local_address)

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

        # Give the telemetry pipe its final flush while the process is still
        # ours to block: the pipe would also stop when the protocol is
        # dropped, but an explicit disable is what puts the last batch on the
        # wire rather than in the durable queue. Idempotent on the Rust side.
        teardown_clean = True
        try:
            # Off the loop: the final flush blocks for up to three seconds,
            # and every other awaitable in this teardown would wait behind it.
            await asyncio.to_thread(self.disable_telemetry)
        except Exception:
            teardown_clean = False
            logger.debug(
                "disable_telemetry raised during stop (non-fatal)",
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

    def enable_telemetry(
        self,
        api_key: str,
        app_id: str,
        app_version: str | None = None,
        *,
        debug: bool | None = None,
        flush_interval_ms: int | None = None,
        max_batch_bytes: int | None = None,
        max_buffered_records: int | None = None,
        include_device_id: bool | None = None,
        scrub_ids: bool | None = None,
        mls_verbosity: MlsVerbosity | None = None,
        metrics_cadence_ms: int | None = None,
        routing_diagnostic: bool | None = None,
        mls_sampling_bypass: bool | None = None,
        app_state: AppState = AppState.ACTIVE,
    ) -> None:
        """Enable telemetry: the SDK collects, batches and uploads accepted
        events to the Offline Protocol ingest itself, on a background thread
        inside the native library.

        ``api_key`` and ``app_id`` come from the developer portal. The host
        platform is filled in from :mod:`platform`, never by the caller.
        Every keyword argument left ``None`` keeps the Rust default. The
        batch queue is durable from the first batch, because
        :meth:`start` attaches the protocol-state store before anything is
        emitted.

        Raises ``ProtocolError.TelemetryConfigInvalid`` naming the refused
        field. Calling it again replaces the running pipe after its final
        flush. See ``docs/telemetry.md`` for what leaves the device.
        """
        os_kind, os_major = _host_platform()
        config = TelemetryConfig(
            api_key=api_key,
            app_id=app_id,
            os=os_kind,
            os_major=os_major,
            app_version=app_version,
            debug=debug,
            flush_interval_ms=flush_interval_ms,
            max_batch_bytes=max_batch_bytes,
            max_buffered_records=max_buffered_records,
            include_device_id=include_device_id,
            scrub_ids=scrub_ids,
            mls_verbosity=mls_verbosity,
            metrics_cadence_ms=metrics_cadence_ms,
            routing_diagnostic=routing_diagnostic,
            mls_sampling_bypass=mls_sampling_bypass,
        )
        self._protocol.enable_telemetry(config, app_state)

    def disable_telemetry(self) -> None:
        """Disable telemetry after one final flush of up to three seconds.
        Idempotent."""
        self._protocol.disable_telemetry()

    def flush_telemetry(self) -> None:
        """Ask the uploader to send what is queued now. Returns at once."""
        self._protocol.flush_telemetry()

    def telemetry_stats(self) -> TelemetryStats | None:
        """The pipe's counters, or ``None`` while telemetry is not enabled.

        ``accepted_events`` is what the ingest reported accepting, which is
        what an invoice is reconciled against; ``dropped`` counts events
        lost to the ring buffer, the queue caps, the six-day expiry or a
        permanent rejection; ``last_error`` is the most recent send failure.
        """
        return self._protocol.telemetry_stats()

    def end_telemetry_session(self) -> None:
        """Close the current telemetry session: a summary, a flush of up to
        three seconds, then a fresh session id."""
        self._protocol.end_telemetry_session()

    def notify_app_state(self, state: AppState) -> None:
        """Report an application lifecycle transition. ``BACKGROUND`` closes
        the session and requests a flush; the first ``ACTIVE`` after it
        opens the next one. A desktop host that has no such lifecycle never
        needs to call this."""
        self._protocol.notify_app_state(state)

    def set_telemetry_enabled(self, enabled: bool) -> None:
        """Stop or resume collection without tearing the pipe down. Off,
        every emit costs one atomic load and nothing is buffered; what is
        already queued still drains."""
        self._protocol.set_telemetry_enabled(enabled)

    def telemetry_install_id(self) -> str | None:
        """Stable, opaque per-install telemetry identifier (32 hex chars),
        derived from the SDK-managed persistent scrub secret. The secret
        itself never crosses the FFI and cannot be recovered from the id.

        Returns ``None`` until the persistent secret is available, which is
        after :meth:`start` has wired secure storage. Stamped on telemetry
        batches only when ``include_device_id`` was set; that setting is a
        disclosure decision, see ``docs/privacy.md``.
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


def _host_platform() -> tuple[TelemetryOs, int]:
    """The host as the telemetry ingest names it, and its major version.

    Filled here rather than by the caller: the ingest keys its per-platform
    aggregates on these, and a value the application cannot set is a value
    it cannot set wrong.
    """
    system = platform.system()
    if system == "Darwin":
        release = platform.mac_ver()[0] or "0"
        return TelemetryOs.MACOS, _major(release)
    if system == "Linux":
        return TelemetryOs.LINUX, _major(platform.release())
    if system == "Windows":
        # `platform.release()` answers "10" on Windows 11, so on its own it
        # would fold two releases into one bucket for the whole life of the
        # aggregate. The build number is what separates them: 22000 is the
        # first Windows 11 build, and Microsoft never moved it.
        try:
            build = sys.getwindowsversion().build  # type: ignore[attr-defined]
        except (AttributeError, OSError):
            build = 0
        return TelemetryOs.WINDOWS, 11 if build >= 22000 else _major(platform.release())
    return TelemetryOs.OTHER, 0


def _major(version: str) -> int:
    head = version.split(".", 1)[0]
    digits = "".join(ch for ch in head if ch.isdigit())
    try:
        return max(0, min(int(digits), 65535)) if digits else 0
    except ValueError:
        return 0
