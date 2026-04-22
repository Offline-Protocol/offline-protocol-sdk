"""Internet transport manager using WebSocket.

Mirrors ``InternetManager.swift`` / ``InternetManager.kt`` — connects to a
relay server for internet-based message routing and drives the UniFFI
``OfflineProtocol`` internet transport methods.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
from typing import Any, Coroutine

import websockets
from websockets.asyncio.client import ClientConnection

from .transport_manager import TransportError, TransportManager, TransportState

logger = logging.getLogger(__name__)

# -- Constants (matching iOS InternetManager.swift) ---------------------------

_MESSAGE_POLL_INTERVAL = 0.1  # 100 ms
_RECONNECT_INITIAL_DELAY = 1.0  # seconds
_RECONNECT_MAX_DELAY = 30.0
_RECONNECT_BACKOFF_MULTIPLIER = 2.0
_PING_INTERVAL = 10.0  # seconds
_CONNECTION_TIMEOUT = 10.0
_MAX_CONSECUTIVE_FAILURES = 2
_MAX_CONCURRENT_SENDS = 50


class InternetManager(TransportManager):
    """WebSocket-based internet transport.

    Connects to a relay server, authenticates, and then continuously:
    * receives incoming messages and feeds them to the Rust protocol core
    * polls the protocol core for outgoing messages and sends them over WS

    Parameters
    ----------
    protocol:
        The ``OfflineProtocol`` UniFFI instance.
    device_id:
        The local device identifier (used for auth fallback and message routing).
    server_url:
        WebSocket URL of the relay server (e.g. ``ws://relay.example.com``).
    auth_token:
        Optional auth token (falls back to *device_id* if ``None``).
    auto_reconnect:
        Whether to reconnect on disconnection.
    max_reconnect_attempts:
        0 means infinite.
    """

    transport_id = "internet"
    transport_name = "Internet (WebSocket)"

    def __init__(
        self,
        protocol: Any,
        device_id: str,
        server_url: str | None = None,
        auth_token: str | None = None,
        auto_reconnect: bool = True,
        max_reconnect_attempts: int = 0,
        app_id: str = "offline-messenger",
    ) -> None:
        super().__init__()
        self._protocol = protocol
        self._device_id = device_id
        self._app_id = app_id
        self._server_url = server_url
        self._auth_token = auth_token
        self._auto_reconnect = auto_reconnect
        self._max_reconnect_attempts = max_reconnect_attempts

        # Connection state
        self._ws: ClientConnection | None = None
        self._connected = False
        self._authenticated = False

        # Reconnection state
        self._reconnect_attempts = 0
        self._current_reconnect_delay = _RECONNECT_INITIAL_DELAY

        # Failure tracking for DORS
        self._consecutive_send_failures = 0
        self._consecutive_ping_failures = 0

        # Metrics
        self._bytes_sent: int = 0
        self._bytes_received: int = 0
        self._messages_sent: int = 0
        self._messages_received: int = 0

        # Event loop — captured in start() for thread-safe scheduling
        self._loop: asyncio.AbstractEventLoop | None = None

        # Async tasks
        self._recv_task: asyncio.Task[None] | None = None
        self._poll_task: asyncio.Task[None] | None = None
        self._ping_task: asyncio.Task[None] | None = None
        self._send_tasks: set[asyncio.Task[None]] = set()
        # Fire-and-forget tasks scheduled from `_process_received` (auth /
        # connection-closed fallouts). Retained here so the event loop's
        # weak task table cannot GC them mid-execution.
        self._process_tasks: set[asyncio.Task[None]] = set()
        self._send_semaphore: asyncio.Semaphore = asyncio.Semaphore(_MAX_CONCURRENT_SENDS)
        self._reconnect_handle: asyncio.TimerHandle | None = None

        # Re-entrancy guard for `_handle_connection_closed`. Set synchronously
        # at the top of the coroutine so a second caller reaching it before
        # the first returns (e.g. AuthError process-task + recv-loop
        # ConnectionClosed both racing) deduplicates without relying on the
        # state-machine guard, which does not fire during reconnect
        # (state == STARTING).
        self._teardown_in_progress: bool = False

    # -- configuration --------------------------------------------------------

    def configure(
        self,
        server_url: str,
        auto_reconnect: bool = True,
        max_reconnect_attempts: int = 0,
    ) -> None:
        self._server_url = server_url
        self._auto_reconnect = auto_reconnect
        self._max_reconnect_attempts = max_reconnect_attempts
        self._emit_diagnostic("info", "Internet transport configured", {
            "server_url": server_url,
            "auto_reconnect": auto_reconnect,
        })

    def set_auth_token(self, token: str | None) -> None:
        self._auth_token = token
        self._emit_diagnostic("info", "Auth token updated", {
            "has_token": token is not None,
        })
        if self._connected and self._ws is not None:
            loop = self._loop
            if loop is not None and not loop.is_closed() and loop.is_running():
                loop.call_soon_threadsafe(
                    asyncio.ensure_future, self._send_authentication()
                )

    # -- TransportManager interface -------------------------------------------

    def is_available(self) -> bool:
        return self._server_url is not None

    async def start(self) -> None:
        if self._state == TransportState.RUNNING:
            raise TransportError("Internet transport is already running")
        if self._server_url is None:
            raise TransportError(
                "Server URL not configured. Call configure(server_url=...) first."
            )
        self._loop = asyncio.get_running_loop()
        self._emit_diagnostic("info", "Starting Internet transport", {
            "device_id": self._device_id,
            "server_url": self._server_url,
        })
        self._update_state(TransportState.STARTING)
        await self._connect()

    async def stop(self) -> None:
        if self._state not in (TransportState.RUNNING, TransportState.STARTING):
            return
        self._update_state(TransportState.STOPPING)

        # Cancel reconnect
        if self._reconnect_handle is not None:
            self._reconnect_handle.cancel()
            self._reconnect_handle = None

        # Cancel async tasks
        for task in (self._recv_task, self._poll_task, self._ping_task):
            if task is not None and not task.done():
                task.cancel()
        self._recv_task = self._poll_task = self._ping_task = None

        # Cancel in-flight send tasks — snapshot to avoid "Set changed size
        # during iteration" if done-callbacks fire concurrently.
        pending_sends = list(self._send_tasks)
        self._send_tasks.clear()
        for task in pending_sends:
            if not task.done():
                task.cancel()

        # Cancel in-flight process-received tasks (auth / close fallouts).
        pending_process = list(self._process_tasks)
        self._process_tasks.clear()
        for task in pending_process:
            if not task.done():
                task.cancel()

        # Close WebSocket
        await self._disconnect()

        # Notify protocol
        try:
            self._protocol.internet_status_changed(is_connected=False)
        except Exception:
            logger.debug("internet_status_changed(False) failed", exc_info=True)

        self._update_state(TransportState.STOPPED)
        self._emit_diagnostic("info", "Internet transport stopped")

    def get_metrics(self) -> dict[str, Any]:
        return {
            "bytes_sent": self._bytes_sent,
            "bytes_received": self._bytes_received,
            "messages_sent": self._messages_sent,
            "messages_received": self._messages_received,
            "is_connected": self._connected,
            "reconnect_attempts": self._reconnect_attempts,
        }

    # -- connection management ------------------------------------------------

    async def _connect(self) -> None:
        if self._connected or self._server_url is None:
            return
        try:
            extra_headers = {"X-Device-ID": self._device_id}
            self._ws = await asyncio.wait_for(
                websockets.connect(
                    self._server_url,
                    additional_headers=extra_headers,
                ),
                timeout=_CONNECTION_TIMEOUT,
            )
            self._connected = True
            self._authenticated = False
            self._reconnect_attempts = 0
            self._current_reconnect_delay = _RECONNECT_INITIAL_DELAY
            self._consecutive_send_failures = 0
            self._consecutive_ping_failures = 0

            # Start receive loop BEFORE auth so it's always cleaned up on
            # failure — prevents _recv_task staying None if auth throws.
            self._recv_task = asyncio.ensure_future(self._receive_loop())

            self._emit_diagnostic("info", "WebSocket connected, authenticating...")
            await self._send_authentication()

        except Exception as exc:
            self._emit_diagnostic("error", "WebSocket connection failed", {
                "error": str(exc),
            })
            await self._handle_connection_closed(exc)

    async def _disconnect(self) -> None:
        if self._ws is not None:
            try:
                await self._ws.close()
            except Exception:
                pass
            self._ws = None
        self._connected = False
        self._authenticated = False

    async def _send_authentication(self) -> None:
        if self._ws is None:
            return
        token = self._auth_token or self._device_id
        auth_msg = json.dumps({"type": "Authenticate", "token": token})
        try:
            await self._ws.send(auth_msg)
            self._emit_diagnostic("debug", "Auth message sent", {
                "user_id": self._device_id,
            })
        except Exception as exc:
            self._emit_diagnostic("error", f"Failed to send auth: {exc}")

    async def _safe_handle_authenticated(self, user_id: str, username: str) -> None:
        """Wrapper that catches exceptions to prevent stuck STARTING state."""
        try:
            await self._handle_authenticated(user_id, username)
        except Exception as exc:
            self._emit_diagnostic("error", f"Authentication handler failed: {exc}")
            await self._handle_connection_closed(exc)

    async def _safe_handle_connection_closed(self, error: Exception | None) -> None:
        """Wrapper that catches exceptions from connection-closed handling."""
        try:
            await self._handle_connection_closed(error)
        except Exception as exc:
            self._emit_diagnostic("error", f"Connection-closed handler failed: {exc}")

    async def _handle_authenticated(self, user_id: str, username: str) -> None:
        self._authenticated = True
        self._update_state(TransportState.RUNNING)

        # Notify protocol — triggers outbox flush
        try:
            self._protocol.internet_status_changed(is_connected=True)
        except Exception as exc:
            self._emit_diagnostic("error", f"Protocol notify failed: {exc}")

        # Cancel any existing poll/ping tasks before creating new ones
        # (guards against duplicate Authenticated messages from the server)
        for task in (self._poll_task, self._ping_task):
            if task is not None and not task.done():
                task.cancel()

        # Start polling and ping tasks
        self._poll_task = asyncio.ensure_future(self._poll_outgoing_loop())
        self._ping_task = asyncio.ensure_future(self._ping_loop())

        # Immediate flush
        self._poll_and_send_messages()

        self._emit_diagnostic("info", "Authenticated with relay server", {
            "user_id": user_id,
            "username": username,
        })

    async def _handle_connection_closed(self, error: Exception | None) -> None:
        # Re-entrancy guard. On AuthError + WS-close, two paths race here:
        # the fire-and-forget process-task from `_process_received` and the
        # recv-loop's `except ConnectionClosed` branch. The state-machine
        # guard below only fires when STOPPING/STOPPED, not during reconnect
        # (STARTING). The flag is set synchronously before any await, so
        # whichever caller gets here second returns immediately.
        if self._teardown_in_progress:
            return
        if (
            not self._connected
            and not self._authenticated
            and self._state in (TransportState.STOPPING, TransportState.STOPPED)
        ):
            return  # already handled post-stop

        self._teardown_in_progress = True
        try:
            was_connected = self._connected
            self._connected = False
            self._authenticated = False

            # Cancel recv/poll/ping tasks. Skip whichever (if any) is the
            # task currently running this coroutine — any task in the cancel
            # list can itself be the caller (recv-loop on ConnectionClosed,
            # ping-loop on repeated failure, send task on repeated send
            # failure). Self-cancel would surface CancelledError mid-await;
            # the running task exits naturally once this coroutine returns.
            current = asyncio.current_task()
            for task in (self._recv_task, self._poll_task, self._ping_task):
                if task is not None and task is not current and not task.done():
                    task.cancel()
            self._recv_task = self._poll_task = self._ping_task = None

            # Close the WebSocket now that no loop is reading from it.
            # Without this, the old socket would stay open across a
            # reconnect, and `_connect` would overwrite `self._recv_task` —
            # leaving a zombie recv loop draining the previous connection.
            await self._disconnect()

            if was_connected:
                try:
                    self._protocol.internet_status_changed(is_connected=False)
                except Exception:
                    logger.debug("internet_status_changed(False) failed", exc_info=True)

            self._emit_diagnostic("warning", "WebSocket disconnected", {
                "error": str(error) if error else "none",
            })

            if (
                self._auto_reconnect
                and self._state not in (TransportState.STOPPING, TransportState.STOPPED)
            ):
                self._schedule_reconnect()
            else:
                self._update_state(TransportState.STOPPED)
        finally:
            self._teardown_in_progress = False

    def _schedule_reconnect(self) -> None:
        if not self._auto_reconnect:
            return
        if (
            self._max_reconnect_attempts > 0
            and self._reconnect_attempts >= self._max_reconnect_attempts
        ):
            self._emit_diagnostic("error", "Max reconnect attempts reached")
            self._update_state(TransportState.STOPPED)
            return

        self._reconnect_attempts += 1
        delay = self._current_reconnect_delay
        self._current_reconnect_delay = min(
            self._current_reconnect_delay * _RECONNECT_BACKOFF_MULTIPLIER,
            _RECONNECT_MAX_DELAY,
        )

        # Transition to STARTING so callers see a consistent state while
        # waiting for the reconnect timer (avoids RUNNING + _connected=False).
        self._update_state(TransportState.STARTING)

        self._emit_diagnostic("info", "Scheduling reconnect", {
            "attempt": self._reconnect_attempts,
            "delay_seconds": delay,
        })

        loop = self._loop
        if loop is None or loop.is_closed() or not loop.is_running():
            self._emit_diagnostic("warning", "No running event loop for reconnect")
            self._update_state(TransportState.STOPPED)
            return

        # Cancel any prior handle before overwriting so concurrent
        # `_schedule_reconnect` callers don't orphan a timer that would still
        # fire and race a second `_connect()` against the first.
        if self._reconnect_handle is not None:
            self._reconnect_handle.cancel()
        self._reconnect_handle = loop.call_later(
            delay,
            lambda: asyncio.ensure_future(self._connect()),
        )

    # -- receive loop ---------------------------------------------------------

    async def _receive_loop(self) -> None:
        """Continuously receive messages from the WebSocket."""
        try:
            if self._ws is None:
                return
            async for raw in self._ws:
                if isinstance(raw, bytes):
                    data = raw
                else:
                    data = raw.encode("utf-8")
                self._bytes_received += len(data)
                self._process_received(data)
        except websockets.ConnectionClosed:
            await self._handle_connection_closed(None)
        except asyncio.CancelledError:
            return
        except Exception as exc:
            await self._handle_connection_closed(exc)

    def _spawn_process_task(self, coro: Coroutine[Any, Any, None]) -> None:
        """Schedule a fire-and-forget coroutine and retain a strong ref.

        asyncio only holds weak references to tasks, so an untracked
        ``ensure_future`` can be GC'd mid-await. `_process_tasks` keeps the
        reference alive until completion.
        """
        task = asyncio.ensure_future(coro)
        self._process_tasks.add(task)
        task.add_done_callback(self._process_tasks.discard)

    def _process_received(self, data: bytes) -> None:
        """Dispatch an incoming WebSocket frame to the protocol core."""
        try:
            msg = json.loads(data)
        except json.JSONDecodeError:
            self._emit_diagnostic("warning", "Non-JSON message received")
            return

        msg_type = msg.get("type")
        if msg_type is None:
            return

        if msg_type == "Authenticated":
            user_id = msg.get("user_id", self._device_id)
            username = msg.get("username", self._device_id)
            self._spawn_process_task(
                self._safe_handle_authenticated(user_id, username)
            )

        elif msg_type == "AuthError":
            reason = msg.get("reason", "Unknown")
            self._emit_diagnostic("error", f"Auth failed: {reason}")
            self._spawn_process_task(self._safe_handle_connection_closed(None))

        elif msg_type == "MessageReceived":
            self._handle_incoming_message(msg)

        elif msg_type == "ConnectionRequestReceived":
            self._handle_connection_request(msg)

        elif msg_type == "ConnectionAccepted":
            self._handle_connection_accepted(msg)

        elif msg_type == "ConnectionRejected":
            self._handle_connection_rejected(msg)

        elif msg_type == "DeliveryError":
            recipient = msg.get("recipient", "unknown")
            reason = msg.get("reason", "unknown")
            self._emit_diagnostic("warning", "Delivery failed", {
                "recipient": recipient,
                "reason": reason,
            })

        elif msg_type in ("GroupCreated", "GroupInvitation", "GroupMessageReceived"):
            self._handle_group_message(msg_type, msg)

        else:
            self._emit_diagnostic("debug", f"Unhandled relay message type: {msg_type}")

    # -- incoming message handlers --------------------------------------------

    def _handle_incoming_message(self, msg: dict[str, Any]) -> None:
        sender_id = msg.get("sender", "")
        content = msg.get("content", "")
        message_id = msg.get("message_id")
        reply_to_msg = msg.get("reply_to_msg")

        if not sender_id:
            return

        self._messages_received += 1

        # Try to parse content as full Message JSON, otherwise wrap it
        is_full_message = False
        message_dict: dict[str, Any] = {}
        try:
            content_json = json.loads(content)
            if isinstance(content_json, dict) and "sender" in content_json and "recipient" in content_json:
                message_dict = content_json
                if message_id and "id" not in message_dict:
                    message_dict["id"] = message_id
                if reply_to_msg and "reply_to_msg" not in message_dict:
                    message_dict["reply_to_msg"] = reply_to_msg
                is_full_message = True
        except json.JSONDecodeError:
            pass
        if not is_full_message:
            message_dict = {
                "sender": sender_id,
                "recipient": self._device_id,
                "content": content,
                "app_id": self._app_id,
                "priority": "Medium",
                "ttl": 8,
                "hop_count": 0,
                "requires_ack": True,
            }
            if message_id:
                message_dict["id"] = message_id
            if reply_to_msg:
                message_dict["reply_to_msg"] = reply_to_msg

        data_bytes = json.dumps(message_dict).encode("utf-8")
        try:
            self._protocol.internet_message_received(
                sender_id=sender_id, data=list(data_bytes)
            )
        except Exception as exc:
            self._emit_diagnostic("error", f"Error feeding message to protocol: {exc}")

    def _handle_connection_request(self, msg: dict[str, Any]) -> None:
        sender_id = msg.get("sender", "")
        if not sender_id:
            return
        sender_name = msg.get("sender_name", sender_id)
        payload = {"sender_name": sender_name}
        kp = msg.get("key_package")
        if kp is not None:
            payload["key_package"] = [b & 0xFF for b in kp]
        content = "__CONN_REQ__" + json.dumps(payload)
        data_bytes = json.dumps(
            self._build_internal_message(sender_id, content)
        ).encode("utf-8")
        try:
            self._protocol.internet_message_received(
                sender_id=sender_id, data=list(data_bytes)
            )
        except Exception as exc:
            self._emit_diagnostic("error", f"Error processing ConnectionRequest: {exc}")

    def _handle_connection_accepted(self, msg: dict[str, Any]) -> None:
        accepted_by = msg.get("accepted_by") or msg.get("sender", "")
        if not accepted_by:
            return
        accepted_by_name = (
            msg.get("accepted_by_name") or msg.get("sender_name", accepted_by)
        )
        payload: dict[str, Any] = {"accepted_by_name": accepted_by_name}
        kp = msg.get("key_package")
        if kp is not None:
            payload["key_package"] = [b & 0xFF for b in kp]
        content = "__CONN_ACC__" + json.dumps(payload)
        data_bytes = json.dumps(
            self._build_internal_message(accepted_by, content)
        ).encode("utf-8")
        try:
            self._protocol.internet_message_received(
                sender_id=accepted_by, data=list(data_bytes)
            )
        except Exception as exc:
            self._emit_diagnostic("error", f"Error processing ConnectionAccepted: {exc}")

    def _handle_connection_rejected(self, msg: dict[str, Any]) -> None:
        rejected_by = msg.get("rejected_by") or msg.get("sender", "")
        if not rejected_by:
            return
        content = "__CONN_REJ__"
        data_bytes = json.dumps(
            self._build_internal_message(rejected_by, content)
        ).encode("utf-8")
        try:
            self._protocol.internet_message_received(
                sender_id=rejected_by, data=list(data_bytes)
            )
        except Exception as exc:
            self._emit_diagnostic("error", f"Error processing ConnectionRejected: {exc}")

    def _handle_group_message(self, msg_type: str, msg: dict[str, Any]) -> None:
        group_id = msg.get("group_id", "")
        if not group_id:
            return
        sender_id = msg.get("sender", "system")
        content_prefix = {
            "GroupCreated": "__GRP_CREATED__",
            "GroupInvitation": "__GRP_INVITE__",
            "GroupMessageReceived": "__GRP_MSG__",
        }.get(msg_type, "")
        content = content_prefix + json.dumps(msg)
        data_bytes = json.dumps(
            self._build_internal_message(sender_id, content)
        ).encode("utf-8")
        try:
            self._protocol.internet_message_received(
                sender_id=sender_id, data=list(data_bytes)
            )
        except Exception as exc:
            self._emit_diagnostic("error", f"Error processing {msg_type}: {exc}")

    def _build_internal_message(
        self, sender_id: str, content: str
    ) -> dict[str, Any]:
        return {
            "sender": sender_id,
            "recipient": self._device_id,
            "content": content,
            "app_id": self._app_id,
            "priority": "Medium",
            "ttl": 8,
            "hop_count": 0,
            "requires_ack": False,
        }

    # -- outgoing message poll loop -------------------------------------------

    async def _poll_outgoing_loop(self) -> None:
        """Poll the protocol core for outgoing messages every 100 ms."""
        try:
            while self._connected and self._authenticated:
                self._poll_and_send_messages()
                await asyncio.sleep(_MESSAGE_POLL_INTERVAL)
        except asyncio.CancelledError:
            return

    def _poll_and_send_messages(self) -> None:
        """Drain outgoing messages from the protocol and send them.

        Only dequeues up to the number of available semaphore slots to
        prevent unbounded task creation when the protocol core has a large
        outbox.  Remaining messages will be picked up on the next poll tick.
        """
        # Limit to available concurrency slots to avoid creating thousands
        # of tasks that all block on the semaphore.
        available_slots = max(0, _MAX_CONCURRENT_SENDS - len(self._send_tasks))
        drained = 0
        while drained < available_slots:
            try:
                msg = self._protocol.internet_get_next_message()
            except (AttributeError, ReferenceError) as exc:
                logger.error("Protocol instance invalid: %s", exc)
                break
            except Exception as exc:
                logger.warning("Unexpected error polling outgoing messages: %s", exc)
                break
            if msg is None:
                break

            message_id = msg.message_id
            recipient = msg.recipient_id
            data = bytes(msg.data)

            task = asyncio.ensure_future(
                self._send_message(message_id, recipient, data)
            )
            self._send_tasks.add(task)
            task.add_done_callback(self._send_tasks.discard)
            drained += 1

    def _notify_send_failed(self, message_id: str, reason: str) -> None:
        """Best-effort notification to the protocol that a send failed."""
        try:
            self._protocol.internet_send_failed_with_reason(
                message_id=message_id, reason=reason
            )
        except Exception:
            try:
                self._protocol.internet_send_failed(message_id=message_id)
            except Exception:
                logger.debug("internet_send_failed also failed", exc_info=True)

    async def _send_message(
        self, message_id: str, recipient: str, data: bytes
    ) -> None:
        ws = self._ws
        if ws is None or not self._connected:
            self._notify_send_failed(message_id, "Not connected")
            return

        async with self._send_semaphore:
            # Re-check after yielding to the semaphore — connection may have
            # been torn down while we were waiting for a slot.
            ws = self._ws
            if ws is None or not self._connected:
                self._notify_send_failed(message_id, "Disconnected while waiting")
                return

            try:
                try:
                    content = data.decode("utf-8")
                except UnicodeDecodeError:
                    content = base64.b64encode(data).decode("ascii")
                payload = json.dumps({
                    "type": "SendMessage",
                    "recipient": recipient,
                    "content": content,
                })
                await ws.send(payload)
                self._bytes_sent += len(payload)
                self._messages_sent += 1
                self._consecutive_send_failures = 0

                self._protocol.internet_confirm_sent(message_id=message_id)

            except Exception as exc:
                self._consecutive_send_failures += 1
                self._emit_diagnostic("error", f"Send failed: {exc}", {
                    "message_id": message_id,
                    "consecutive_failures": self._consecutive_send_failures,
                })
                self._notify_send_failed(message_id, str(exc))

                if self._consecutive_send_failures >= _MAX_CONSECUTIVE_FAILURES:
                    self._emit_diagnostic(
                        "error", "Too many consecutive send failures, disconnecting"
                    )
                    await self._handle_connection_closed(exc)

    # -- ping loop ------------------------------------------------------------

    async def _ping_loop(self) -> None:
        """Send WebSocket pings at regular intervals."""
        try:
            while self._connected and self._authenticated:
                await asyncio.sleep(_PING_INTERVAL)
                if self._ws is not None:
                    try:
                        await self._ws.ping()
                        self._consecutive_ping_failures = 0
                    except Exception:
                        self._consecutive_ping_failures += 1
                        if self._consecutive_ping_failures >= _MAX_CONSECUTIVE_FAILURES:
                            self._emit_diagnostic(
                                "error",
                                "Too many ping failures, disconnecting",
                            )
                            await self._handle_connection_closed(None)
                            return
        except asyncio.CancelledError:
            return
