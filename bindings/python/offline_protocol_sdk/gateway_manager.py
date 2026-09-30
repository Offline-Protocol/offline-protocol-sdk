"""A gateway daemon client for the ``reticulum`` transport slot.

Speaks the gateway-daemon contract (``docs/spec/gateway-contract.md``) over
TCP to a daemon on local IP: newline-delimited JSON, an attach that binds
the session to this device's address, one verdict per submitted frame, and
presence for the peers the core is waiting to hear about. What answers on
the far end is a daemon built to that contract; this module is the device
half.

A port of ``ReticulumManager.swift`` (the port source) and
``ReticulumManager.kt``, over asyncio. The decisions and frame shapes live
in ``gateway_attach_policy.py``; what is here is the socket, the tasks and
the lifecycle.

Invariants this manager keeps, each of which was a defect on some earlier
client of this contract:

* **The carrier is announced only for a bound session, and in the
  contract's order.** ``reticulum_status_changed(True)`` is called from one
  place, on ``StatusUpdate(connected)``, and only after the gateway echoed
  our own address back. Its capabilities are handed to the core on their own
  frame, which the contract puts before the announcement, so on a conforming
  gateway the flush the announcement triggers sees them. A session the
  gateway never bound is verdict-only on the other side: it may submit and
  be told ``attach_required``, and nothing addressed to this device ever
  arrives over it. Offering that to the selector would be offering a
  transport that can only refuse.
* **The write is not the outcome.** A frame is confirmed or failed on the
  gateway's verdict, correlated by id, never on the socket write. Confirming
  on the write is what the first bridges did, and it is why
  ``recipient_unreachable``, the one verdict that parks a message and offers
  it to the mesh, never reached the core from them.
* **Every frame in flight is owed an outcome.** A connection that closes
  fails what it was carrying, and a gateway that answers nothing is failed
  at the verdict timeout, which stays under the core's own expiry.
* **A session generation is checked after every suspension.** Each socket
  claims a fresh generation and every handler captures it, because an
  ``await`` can resume after the socket that carried the frame is gone and
  its successor is up. A stale ``StatusUpdate(connected)`` acted on then
  would announce the successor before the gateway bound it; a stale
  ``Challenge`` would sign the old challenge onto the new socket.
* **The reconnect ladder resets only on a bound and announced session.** A
  TCP open proves that something is listening; the handshake that follows
  has four places left to fail. Reset on the open, a refusing gateway was
  retried at the floor forever.

The module names no signing domain: the proof is built in the core behind
``gateway_address_declaration``, and a Rust guard reads this file to keep
it that way.
"""

from __future__ import annotations

import asyncio
import base64
import binascii
import json
import logging
import time
from typing import Any, Coroutine

from . import gateway_attach_policy as policy
from .gateway_verdict_tracker import GatewayVerdictTracker
from .presence_watch_policy import DEFAULT_TICK_INTERVAL, PresenceWatchPolicy
from .transport_manager import TransportError, TransportManager, TransportState

logger = logging.getLogger(__name__)

#: Fallback drain cadence, in seconds. The primary send path is the core's
#: wake (``on_messages_available``); this tick is what runs the verdict
#: sweep and catches a wake that was lost to a pause.
MESSAGE_POLL_INTERVAL = 5.0
RECONNECT_INITIAL_DELAY = 1.0
RECONNECT_MAX_DELAY = 30.0
RECONNECT_BACKOFF_MULTIPLIER = 2.0
#: Seconds the TCP open may take. Long, because the address may be a
#: gateway box on venue Wi-Fi rather than this host; the attach that follows
#: has its own, much shorter, deadline.
CONNECTION_TIMEOUT = 60.0
MAX_CONSECUTIVE_FAILURES = 3
#: Frames handed to the gateway per drain pass, before yielding.
MAX_BATCH_SIZE = 10
DEFAULT_DAEMON_ADDRESS = "localhost:4242"

_LOCALHOST_ALIASES = frozenset({"localhost", "127.0.0.1", "::1"})


def parse_daemon_address(daemon_address: str) -> tuple[str, int]:
    """``host:port``, ``[v6]:port``, or a bare host with the default port."""
    text = daemon_address.strip() or DEFAULT_DAEMON_ADDRESS
    default_port = int(DEFAULT_DAEMON_ADDRESS.rsplit(":", 1)[1])
    if text.startswith("["):
        host, _, rest = text[1:].partition("]")
        port_text = rest[1:] if rest.startswith(":") else ""
    elif text.count(":") == 1:
        host, _, port_text = text.partition(":")
    else:
        host, port_text = text, ""
    host = host or "localhost"
    if not port_text:
        return host, default_port
    try:
        port = int(port_text)
    except ValueError:
        raise ValueError(f"daemon address {daemon_address!r} has no numeric port") from None
    if not 0 < port < 65536:
        raise ValueError(f"daemon address {daemon_address!r} has an out-of-range port")
    return host, port


