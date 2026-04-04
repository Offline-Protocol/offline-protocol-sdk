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
    OfflineProtocol,
    ProtocolConfig,
    WifiDirectTransportCallback,
)
from .ble_manager import BleManager
from .ble_peripheral import BlePeripheral
from .internet_manager import InternetManager
from .secure_storage import SecureStorage

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
        if self._ble is not None:
            self._ble.on_fragments_available()
        if self._peripheral is not None:
            self._peripheral.on_fragments_available()


class _WifiDirectTransportCallbackImpl(WifiDirectTransportCallback):
    """Stub — WiFi Direct is not implemented on desktop."""

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
    """

    def __init__(
        self,
        config: ProtocolConfig,
        event_handler: Callable[[dict[str, Any]], None] | None = None,
        storage: Any | None = None,
    ) -> None:
        self._config = config
        self._event_handler = event_handler
        self._storage = storage or SecureStorage()

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

        # Keep strong references to all objects passed to the Rust/UniFFI
        # side so that Python's GC cannot collect them while Rust holds
        # raw callback pointers.
        self._prevent_gc = [
            self._event_cb,
            self._ble_cb,
            self._wifi_cb,
            self._storage,
        ]

        # Initialise MLS encryption
        try:
            self._protocol.initialize_mls(self._storage)
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

        # Stop protocol engine
        try:
            self._protocol.stop()
        except Exception:
            pass

        # Release GC-prevention references now that Rust no longer calls back.
        self._prevent_gc.clear()

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
        """Send a text message and return its message ID."""
        return self._protocol.send_message(
            recipient=recipient,
            content=content,
            priority=priority,
            reply_to_msg=reply_to,
        )

    def get_state(self) -> Any:
        """Return the current protocol state."""
        return self._protocol.get_state()

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
            if handler is not None:
                try:
                    event = json.loads(raw)
                    if isinstance(event, dict):
                        event.setdefault("type", "message_received")
                    else:
                        event = {"type": "message_received", "raw": raw}
                except json.JSONDecodeError:
                    event = {"type": "message_received", "raw": raw}
                try:
                    handler(event)
                except Exception:
                    logger.debug("Event handler error for message", exc_info=True)
        if drained == _MAX_MESSAGES_PER_TICK:
            logger.warning(
                "Capped receiveMessage drain at %d for this tick",
                _MAX_MESSAGES_PER_TICK,
            )