class GatewayManager(TransportManager):
    """The gateway-daemon client behind the ``reticulum`` slot.

    Parameters
    ----------
    protocol:
        The ``OfflineProtocol`` UniFFI instance.
    device_id:
        What ``Identify`` carries when this device has no address yet. The
        address is used once there is one; only ``DeclareAddress`` binds
        either way.

    Configure with :meth:`configure` and start after
    ``ProtocolManager.start()``, once this device has an identity to
    declare; ``ProtocolManager.stop()`` stops it with everything else.
    """

    transport_id = "reticulum"
    transport_name = "Gateway daemon (the reticulum slot)"

    def __init__(self, protocol: Any, device_id: str) -> None:
        super().__init__()
        self._protocol = protocol
        self._device_id = device_id
        self._daemon_host = "localhost"
        self._daemon_port = 4242
        self._auto_reconnect = True
        self._max_reconnect_attempts = 0
        self._configured = False

        self._loop: asyncio.AbstractEventLoop | None = None
        self._reader: asyncio.StreamReader | None = None
        self._writer: asyncio.StreamWriter | None = None

        # Which gateway session the current socket belongs to. Bumped when a
        # connect claims the flags and on every retire, so a socket never
        # shares a number with the one it replaces.
        self._generation = 0
        self._connected = False
        self._connecting = False
        # True once the gateway has echoed our own address back. This, not
        # the TCP connection, is what makes the carrier usable.
        self._bound = False
        self._paused = False

        self._reconnect_attempts = 0
        self._current_reconnect_delay = RECONNECT_INITIAL_DELAY
        self._consecutive_send_failures = 0

        self._verdicts = GatewayVerdictTracker()
        self._presence_watch = PresenceWatchPolicy()

        self._recv_task: asyncio.Task[None] | None = None
        self._poll_task: asyncio.Task[None] | None = None
        self._presence_task: asyncio.Task[None] | None = None
        self._connect_task: asyncio.Task[None] | None = None
        self._drain_task: asyncio.Task[None] | None = None
        self._drain_again = False
        self._drain_lock = asyncio.Lock()
        self._reconnect_handle: asyncio.TimerHandle | None = None
        self._attach_timeout_handle: asyncio.TimerHandle | None = None

        self._messages_sent = 0
        self._messages_received = 0

    # -- configuration --------------------------------------------------------

    def configure(
        self,
        daemon_address: str = DEFAULT_DAEMON_ADDRESS,
        auto_reconnect: bool = True,
        max_reconnect_attempts: int = 0,
    ) -> None:
        """Where the daemon listens, as ``host:port`` (default
        ``localhost:4242``). ``max_reconnect_attempts`` of 0 means
        unbounded."""
        self._daemon_host, self._daemon_port = parse_daemon_address(daemon_address)
        self._auto_reconnect = auto_reconnect
        self._max_reconnect_attempts = max_reconnect_attempts
        if self._daemon_host not in _LOCALHOST_ALIASES:
            # The TCP link is unencrypted: the daemon sees ciphertext and
            # routing metadata either way, but a non-local link exposes the
            # attach and the verdicts to the path.
            self._emit_diagnostic(
                "warning",
                "Gateway daemon is not on localhost; the TCP connection is unencrypted",
                {"daemon_host": self._daemon_host},
            )
        self._configured = True
        self._emit_diagnostic(
            "info",
            "Gateway transport configured",
            {
                "daemon_host": self._daemon_host,
                "daemon_port": self._daemon_port,
                "auto_reconnect": auto_reconnect,
                "max_reconnect_attempts": max_reconnect_attempts,
            },
        )

    @property
    def bound(self) -> bool:
        """True while the gateway holds this session bound to our address."""
        return self._bound

    # -- TransportManager interface -------------------------------------------

    def is_available(self) -> bool:
        # Only after configure(), so the selector is never offered an
        # unconfigured gateway transport.
        return self._configured

    async def start(self) -> None:
        if self._state in (TransportState.RUNNING, TransportState.STARTING):
            raise TransportError("Gateway transport is already running")
        if not self._configured:
            raise TransportError(
                "Daemon address not configured. Call configure(daemon_address=...) first."
            )
        self._loop = asyncio.get_running_loop()
        # An explicit start() means "run": a pause() from a previous session
        # must not leave this fresh transport connected-but-mute.
        self._paused = False
        self._emit_diagnostic(
            "info",
            "Starting gateway transport",
            {"device_id": self._device_id, "daemon": f"{self._daemon_host}:{self._daemon_port}"},
        )
        self._update_state(TransportState.STARTING)
        self._spawn_connect()

    async def stop(self) -> None:
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

        if self._reconnect_handle is not None:
            self._reconnect_handle.cancel()
            self._reconnect_handle = None
        pending = [
            task
            for task in (
                self._connect_task,
                self._drain_task,
                self._poll_task,
                self._presence_task,
            )
            if task is not None and not task.done()
        ]
        for task in (self._connect_task, self._drain_task):
            if task is not None and not task.done():
                task.cancel()
        self._connect_task = None
        self._drain_task = None

        self._stop_polling()
        # Closes the socket and retires the session: every id in flight is
        # failed with a reason, the presence watch stops, the attach timeout
        # is disarmed.
        recv = self._disconnect("Disconnected")
        if recv is not None:
            pending.append(recv)
        if pending:
            await asyncio.gather(*pending, return_exceptions=True)

        # Told to the core after the session is retired, in the order the
        # Swift manager keeps: the false lands after any true this session
        # announced, which is the correct final answer.
        try:
            self._protocol.reticulum_status_changed(is_connected=False)
        except Exception:
            logger.debug("reticulum_status_changed(False) failed", exc_info=True)

        self._update_state(TransportState.STOPPED)
        self._emit_diagnostic("info", "Gateway transport stopped")

    async def pause(self) -> None:
        # Set before the tasks are cancelled, and read by every path that
        # would re-arm them: the reconnect edge and the core's wake.
        self._paused = True
        self._stop_polling()
        # A backgrounded host must not keep spending on presence ticks; the
        # watchlist is rebuilt from the core after resume().
        self._stop_presence_watch()

    async def resume(self) -> None:
        self._paused = False
        if self._state == TransportState.RUNNING and self._connected:
            self._start_polling()
            # Only a bound session has anyone to ask. An unbound one is not
            # announced as a carrier either, so nothing waits on its answers.
            if self._bound:
                self._start_presence_watch()
                # Drains whatever queued during the pause: the core does not
                # re-issue its wake for messages it already announced.
                self._schedule_drain()

    def get_metrics(self) -> dict[str, Any]:
        return {
            "messages_sent": self._messages_sent,
            "messages_received": self._messages_received,
            "in_flight": self._verdicts.count,
            "is_connected": self._connected,
            "is_bound": self._bound,
            "reconnect_attempts": self._reconnect_attempts,
        }

    # -- the core's wake ------------------------------------------------------

    def on_messages_available(self) -> None:
        """``ReticulumTransportCallback.on_messages_available``: the core
        queued a frame for this carrier.

        The *primary* send path; the poll tick is the fallback. Invoked from
        a Rust callback thread, so the drain is scheduled onto the event
        loop rather than run here. Carries the pause check itself: without
        it a paused transport still drained a batch per wake for as long as
        the core kept announcing. The frames are not lost; they stay queued
        in the core and ``resume()`` drains them.
        """
        if self._paused:
            return
        self._schedule_drain()

    def _schedule_drain(self) -> None:
        loop = self._loop
        if loop is not None and not loop.is_closed() and loop.is_running():
            loop.call_soon_threadsafe(self._drain_soon)

    def _drain_soon(self) -> None:
        if self._drain_task is not None and not self._drain_task.done():
            # One drain at a time; a wake that lands mid-drain runs another
            # pass afterwards rather than a second concurrent one.
            self._drain_again = True
            return
        self._drain_task = self._spawn(self._drain_until_quiet())

    async def _drain_until_quiet(self) -> None:
        while True:
            self._drain_again = False
            await self._poll_and_send()
            if not self._drain_again:
                return

    # -- connection management ------------------------------------------------

    def _spawn(self, coro: Coroutine[Any, Any, None]) -> asyncio.Task[None]:
        return asyncio.ensure_future(coro)

    def _spawn_connect(self) -> None:
        if self._connect_task is not None and not self._connect_task.done():
            return
        self._connect_task = self._spawn(self._connect())

    async def _connect(self) -> None:
        # The attempt takes a fresh session generation as it claims the
        # flags, so a socket never shares a number with the one it replaces
        # even if nothing retired the predecessor in between.
        if self._connecting or self._connected:
            return
        self._connecting = True
        self._generation += 1
        generation = self._generation

        self._emit_diagnostic(
            "info",
            "Connecting to the gateway daemon",
            {"host": self._daemon_host, "port": self._daemon_port},
        )
        try:
            reader, writer = await asyncio.wait_for(
                asyncio.open_connection(
                    self._daemon_host,
                    self._daemon_port,
                    limit=policy.MAX_LINE_BYTES,
                ),
                timeout=CONNECTION_TIMEOUT,
            )
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            self._emit_diagnostic("error", "Gateway connection failed", {"error": str(exc)})
            self._handle_connection_closed(generation, exc)
            return

        # The claim refuses a late open for a session a stop() or a timeout
        # already retired; either way the socket is stray.
        if self._generation != generation:
            writer.close()
            return
        self._connected = True
        self._connecting = False
        self._reader, self._writer = reader, writer
        self._consecutive_send_failures = 0
        # Not a backoff reset. A TCP open proves only that something is
        # listening; the handshake that follows has four places left to
        # fail, and every one of them reconnects. Reset here, a refusing
        # gateway was retried at the floor forever, with a challenge, a
        # signature and two events spent per turn, and
        # `max_reconnect_attempts` never tripped because the count went back
        # to zero each time. The reset lives in `_complete_attach`, on the
        # bound session.
        self._emit_diagnostic("info", "Connected to the gateway daemon")

        self._recv_task = self._spawn(self._receive_loop(reader, generation))

        # `device_id` is this device's address where there is one. The first
        # clients sent the profile, a local storage-namespace selector that
        # is not an identity in any namespace the gateway knows; it was
        # logged and never routed on. Only `DeclareAddress` binds either way.
        identity = self._local_address() or self._device_id
        if not await self._write_line(policy.identify_json(identity), generation):
            self._handle_connection_closed(generation, None)
            return
        # Checked again after the write suspended: a session retired in
        # between belongs to nothing this attempt should touch.
        if self._generation != generation:
            return
        self._arm_attach_timeout(generation)

        # A stop() that landed while this connection was being established
        # has already told the core we are down and moved to STOPPED.
        # Announcing now would put the state back to RUNNING against a
        # transport nothing will ever tear down again. The socket is stray.
        if self._state in (TransportState.STOPPING, TransportState.STOPPED):
            self._disconnect("Disconnected")
            return
        self._update_state(TransportState.RUNNING)
        # Skipped while paused: a daemon that drops and reconnects during a
        # background stay reaches here, and this is the durable half of what
        # `_paused` closes. No status flip here: the carrier is announced in
        # `_complete_attach`, on a bound session.
        if not self._paused:
            self._start_polling()

    def _disconnect(self, reason: str) -> asyncio.Task[None] | None:
        """Closes the socket and ends the session it carried. Returns the
        receive task, if one is still running, for the caller to await."""
        recv = self._close_socket()
        self._connected = False
        self._connecting = False
        self._retire_session(reason)
        return recv

    def _close_socket(self) -> asyncio.Task[None] | None:
        writer = self._writer
        self._reader = None
        self._writer = None
        if writer is not None:
            try:
                writer.close()
            except Exception:
                logger.debug("closing the gateway socket raised", exc_info=True)
        recv = self._recv_task
        self._recv_task = None
        if recv is not None and not recv.done() and recv is not asyncio.current_task():
            recv.cancel()
            return recv
        return None

    def _retire_session(self, reason: str) -> None:
        """Ends the gateway session the current socket carried, whether this
        side is closing the socket or the socket has already gone.

        Shared by every close path so that all of them run it. Before it
        was shared, a daemon-side drop ran none of it: the session stayed
        bound for the successor to inherit, which defeated the attach
        timeout, the presence watch kept writing to a dead socket, and the
        ids in flight were never failed, so the core waited out its own
        expiry on each.
        """
        self._generation += 1
        self._bound = False
        self._cancel_attach_timeout()
        self._stop_presence_watch()
        self._fail_in_flight(reason)

    def _handle_connection_closed(self, generation: int, error: BaseException | None) -> None:
        """Ends the session ``generation`` names, if it is still the current
        one. A report for a session that is already over is dropped before
        it touches the flags: a stale close would otherwise clear
        ``_connected`` under a healthy successor, tell the core the
        transport is down, and start a reconnect ladder against it."""
        if self._generation != generation:
            return
        was_connected = self._connected
        was_connecting = self._connecting
        self._connected = False
        self._connecting = False
        if not (was_connected or was_connecting):
            return

        # The socket goes now, not when the reconnect fires. A refused or
        # mismatched session is still open on the gateway's side and it
        # keeps sending on it, including, after its grace, the
        # `StatusUpdate(connected)` of a session it never bound.
        self._close_socket()
        self._retire_session("Connection lost")
        self._stop_polling()

        self._emit_diagnostic(
            "warning",
            "Gateway daemon disconnected",
            {"error": str(error) if error else "none", "was_connected": was_connected},
        )
        try:
            self._protocol.reticulum_status_changed(is_connected=False)
        except Exception:
            logger.debug("reticulum_status_changed(False) failed", exc_info=True)

        if self._auto_reconnect and self._state not in (
            TransportState.STOPPING,
            TransportState.STOPPED,
        ):
            self._schedule_reconnect()
        else:
            self._update_state(TransportState.STOPPED)

    def _schedule_reconnect(self) -> None:
        if not self._auto_reconnect:
            return
        if (
            self._max_reconnect_attempts > 0
            and self._reconnect_attempts >= self._max_reconnect_attempts
        ):
            self._emit_diagnostic(
                "error",
                "Max reconnect attempts reached",
                {"attempts": self._reconnect_attempts, "max_attempts": self._max_reconnect_attempts},
            )
            self._update_state(TransportState.STOPPED)
            return

        self._reconnect_attempts += 1
        delay = self._current_reconnect_delay
        self._current_reconnect_delay = min(
            self._current_reconnect_delay * RECONNECT_BACKOFF_MULTIPLIER,
            RECONNECT_MAX_DELAY,
        )
        self._update_state(TransportState.STARTING)
        self._emit_diagnostic(
            "info",
            "Scheduling reconnect to the gateway daemon",
            {"attempt": self._reconnect_attempts, "delay_seconds": delay},
        )
        loop = self._loop
        if loop is None or loop.is_closed() or not loop.is_running():
            self._emit_diagnostic("warning", "No running event loop for reconnect")
            self._update_state(TransportState.STOPPED)
            return
        if self._reconnect_handle is not None:
            self._reconnect_handle.cancel()
        self._reconnect_handle = loop.call_later(delay, self._reconnect_now)

    def _reconnect_now(self) -> None:
        self._reconnect_handle = None
        if self._state in (TransportState.STOPPING, TransportState.STOPPED):
            return
        self._spawn_connect()

    # -- the attach timeout ---------------------------------------------------

    def _arm_attach_timeout(self, generation: int) -> None:
        """Bounds the handshake. Disarmed by ``StatusUpdate(connected)`` and
        by a teardown, and by nothing else: it is not gated on the bind,
        because a gateway that binds and then never announces is exactly
        the wedge this exists to end."""
        self._cancel_attach_timeout()
        loop = self._loop
        if loop is None:
            return
        self._attach_timeout_handle = loop.call_later(
            policy.ATTACH_TIMEOUT, self._attach_timed_out, generation
        )

    def _cancel_attach_timeout(self) -> None:
        handle = self._attach_timeout_handle
        self._attach_timeout_handle = None
        if handle is not None:
            handle.cancel()

    def _attach_timed_out(self, generation: int) -> None:
        self._attach_timeout_handle = None
        if not self._connected or self._generation != generation:
            return
        self._emit_diagnostic("error", "Gateway attach timed out before StatusUpdate(connected)")
        self._handle_connection_closed(generation, None)

    # -- receiving --------------------------------------------------------------

    async def _receive_loop(self, reader: asyncio.StreamReader, generation: int) -> None:
        """Reads frames off ``reader`` for the session ``generation`` names.

        The generation is the one this socket was armed under: a handler
        that closes the session ends the segment, and the lines after it
        belong to a socket this side has retired.
        """
        try:
            while self._generation == generation:
                try:
                    line = await reader.readline()
                except (asyncio.LimitOverrunError, ValueError):
                    # A partial line past the cap cannot be resynchronised:
                    # its tail would be read as a fresh line, so every frame
                    # after it is garbage. The connection goes instead.
                    self._emit_diagnostic(
                        "error",
                        "Over-long line from the gateway",
                        {"limit": policy.MAX_LINE_BYTES},
                    )
                    self._handle_connection_closed(generation, None)
                    return
                except asyncio.CancelledError:
                    raise
                except Exception as exc:
                    self._handle_connection_closed(generation, exc)
                    return
                if not line:
                    self._handle_connection_closed(generation, None)
                    return
                if self._generation != generation:
                    return
                stripped = line.strip()
                if not stripped:
                    continue
                try:
                    await self._process_line(stripped, generation)
                except asyncio.CancelledError:
                    raise
                except Exception as exc:
                    # A handler that raises must not end this task quietly.
                    # Ended quietly, nothing retires the session: the flags
                    # stay up, the poll loop keeps submitting, every frame
                    # fails at the verdict timeout, and nothing addressed to
                    # this device is delivered again until stop(). The
                    # remote-triggerable causes are handled per field
                    # (`utf8_bytes`); this is the net for the rest, and the
                    # ladder brings a fresh session.
                    self._emit_diagnostic(
                        "error",
                        "Gateway frame handler raised",
                        {"error": str(exc), "type": type(exc).__name__},
                    )
                    self._handle_connection_closed(generation, exc)
                    return
        except asyncio.CancelledError:
            return

    async def _process_line(self, line: bytes, generation: int) -> None:
        try:
            frame = json.loads(line)
        except (json.JSONDecodeError, UnicodeDecodeError):
            frame = None
        if not isinstance(frame, dict) or not isinstance(frame.get("type"), str):
            self._emit_diagnostic(
                "warning",
                "Received non-JSON data from the gateway daemon, skipping",
                {"size": len(line)},
            )
            return
        frame_type = frame["type"]

        if frame_type == "MessageReceived":
            self._handle_message_received(frame)
        elif frame_type == "Challenge":
            await self._handle_challenge(frame, generation)
        elif frame_type == "AddressDeclared":
            self._handle_address_declared(frame, generation)
        elif frame_type == "AddressError":
            self._handle_address_error(frame, generation)
        elif frame_type == "Capabilities":
            # Before the status flip, never after: the flush that flip
            # triggers has to see them.
            tokens = policy.capability_tokens(frame)
            try:
                self._protocol.reticulum_gateway_capabilities(capabilities=tokens)
            except Exception as exc:
                self._emit_diagnostic(
                    "error", "Gateway capability injection failed", {"error": str(exc)}
                )
        elif frame_type in ("MessageSent", "DeliveryError"):
            self._handle_verdict(frame, frame_type)
        elif frame_type == "PresenceStatus":
            self._handle_presence(frame)
        elif frame_type == "StatusUpdate":
            status = frame.get("status")
            self._emit_diagnostic("debug", "Gateway daemon status update", {"status": status})
            if status == "connected":
                # The gateway's half of the handshake is done, whatever is
                # decided below about ours.
                self._cancel_attach_timeout()
                self._complete_attach(generation)
        else:
            self._emit_diagnostic("debug", "Unknown gateway message type", {"type": frame_type})

    # -- the gateway handshake ------------------------------------------------

    async def _handle_challenge(self, frame: dict[str, Any], generation: int) -> None:
        """Signs the gateway's challenge and declares this device's address.

        The signing happens in the core: this hands it the challenge and
        gets back the three fields ``DeclareAddress`` carries. A failure
        here is not retried on this connection: the gateway spends a
        challenge per connection, and the next reconnect gets a fresh one.
        """
        outcome = policy.decode_challenge(frame)
        if isinstance(outcome, policy.Skip):
            self._emit_diagnostic(
                "warning", "Cannot declare an address to the gateway", {"reason": outcome.reason}
            )
            self._handle_connection_closed(generation, None)
            return
        try:
            declaration = self._protocol.gateway_address_declaration(list(outcome.challenge))
        except Exception as exc:
            # No identity yet, or a challenge the core refused to sign.
            # Either way this connection can only ever be verdict-only, so
            # it is not worth holding open.
            self._emit_diagnostic(
                "warning",
                "Cannot build the address declaration",
                {"reason": policy.SkipReason.SIGNING_FAILED, "error": str(exc)},
            )
            self._handle_connection_closed(generation, None)
            return
        line = policy.declaration_json(
            declaration.address, bytes(declaration.public_key), bytes(declaration.signature)
        )
        if self._generation != generation:
            return
        if not await self._write_line(line, generation):
            # A write that fails leaves no frame for the gateway to answer,
            # and waiting out the attach timeout to learn that is ten
            # seconds of a carrier the selector has been told nothing about.
            self._emit_diagnostic("error", "Failed to write the address declaration")
            self._handle_connection_closed(generation, None)

    def _handle_address_declared(self, frame: dict[str, Any], generation: int) -> None:
        """Checks what the gateway says it bound against what we hold.

        Both answers go to the core, which owns the security warning; what
        is decided here is narrower and is the manager's own: whether this
        carrier can be offered to the selector at all.
        """
        declared = frame.get("address")
        # Bounded before it reaches the core: the echo is remote-chosen and
        # the line it arrived on may be a mebibyte.
        encoded = policy.utf8_bytes(declared) if isinstance(declared, str) else None
        if not declared or encoded is None or len(encoded) > policy.MAX_ADDRESS_BYTES:
            self._emit_diagnostic(
                "warning", "Invalid AddressDeclared: missing, over-long or unencodable address"
            )
            return
        if self._generation != generation:
            return
        try:
            self._protocol.reticulum_address_declared(address=declared)
        except Exception:
            logger.debug("reticulum_address_declared failed", exc_info=True)

        outcome = policy.binding_outcome(declared, self._local_address())
        if outcome is policy.BindingOutcome.BOUND:
            # One step with the generation check: a teardown landing between
            # a check and a set leaves the successor bound on an echo it
            # never received.
            if self._generation != generation:
                return
            self._bound = True
            self._emit_diagnostic("info", "Gateway bound this session to our address")
        else:
            # The core has already reported this as a security warning. Here
            # it costs the connection: a gateway that bound an address we do
            # not control will attribute our frames to an identity we cannot
            # prove and answer presence about someone else, and reconnecting
            # is the only thing this side can do that might land somewhere
            # honest.
            self._emit_diagnostic("error", "Gateway bound a session to an address we do not hold")
            self._handle_connection_closed(generation, None)

    def _handle_address_error(self, frame: dict[str, Any], generation: int) -> None:
        """The gateway refused the declaration, so this session can only be
        told verdicts. The carrier is never announced; the connection goes
        and the existing backoff decides when to try again."""
        reason = frame.get("reason")
        if not isinstance(reason, str) or not reason:
            reason = "unspecified"
        try:
            self._protocol.reticulum_address_declaration_refused(reason=reason)
        except Exception:
            logger.debug("reticulum_address_declaration_refused failed", exc_info=True)
        self._emit_diagnostic(
            "warning",
            "Gateway refused the address declaration",
            {"reason": policy.bounded_reason(reason)},
        )
        self._handle_connection_closed(generation, None)

    def _complete_attach(self, generation: int) -> None:
        """Announces the carrier once the gateway has bound this session.

        Called on ``StatusUpdate(connected)``, which the contract puts after
        ``AddressDeclared`` and after ``Capabilities``, so by the time the
        core is told this transport is available it has already been told
        what the gateway can do, and the flush the false-to-true edge
        triggers sees them.

        A session the gateway refused to bind never reaches here: the
        refusal closes the connection instead.
        """
        if self._generation != generation:
            return
        if not self._bound:
            # A gateway that announces before it binds is not speaking the
            # contract, and a session it never binds is verdict-only.
            # Closing is the one action that can end somewhere usable.
            self._emit_diagnostic("error", "Gateway reported connected before binding the session")
            self._handle_connection_closed(generation, None)
            return
        if not self._connected or self._state in (
            TransportState.STOPPING,
            TransportState.STOPPED,
        ):
            return
        try:
            self._protocol.reticulum_status_changed(is_connected=True)
        except Exception as exc:
            self._emit_diagnostic("error", "Protocol notify failed", {"error": str(exc)})
        # The gateway bound and announced this session: this, not the TCP
        # open, is what proves the connection good and earns a backoff
        # reset.
        self._reconnect_attempts = 0
        self._current_reconnect_delay = RECONNECT_INITIAL_DELAY
        if self._paused:
            return
        self._start_presence_watch()
        self._schedule_drain()

    # -- verdicts ---------------------------------------------------------------

    def _handle_verdict(self, frame: dict[str, Any], frame_type: str) -> None:
        """Settles one submitted frame on the gateway's answer."""
        verdict = policy.parse_verdict(frame, frame_type)
        if verdict is None:
            self._emit_diagnostic(
                "warning", "Verdict with no message_id, ignored", {"type": frame_type}
            )
            return
        if not self._verdicts.settle(verdict.message_id):
            # Already settled: a duplicate, or an answer to a frame this
            # connection timed out on. Reporting it again would settle an id
            # the core has moved past.
            return
        report = policy.verdict_report(verdict)
        # The three core calls below are wrapped for one exception only. A
        # lone surrogate in a field the parser did not already sanitise is
        # refused by the FFI as it lowers the string, and a gateway can put
        # one in any field, so that costs the frame and never the session.
        # Anything else the core raises is a core in a state this manager
        # cannot reason about, and it propagates to the receive loop, which
        # closes the session: re-attaching fails every id in flight, so the
        # core hears an outcome for each rather than waiting out its expiry.
        try:
            if report.confirmed:
                self._protocol.reticulum_confirm_sent(message_id=verdict.message_id)
            else:
                # Verbatim. The core classifies on the token and discards
                # the rest at that boundary, so nothing here needs to
                # understand the gateway's wording.
                self._protocol.reticulum_send_failed_with_reason(
                    message_id=verdict.message_id, reason=report.reason
                )
                recipient = verdict.recipient
                if report.watch_recipient and recipient and not self._is_self_peer(recipient):
                    # Watch them: the gateway pushes a PresenceStatus when a
                    # watched peer attaches, and that answer is what
                    # un-parks the message this verdict just parked. And
                    # feed the verdict in as presence: the core parks on the
                    # verdict already; this is what emits the
                    # `presence_updated(offline)` an app renders a header
                    # from, labelled with the carrier that answered. Never
                    # for self: the core drops that too, but a malformed
                    # self-addressed frame should not cost a watch slot.
                    self._presence_watch.watch(recipient, self._now_ms())
                    self._protocol.reticulum_peer_presence(
                        peer_id=recipient, online=False, last_seen_ms=None
                    )
        except UnicodeEncodeError:
            self._emit_diagnostic(
                "warning", "Verdict with an unencodable field, dropped", {"type": frame_type}
            )
        # A verdict frees an in-flight slot, so the next frame goes out on
        # it. Unless paused: with eight in flight, a paused transport would
        # otherwise drain a batch per answer for the whole background stay.
        if self._paused:
            return
        self._schedule_drain()

    def _fail_in_flight(self, reason: str) -> None:
        """Fails every outstanding frame with ``reason``, on a connection
        going away or on a gateway that answered nothing."""
        for message_id in self._verdicts.drain_all():
            try:
                self._protocol.reticulum_send_failed_with_reason(
                    message_id=message_id, reason=reason
                )
            except Exception:
                logger.debug("reticulum_send_failed_with_reason failed", exc_info=True)

    def _settle_locally(self, message_id: str, reason: str) -> None:
        """Settles a frame that never reached the gateway, so no verdict is
        coming for it. Only reports to the core if this call is the one
        that took the id out of flight."""
        if not self._verdicts.settle(message_id):
            return
        try:
            self._protocol.reticulum_send_failed_with_reason(message_id=message_id, reason=reason)
        except Exception:
            logger.debug("reticulum_send_failed_with_reason failed", exc_info=True)

    def _sweep_expired_verdicts(self) -> None:
        """Fails frames the gateway never answered.

        The contract says a gateway MUST answer every submission, and
        silence is the one failure the core cannot see: it holds the frame
        in pending confirmation until its own 120 s expiry and counts it a
        failure then. Failing it here, at 60 s, puts it back on the retry
        ladder while the core still considers it live, which is why this
        timeout has to stay the shorter of the two.
        """
        stale = self._verdicts.expired(self._now_seconds(), policy.VERDICT_TIMEOUT)
        if not stale:
            return
        self._emit_diagnostic(
            "warning", "Gateway did not answer for submitted frames", {"count": len(stale)}
        )
        for message_id in stale:
            try:
                self._protocol.reticulum_send_failed_with_reason(
                    message_id=message_id, reason=policy.GATEWAY_SILENT
                )
            except Exception:
                logger.debug("reticulum_send_failed_with_reason failed", exc_info=True)

    # -- inbound frames -------------------------------------------------------

    def _handle_message_received(self, frame: dict[str, Any]) -> None:
        sender = frame.get("sender")
        content = frame.get("content")
        if (
            not isinstance(sender, str)
            or not sender
            or policy.utf8_bytes(sender) is None
            or not isinstance(content, str)
        ):
            self._emit_diagnostic(
                "warning", "Invalid MessageReceived: missing or unencodable sender, or no content"
            )
            return
        data: bytes | None
        if frame.get("encoding") == "base64":
            try:
                data = base64.b64decode(content, validate=True)
            except (binascii.Error, ValueError):
                self._emit_diagnostic("warning", "Invalid MessageReceived: malformed base64")
                return
        else:
            data = policy.utf8_bytes(content)
            if data is None:
                self._emit_diagnostic("warning", "Invalid MessageReceived: unencodable content")
                return
        try:
            self._protocol.reticulum_message_received(sender_id=sender, data=list(data))
            self._messages_received += 1
        except Exception as exc:
            self._emit_diagnostic(
                "error", "Error processing a gateway-delivered frame", {"error": str(exc)}
            )

    def _handle_presence(self, frame: dict[str, Any]) -> None:
        answer = policy.parse_presence(frame)
        if answer is None:
            self._emit_diagnostic("warning", "Invalid PresenceStatus, ignored")
            return
        if answer.online:
            self._presence_watch.unwatch(answer.peer)
        try:
            self._protocol.reticulum_peer_presence(
                peer_id=answer.peer, online=answer.online, last_seen_ms=answer.last_seen_ms
            )
        except Exception:
            logger.debug("reticulum_peer_presence failed", exc_info=True)

    # -- presence watch -------------------------------------------------------

    def _start_presence_watch(self) -> None:
        # Bound as well as unpaused: a stop() that lands between the status
        # flip and this call has already cleared the bind, and a tick armed
        # past it would ask a stopped transport's questions.
        if self._paused or not self._bound:
            return
        self._stop_presence_watch(clear=False)
        self._presence_task = self._spawn(self._presence_loop(self._generation))

    def _stop_presence_watch(self, clear: bool = True) -> None:
        task = self._presence_task
        self._presence_task = None
        if task is not None and not task.done() and task is not asyncio.current_task():
            task.cancel()
        if clear:
            self._presence_watch.clear()

    async def _presence_loop(self, generation: int) -> None:
        try:
            while self._generation == generation and self._bound and not self._paused:
                await asyncio.sleep(DEFAULT_TICK_INTERVAL)
                if self._generation != generation or not self._bound or self._paused:
                    return
                await self._presence_tick(generation)
        except asyncio.CancelledError:
            return

    async def _presence_tick(self, generation: int) -> None:
        """Asks the gateway about the peers the core is waiting to hear
        about. One frame for the whole batch. The core owns the list, every
        peer with an undelivered welcome and every recipient of a parked
        message, so the app is not asked to maintain one."""
        try:
            core_watchlist = list(self._protocol.reticulum_presence_watchlist())
        except Exception:
            logger.debug("reticulum_presence_watchlist failed", exc_info=True)
            core_watchlist = []
        self_address = self._local_address()
        candidates = [
            peer
            for peer in core_watchlist
            if peer and peer != self._device_id and peer != self_address
        ]
        peers = self._presence_watch.peers_to_query(candidates, self._now_ms())
        line = policy.check_presence_json(peers)
        if line is None:
            return
        await self._write_line(line, generation)

    # -- sending ----------------------------------------------------------------

    def _start_polling(self) -> None:
        # The pause gate lives here, at the one function that can violate
        # it, and not at the call sites.
        if self._paused:
            return
        self._stop_polling()
        self._poll_task = self._spawn(self._poll_loop(self._generation))

    def _stop_polling(self) -> None:
        task = self._poll_task
        self._poll_task = None
        if task is not None and not task.done() and task is not asyncio.current_task():
            task.cancel()

    async def _poll_loop(self, generation: int) -> None:
        try:
            while self._generation == generation and not self._paused:
                await self._poll_and_send()
                await asyncio.sleep(MESSAGE_POLL_INTERVAL)
        except asyncio.CancelledError:
            return

    async def _poll_and_send(self) -> None:
        async with self._drain_lock:
            if not self._bound or self._paused:
                return
            self._sweep_expired_verdicts()
            generation = self._generation
            sent = 0
            while sent < MAX_BATCH_SIZE and self._bound and self._generation == generation:
                # Bounded by what is unanswered, not only by the batch: a
                # gateway that is slow to answer must not have the whole
                # outbox handed to it, because every id in flight is one the
                # core cannot retry until it is settled.
                if self._verdicts.count >= policy.MAX_IN_FLIGHT:
                    return
                try:
                    message = self._protocol.reticulum_get_next_message()
                except Exception as exc:
                    logger.warning("Unexpected error polling outgoing messages: %s", exc)
                    return
                if message is None:
                    return
                # The core re-queues an unconfirmed frame under the same id
                # after its own acknowledgement timeout, and a verdict can
                # honestly take longer than that over a radio backbone.
                # Sending it again would forward the frame twice and, when
                # this copy timed out, fail an id the gateway had already
                # confirmed. Popping it was enough: the core's pending entry
                # is refreshed by the pop.
                if not self._verdicts.begin(message.message_id, self._now_seconds()):
                    continue
                await self._send_message(message, generation)
                sent += 1

    async def _send_message(self, message: Any, generation: int) -> None:
        """Writes one frame. **The write is not the outcome.**

        A successful write means the gateway has the bytes, which says
        nothing about whether it could forward them; the answer arrives
        later as a ``MessageSent`` or a ``DeliveryError`` and is settled in
        :meth:`_handle_verdict`. A *failed* write is settled here, because
        there is no frame on the wire for the gateway to answer about.
        """
        message_id = message.message_id
        recipient = message.recipient_id
        if not self._bound or self._writer is None:
            self._emit_diagnostic(
                "warning",
                "Cannot send message: not attached",
                {"message_id": message_id, "recipient_id": recipient},
            )
            self._settle_locally(message_id, "Not attached to a gateway")
            return
        # Sanitised the way the gateway sanitises it: an id it would refuse
        # is replaced *there* by one it mints, and the verdict then comes
        # back under a name nothing here is waiting on.
        wire_id = policy.sanitize_message_id(message_id)
        if wire_id is None:
            self._emit_diagnostic("error", "Message id the gateway would refuse", {"message_id": message_id})
            self._settle_locally(message_id, "Unserializable frame")
            return
        content = base64.b64encode(bytes(message.data)).decode("ascii")
        line = policy.send_message_json(wire_id, recipient, content, message.reply_to_msg)
        if await self._write_line(line, generation):
            self._consecutive_send_failures = 0
            self._messages_sent += 1
            self._emit_diagnostic(
                "debug",
                "Message submitted to the gateway",
                {"message_id": message_id, "recipient_id": recipient, "content_length": len(content)},
            )
            return
        self._consecutive_send_failures += 1
        self._settle_locally(message_id, "Write failed")
        self._emit_diagnostic(
            "error",
            "Failed to write a message to the gateway",
            {
                "message_id": message_id,
                "recipient_id": recipient,
                "consecutive_failures": self._consecutive_send_failures,
            },
        )
        if self._consecutive_send_failures >= MAX_CONSECUTIVE_FAILURES:
            self._emit_diagnostic(
                "warning",
                "Too many consecutive send failures, triggering reconnect",
                {"failures": self._consecutive_send_failures},
            )
            self._handle_connection_closed(generation, None)

    async def _write_line(self, text: str, generation: int) -> bool:
        """Writes one frame on the socket the session ``generation`` names.
        True when the bytes reached the kernel; a stale generation or a
        failed write is False, and the caller decides what that costs."""
        writer = self._writer
        if writer is None or self._generation != generation:
            return False
        try:
            writer.write(text.encode("utf-8") + b"\n")
            await writer.drain()
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            logger.debug("gateway write failed: %s", exc)
            return False
        return True

    # -- helpers ----------------------------------------------------------------

    def _local_address(self) -> str | None:
        try:
            address = self._protocol.local_address()
        except Exception:
            return None
        return address or None

    def _is_self_peer(self, peer_id: str) -> bool:
        if not peer_id:
            return False
        if peer_id == self._device_id:
            return True
        return peer_id == self._local_address()

    @staticmethod
    def _now_seconds() -> float:
        # Monotonic, like every tracker and watch call in the relay manager:
        # a wall-clock step (an NTP correction after airplane mode) must not
        # fail every frame in flight as unanswered.
        return time.monotonic()

    @staticmethod
    def _now_ms() -> int:
        return int(time.monotonic() * 1000)
