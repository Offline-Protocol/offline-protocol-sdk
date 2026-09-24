"""Peer-stream transport manager: TCP streams behind the ``wifi_direct`` slot.

A peer stream is a byte stream established to exactly one other device. On a
phone that is a Wi-Fi Direct group socket or a Multipeer session; on a host
it is a TCP connection over a LAN or a routed mesh. To the engine they are one
transport, registered in the slot the FFI names ``wifi_direct`` for
historical reasons, and this manager is the host's implementation of the
contract that makes them one: ``docs/spec/stream-framing.md``.

The two things a stream does not get from its socket for free are where a
message ends and who the peer is, and both are settled here:

* **Every frame is a ``u32`` big-endian length, then that many bytes.** A
  length of zero, or above the 1 MiB message ceiling, closes the stream
  before a byte of the body is read, so a hostile prefix costs four bytes and
  never an allocation. The ceiling is inclusive.
* **The first frame in each direction is the identity assertion.** This
  side sends its own as soon as the stream opens, without waiting for the
  peer's, and verifies the peer's through ``verify_identity_assertion``, the
  one verifier of that layout. A peer is announced to the core only under
  the address that verification derived, never under a socket address, a
  hostname, or an address merely read from a discovery record. A stream
  whose preamble does not verify carries nothing and is closed, and a stream
  that never proved a peer is never reported lost.
* **A stream carries one peer, and this host holds one announced stream per
  address.** When a second stream proves an address already announced, one
  of the two is kept and the other closed without a report, so the core sees
  one announcement and, later, one loss for that address; without that rule,
  a copied preamble on a second socket would let its close read as the real
  peer's loss. Which one is kept is decided the same way on both ends: the
  stream opened by the lower address wins. Two hosts that each list the
  other open toward each other at once, and without a shared rule each would
  keep the stream the other discards, close the other's, and reconnect
  forever. Between two streams of the winning kind (the lower address
  reconnecting while its old stream is still half-open here) the newer one
  supersedes the older; the cost, that a copied preamble can force the real
  peer to reconnect, is bounded by the same rule to exactly that, and the
  threat model records it as R16. The higher address gets no such help: its
  reconnect is the losing kind for as long as the lower side holds a stream
  it believes live. So every stream carries TCP keepalive, which ends an
  idle dead peer's stream within a bound, and every attempt that ends
  unannounced climbs the reconnect ladder rather than retrying at a fixed
  pace.
* **One peer's receive window never holds another peer's traffic.** The
  core hands every outbound body through one queue, and each is moved at
  once onto its stream's own queue, written by that stream's own writer.
  A peer that stops reading, or reads slowly, stalls only itself. The queue
  is bounded only while its writer is blocked on the peer, so a burst to a
  peer that is reading is never dropped, and the stream is aborted and
  reported lost only when the peer has taken nothing for the write
  deadline, so a slow link that is moving is never cut. Keepalive does not cover that case: a stream with data in
  flight is not idle, and outside Linux nothing bounds its retransmissions.

The Rust reference for the same rules is ``stream_framing.rs`` in the
transport crate, and its conformance vectors are the ones this manager's
tests replay. The bounds are asserted as literals in ``test_peer_stream_manager.py``
rather than read from the core, for the reason the bridge rules give: a test
that read them would agree with any edit.

Peers come from two sources. A static peer list (``host:port`` entries, or
``off1…@host:port`` when the address is known, which turns on the chapter's
fourth verification step: the derived address must equal the claim exactly)
serves routed meshes and tests. DNS-SD serves a LAN, through the optional
``zeroconf`` dependency (``pip install 'offline-protocol-sdk[lan]'``): this
host advertises ``_offlineprotocol._tcp`` with ``txtvers=1`` and
``addr=<off1…>`` and browses for the same, and the ``addr`` in a record is a
hint the preamble proves, never a name to announce.

The link is not encrypted by this manager. The chapter is explicit that link
encryption forms no part of the trust argument: every message inside a frame
authenticates itself at the layer that owns it, and a host that wants the
hop's bytes private on a shared LAN puts the stream inside whatever it
trusts for that (a mesh VPN, an SSH tunnel) and configures the peer list
accordingly.
"""

from __future__ import annotations

import asyncio
import hashlib
import ipaddress
import logging
import socket
from dataclasses import dataclass
from typing import Any

from .offline_protocol import verify_identity_assertion
from .transport_manager import TransportError, TransportManager, TransportState

logger = logging.getLogger(__name__)

# -- Framing constants (docs/spec/stream-framing.md) --------------------------
# Mirrors of the Rust constants; pinned as literals by the tests.

#: The prefix on every frame: a ``u32``, big-endian.
LENGTH_PREFIX_LEN = 4
#: Ceiling on a frame body, inclusive: the transport message ceiling.
MAX_FRAME_BYTES = 1024 * 1024
#: Floor on a preamble body: the identity assertion's own floor.
PREAMBLE_FLOOR = 96

# -- DNS-SD (RFC 6763) ---------------------------------------------------------

#: The service type the chapter fixes. Fifteen characters, the label maximum.
SERVICE_TYPE = "_offlineprotocol._tcp.local."
#: The first TXT entry, as RFC 6763 section 6.7 recommends.
TXT_VERSION = "1"

# -- Policy (local, not wire format) ------------------------------------------

#: Seconds a freshly opened stream has to deliver its whole preamble frame,
#: prefix and body together. A deadline on the prefix alone would let four
#: bytes and then silence hold a slot forever.
PREAMBLE_TIMEOUT = 10.0
#: Seconds an outbound connect may take before it counts as a failed attempt.
CONNECT_TIMEOUT = 10.0
#: Streams this host holds open at once, announced or not.
MAX_STREAMS = 64
#: Inbound streams one remote host may hold at once, announced or not. A host
#: needs one, two while it reconnects past its own stale stream; without a
#: per-host bound one machine on the LAN fills ``MAX_STREAMS`` alone, with
#: silent sockets or with as many self-made identities.
MAX_STREAMS_PER_HOST = 8
#: Outbound reconnect ladder for a configured peer. Doubled after every
#: attempt that ends without an announced stream (connect failure, a silent
#: or closing endpoint, a tie-break loss), reset once a stream is announced.
RECONNECT_INITIAL_DELAY = 1.0
RECONNECT_MAX_DELAY = 30.0
#: Ceiling on the cooldown after repeated preamble refusals from one endpoint,
#: doubled per refusal from ``RECONNECT_INITIAL_DELAY``. A peer that cannot
#: prove its address fails the same way every time.
REFUSAL_BACKOFF_MAX = 600.0
#: TCP keepalive on every stream: idle seconds, probe interval, probe count.
#: An idle stream to a host that lost power or left the network is otherwise
#: never noticed, and the tie-break keeps an announced stream over a new one
#: of the losing kind, so an unnoticed dead stream would lock the peer out.
KEEPALIVE_IDLE = 15
KEEPALIVE_INTERVAL = 5
KEEPALIVE_COUNT = 3
#: Seconds a write may go without the peer taking a single byte before the
#: stream is aborted. Keepalive ends an *idle* dead stream; a stream with
#: unacknowledged data is not idle, and on macOS and Windows nothing else
#: bounds it, so without this a peer that stops reading holds its stream,
#: and everything queued to it, for as long as TCP keeps retransmitting.
#: Measured on progress, not per frame: a per-frame deadline is a throughput
#: floor (1 MiB in thirty seconds), and a slow link below it would be
#: aborted, reconnected, offered the same frame and aborted again, forever.
WRITE_TIMEOUT = 30.0
#: Bytes queued toward one stream and not yet written, while its writer is
#: blocked on the peer. A body that would go over is dropped (the
#: reliability layer retries it) rather than buffered without bound behind a
#: peer that is not taking any. One frame is always accepted onto an empty
#: queue, so the largest legal frame is never refused by this bound.
MAX_QUEUED_BYTES = 4 * 1024 * 1024


@dataclass(frozen=True)
class PeerEntry:
    """A configured peer: where to connect, and optionally who to expect.

    ``expected_address`` is a claim about the peer, and turns on the
    chapter's fourth verification step: the address the preamble derives
    must equal it exactly, or the stream is closed. It is never what gets
    announced; the derived address is.
    """

    host: str
    port: int
    expected_address: str | None = None

    @classmethod
    def parse(cls, entry: str) -> "PeerEntry":
        """Parse ``host:port`` or ``off1…@host:port``.

        An IPv6 literal is written in brackets, ``[::1]:7878``.
        """
        expected: str | None = None
        rest = entry.strip()
        if "@" in rest:
            expected, rest = rest.rsplit("@", 1)
            expected = expected.strip()
            if not expected:
                raise ValueError(f"empty address before '@' in peer entry {entry!r}")
        if rest.startswith("["):
            close = rest.find("]")
            if close < 0 or not rest[close + 1 :].startswith(":"):
                raise ValueError(f"malformed IPv6 peer entry {entry!r}")
            host, port_text = rest[1:close], rest[close + 2 :]
        else:
            if ":" not in rest:
                raise ValueError(f"peer entry {entry!r} has no port")
            host, port_text = rest.rsplit(":", 1)
        if not host:
            raise ValueError(f"peer entry {entry!r} has no host")
        try:
            port = int(port_text)
        except ValueError as exc:
            raise ValueError(f"peer entry {entry!r} has a non-numeric port") from exc
        if not 1 <= port <= 65535:
            raise ValueError(f"peer entry {entry!r} port out of range")
        return cls(host=host, port=port, expected_address=expected)

    @property
    def endpoint(self) -> str:
        return f"{self.host}:{self.port}"


class _Stream:
    """One stream, both directions, and what it has proved."""

    __slots__ = (
        "reader",
        "writer",
        "endpoint",
        "remote_host",
        "expected",
        "outbound",
        "peer",
        "learned",
        "was_announced",
        "superseded",
        "outbound_queue",
        "queued_bytes",
        "writer_task",
        "write_blocked",
    )

    def __init__(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
        endpoint: str,
        expected: str | None,
        outbound: bool,
        remote_host: str | None = None,
    ) -> None:
        self.reader = reader
        self.writer = writer
        self.endpoint = endpoint
        #: The remote IP, for the per-host bound on inbound streams.
        self.remote_host = remote_host
        self.expected = expected
        #: Whether this host opened the stream. Decides the tie-break.
        self.outbound = outbound
        #: The address this stream is announced under, while it is.
        self.peer: str | None = None
        #: The address the preamble derived, kept even when the stream was
        #: not the one announced, so a connector knows whom it reached.
        self.learned: str | None = None
        #: Whether this stream was ever the announced one, so a connector
        #: can tell a lost link (reconnect now) from an attempt that went
        #: nowhere (climb the ladder).
        self.was_announced = False
        #: Set when a newer stream took the announcement over. The stream
        #: may still have a body in its buffer, and none may reach the core:
        #: its frames belong to no announced peer any more.
        self.superseded = False
        #: Framed bodies waiting for this stream's writer, and their total
        #: size. Per stream, so a peer that stops reading holds only its own.
        self.outbound_queue: asyncio.Queue[bytes] = asyncio.Queue()
        self.queued_bytes = 0
        #: The task writing ``outbound_queue``, once the preamble is out.
        self.writer_task: asyncio.Task[None] | None = None
        #: Whether the writer is waiting on the peer's receive window right
        #: now. The queue bound applies only then: a burst the core hands
        #: over in one tick arrives before the writer has run at all, says
        #: nothing about the peer, and its bytes were already held by the
        #: core's queue.
        self.write_blocked = False


def frame(body: bytes) -> bytes:
    """Wrap one body as one frame, refusing what a receiver would refuse."""
    if not body:
        raise ValueError("a frame body is at least one byte")
    if len(body) > MAX_FRAME_BYTES:
        raise ValueError(
            f"frame body of {len(body)} bytes is above the {MAX_FRAME_BYTES}-byte ceiling"
        )
    return len(body).to_bytes(LENGTH_PREFIX_LEN, "big") + body


def _digest(address: str) -> str:
    return hashlib.sha256(address.encode("utf-8")).hexdigest()[:16]


def service_instance_name(address: str) -> str:
    """The DNS-SD instance name for an address: a short digest, not the
    address itself, which the TXT record already carries."""
    return f"op-{_digest(address)}.{SERVICE_TYPE}"


def service_host_name(address: str) -> str:
    """The host name the SRV record targets and the A/AAAA records name.

    Its own name rather than the machine's: the operating system's responder
    may already publish ``<hostname>.local.``, and two responders answering
    for one name is a conflict. Without a host name and addresses at all the
    record is unresolvable: a browser sees the service appear and can never
    learn where it is.
    """
    return f"op-{_digest(address)}.local."


def _set_keepalive(writer: asyncio.StreamWriter) -> None:
    """Turn TCP keepalive on for a stream, with the tightest timers the
    platform exposes. Best effort: a socket that refuses an option keeps the
    rest."""
    sock = writer.get_extra_info("socket")
    if sock is None:
        return
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE, 1)
    except OSError:
        return
    # TCP_KEEPIDLE on Linux and Windows, TCP_KEEPALIVE (the idle time) on macOS.
    idle_option = getattr(socket, "TCP_KEEPIDLE", None) or getattr(socket, "TCP_KEEPALIVE", None)
    options = [
        (idle_option, KEEPALIVE_IDLE),
        (getattr(socket, "TCP_KEEPINTVL", None), KEEPALIVE_INTERVAL),
        (getattr(socket, "TCP_KEEPCNT", None), KEEPALIVE_COUNT),
        # Linux: bound how long unacknowledged data may sit, which keepalive
        # does not cover once a write is in flight.
        (
            getattr(socket, "TCP_USER_TIMEOUT", None),
            (KEEPALIVE_IDLE + KEEPALIVE_INTERVAL * KEEPALIVE_COUNT) * 1000,
        ),
    ]
    for option, value in options:
        if option is None:
            continue
        try:
            sock.setsockopt(socket.IPPROTO_TCP, option, value)
        except OSError:
            pass


def _discard_socket(writer: asyncio.StreamWriter) -> None:
    """Close a stream's socket now, whatever is still buffered.

    ``writer.close()`` waits for the write buffer to flush before the socket
    goes, and toward a peer that stopped reading it never flushes: the
    socket, and a writer awaiting ``drain()`` on it, would outlive the close.
    Abort discards the buffer, which is correct for a stream being
    discarded; a stream with nothing buffered closes gracefully.
    """
    try:
        transport = writer.transport
        if transport.get_write_buffer_size() > 0:
            transport.abort()
            return
    except Exception:
        pass
    try:
        writer.close()
    except Exception:
        pass


class PeerStreamManager(TransportManager):
    """The host's peer-stream transport: a TCP listener, outbound connections
    to configured and discovered peers, the preamble exchange, and the drain
    of the core's outbound queue onto the stream that proved each recipient.

    Parameters
    ----------
    protocol:
        The ``OfflineProtocol`` UniFFI instance. Its MLS must be initialised
        before :meth:`start`, because a stream layer that cannot prove its
        own address is refused by every conforming peer.
    listen_host, listen_port:
        Where to accept streams. The default ``0.0.0.0`` accepts on every
        interface; pass ``127.0.0.1`` or one interface's address to narrow
        it. Port ``0`` binds an ephemeral port, readable afterwards as
        :attr:`listen_port`; ``listen_port=None`` accepts nothing and only
        connects out.
    peers:
        Static peer entries, as :class:`PeerEntry` values or strings
        :meth:`PeerEntry.parse` accepts. Each is connected with a reconnect
        ladder for as long as the manager runs.
    advertise, discover:
        Publish this host over DNS-SD, and connect to hosts found there.
        Either requires the optional ``zeroconf`` dependency.
    write_timeout:
        Seconds a write may go without the peer taking a byte before its
        stream is aborted and reported lost.
    """

    transport_id = "wifi_direct"
    transport_name = "Peer stream (TCP)"

    def __init__(
        self,
        protocol: Any,
        *,
        listen_host: str = "0.0.0.0",
        listen_port: int | None = 0,
        peers: Any = (),
        advertise: bool = False,
        discover: bool = False,
        preamble_timeout: float = PREAMBLE_TIMEOUT,
        max_streams: int = MAX_STREAMS,
        max_streams_per_host: int = MAX_STREAMS_PER_HOST,
        write_timeout: float = WRITE_TIMEOUT,
    ) -> None:
        super().__init__()
        self._protocol = protocol
        self.listen_host = listen_host
        self._listen_port_requested = listen_port
        #: The bound port once started, ``None`` before or when not listening.
        self.listen_port: int | None = None
        self._peers: list[PeerEntry] = [
            p if isinstance(p, PeerEntry) else PeerEntry.parse(p) for p in peers
        ]
        self.advertise = advertise
        self.discover = discover
        self._preamble_timeout = preamble_timeout
        self._max_streams = max_streams
        self._max_streams_per_host = max_streams_per_host
        self._write_timeout = write_timeout

        self._loop: asyncio.AbstractEventLoop | None = None
        self._server: asyncio.base_events.Server | None = None
        #: Every open stream, announced or not.
        self._streams: set[_Stream] = set()
        #: The one announced stream per proved address.
        self._by_peer: dict[str, _Stream] = {}
        #: connector key -> the task keeping a stream open toward it.
        self._connector_tasks: dict[str, asyncio.Task[None]] = {}
        #: connector key -> the entries it tries, in order, on each attempt.
        #: A discovered host's addresses can change while its connector runs.
        self._connector_entries: dict[str, list[PeerEntry]] = {}
        #: Connector keys whose source is gone (a DNS-SD record removed). The
        #: connector finishes its current stream and does not open another.
        self._retired: set[str] = set()
        #: Set on every reported loss, so a connector idling on an announced
        #: peer wakes when that peer is lost rather than on a poll.
        self._loss = asyncio.Event()
        #: endpoint -> consecutive preamble refusals, for the backoff. Only
        #: outbound endpoints: an inbound one is a fresh ephemeral port every
        #: time, and remembering it would only grow the map.
        self._refusals: dict[str, int] = {}
        self._discovery: _LanDiscovery | None = None

        # Metrics
        self._bytes_sent = 0
        self._bytes_received = 0
        self._frames_sent = 0
        self._frames_received = 0
        self._streams_refused = 0
        self._streams_superseded = 0
        self._frames_dropped = 0

    # -- TransportManager interface -------------------------------------------

    def is_available(self) -> bool:
        return True

    def configure(
        self,
        *,
        listen_host: str | None = None,
        listen_port: int | None | object = ...,
        peers: Any | None = None,
        advertise: bool | None = None,
        discover: bool | None = None,
    ) -> None:
        """Change the settings before :meth:`start`."""
        if self._state != TransportState.STOPPED:
            raise TransportError("configure the peer-stream transport before starting it")
        if listen_host is not None:
            self.listen_host = listen_host
        if listen_port is not ...:
            self._listen_port_requested = listen_port  # type: ignore[assignment]
        if peers is not None:
            self._peers = [p if isinstance(p, PeerEntry) else PeerEntry.parse(p) for p in peers]
        if advertise is not None:
            self.advertise = advertise
        if discover is not None:
            self.discover = discover

    @property
    def peers(self) -> list[PeerEntry]:
        return list(self._peers)

    @property
    def local_address(self) -> str | None:
        return self._protocol.local_address()

    def connected_peers(self) -> list[str]:
        """The addresses with an announced stream right now."""
        return list(self._by_peer)

    async def start(self) -> None:
        if self._state == TransportState.RUNNING:
            raise TransportError("peer-stream transport is already running")
        self._loop = asyncio.get_running_loop()
        self._update_state(TransportState.STARTING)

        # Refuse to open a single stream this host cannot prove itself on:
        # every conforming peer would close it at the first frame, and a
        # listener that answers each connect with an unverifiable preamble
        # only costs the LAN a connect-refuse cycle per peer.
        try:
            self._protocol.identity_assertion(signed_data=[])
        except Exception as exc:
            self._update_state(TransportState.STOPPED)
            raise TransportError(
                "peer-stream transport needs an identity to prove: initialise MLS "
                f"before starting it ({exc})"
            ) from exc

        if self._listen_port_requested is not None:
            try:
                self._server = await asyncio.start_server(
                    self._on_inbound, self.listen_host, self._listen_port_requested
                )
            except OSError as exc:
                self._update_state(TransportState.STOPPED)
                raise TransportError(f"peer-stream listen failed: {exc}") from exc
            sockets = self._server.sockets or ()
            self.listen_port = sockets[0].getsockname()[1] if sockets else None

        if self.advertise or self.discover:
            try:
                self._discovery = _LanDiscovery(self)
                await self._discovery.start()
            except Exception as exc:
                await self._close_everything()
                self._update_state(TransportState.STOPPED)
                raise TransportError(f"peer-stream DNS-SD failed to start: {exc}") from exc

        try:
            self._protocol.wifi_direct_status_changed(is_connected=True)
        except Exception:
            logger.debug("wifi_direct_status_changed(True) failed", exc_info=True)

        self._update_state(TransportState.RUNNING)
        for entry in self._peers:
            self._ensure_connector(entry.endpoint, [entry])
        self._emit_diagnostic(
            "info",
            "Peer-stream transport started",
            {"listen_port": self.listen_port, "static_peers": len(self._peers)},
        )

    async def stop(self) -> None:
        if self._state not in (TransportState.RUNNING, TransportState.STARTING):
            return
        self._update_state(TransportState.STOPPING)
        await self._close_everything()
        try:
            self._protocol.wifi_direct_status_changed(is_connected=False)
        except Exception:
            logger.debug("wifi_direct_status_changed(False) failed", exc_info=True)
        self._update_state(TransportState.STOPPED)
        self._emit_diagnostic("info", "Peer-stream transport stopped")

    async def _close_everything(self) -> None:
        tasks = list(self._connector_tasks.values())
        for task in tasks:
            task.cancel()
        # Awaited, so no connector outlives the manager: one that never
        # yields again would otherwise be destroyed pending with the loop.
        await asyncio.gather(*tasks, return_exceptions=True)
        self._connector_tasks.clear()
        self._connector_entries.clear()
        self._retired.clear()
        if self._discovery is not None:
            try:
                await self._discovery.stop()
            except Exception:
                logger.debug("DNS-SD stop failed", exc_info=True)
            self._discovery = None
        if self._server is not None:
            self._server.close()
        # Streams before the server's wait: since Python 3.12 `wait_closed`
        # waits for every connection handler to finish, and a handler
        # finishes only once its stream is closed.
        writers = [s.writer_task for s in self._streams if s.writer_task is not None]
        for stream in list(self._streams):
            await self._close_stream(stream, "transport stopping")
        await asyncio.gather(*writers, return_exceptions=True)
        self._streams.clear()
        self._by_peer.clear()
        self._refusals.clear()
        if self._server is not None:
            try:
                await asyncio.wait_for(self._server.wait_closed(), 2.0)
            except Exception:
                pass
            self._server = None
            self.listen_port = None

    def get_metrics(self) -> dict[str, Any]:
        return {
            "bytes_sent": self._bytes_sent,
            "bytes_received": self._bytes_received,
            "frames_sent": self._frames_sent,
            "frames_received": self._frames_received,
            "streams_refused": self._streams_refused,
            "streams_superseded": self._streams_superseded,
            "frames_dropped": self._frames_dropped,
            "connected_peers": len(self._by_peer),
            "open_streams": len(self._streams),
            "listen_port": self.listen_port,
        }

    # -- inbound and outbound streams -----------------------------------------

    async def _on_inbound(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        peername = writer.get_extra_info("peername")
        endpoint = f"{peername[0]}:{peername[1]}" if peername else "?"
        remote_host = peername[0] if peername else None
        refusal = None
        if self._state not in (TransportState.RUNNING, TransportState.STARTING):
            # Accepted before the listener closed and scheduled after the
            # stop swept the streams: serving it would announce a peer to a
            # core already told the layer is down.
            refusal = "Stream refused: transport stopping"
        elif len(self._streams) >= self._max_streams:
            refusal = "Stream refused: at capacity"
        elif (
            remote_host is not None
            and self._inbound_from(remote_host) >= self._max_streams_per_host
        ):
            refusal = "Stream refused: per-host limit"
        if refusal is not None:
            self._streams_refused += 1
            self._emit_diagnostic("warning", refusal, {"endpoint": endpoint})
            writer.close()
            return
        _set_keepalive(writer)
        # An inbound connection carries no claim: the derived address is the
        # peer's id, and there is nothing to compare.
        await self._run_stream(
            _Stream(reader, writer, endpoint, expected=None, outbound=False, remote_host=remote_host)
        )

    def _inbound_from(self, remote_host: str) -> int:
        return sum(1 for s in self._streams if not s.outbound and s.remote_host == remote_host)

    def _ensure_connector(self, key: str, entries: list[PeerEntry]) -> None:
        """Keep a connector running toward ``key``, trying ``entries`` in
        order. A running connector picks up new entries on its next attempt."""
        self._connector_entries[key] = list(entries)
        self._retired.discard(key)
        existing = self._connector_tasks.get(key)
        if existing is not None and not existing.done():
            return
        self._connector_tasks[key] = asyncio.ensure_future(self._connector(key))

    def _retire_connector(self, key: str) -> None:
        """The source of ``key`` is gone. Its connector keeps a live stream
        until that stream ends, then stops instead of reconnecting."""
        if key in self._connector_tasks:
            self._retired.add(key)

    async def _wait_while_announced(self, address: str) -> None:
        """Idle while ``address`` is announced over another stream, waking on
        each reported loss (and at the ladder's ceiling, as a safety net)."""
        while address in self._by_peer and self._state == TransportState.RUNNING:
            self._loss.clear()
            try:
                await asyncio.wait_for(self._loss.wait(), timeout=RECONNECT_MAX_DELAY)
            except asyncio.TimeoutError:
                pass

    async def _connector(self, key: str) -> None:
        """Keep one stream open toward a configured or discovered peer."""
        delay = RECONNECT_INITIAL_DELAY
        # The address this peer turned out to hold, once a preamble said.
        entries = self._connector_entries.get(key) or []
        reached: str | None = entries[0].expected_address if entries else None
        try:
            while self._state in (TransportState.RUNNING, TransportState.STARTING):
                if key in self._retired:
                    return
                if reached is not None and reached in self._by_peer:
                    # The peer is announced over another stream (it reached us
                    # first, or its stream won the tie-break). Nothing to open
                    # until that one ends.
                    await self._wait_while_announced(reached)
                    continue
                stream = await self._open(key)
                if stream is None:
                    await asyncio.sleep(delay)
                    delay = min(delay * 2, RECONNECT_MAX_DELAY)
                    continue
                await self._run_stream(stream)
                if stream.learned is not None:
                    reached = stream.learned
                if stream.was_announced:
                    # A link that worked and was lost: try again promptly.
                    delay = RECONNECT_INITIAL_DELAY
                    wait = self._cooldown(stream.endpoint)
                else:
                    # An attempt that went nowhere (silent or closing endpoint,
                    # refused preamble, tie-break loss): climb the ladder, and
                    # longer still if the endpoint keeps failing verification.
                    wait = max(delay, self._cooldown(stream.endpoint))
                    delay = min(delay * 2, RECONNECT_MAX_DELAY)
                await asyncio.sleep(wait)
        except asyncio.CancelledError:
            return
        finally:
            if self._connector_tasks.get(key) is asyncio.current_task():
                self._connector_tasks.pop(key, None)
                self._connector_entries.pop(key, None)
                self._retired.discard(key)

    async def _open(self, key: str) -> _Stream | None:
        """One connect attempt toward ``key``: each of its entries in turn,
        the first that connects wins. ``None`` when none did."""
        for entry in list(self._connector_entries.get(key) or []):
            try:
                reader, writer = await asyncio.wait_for(
                    asyncio.open_connection(entry.host, entry.port),
                    timeout=CONNECT_TIMEOUT,
                )
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                # Not only OSError: a malformed host raises ValueError or
                # UnicodeError, and a connector that died on one would never
                # retry the peer, silently.
                self._emit_diagnostic(
                    "debug", "Peer connect failed", {"endpoint": entry.endpoint, "error": str(exc)}
                )
                continue
            _set_keepalive(writer)
            return _Stream(
                reader, writer, entry.endpoint, expected=entry.expected_address, outbound=True
            )
        return None

    def _cooldown(self, endpoint: str) -> float:
        refusals = self._refusals.get(endpoint, 0)
        if refusals == 0:
            return RECONNECT_INITIAL_DELAY
        return min(RECONNECT_INITIAL_DELAY * (2 ** min(refusals, 16)), REFUSAL_BACKOFF_MAX)

    async def _read_frame_prefix(self, stream: _Stream, deadline: float | None) -> bytes:
        read = stream.reader.readexactly(LENGTH_PREFIX_LEN)
        if deadline is None:
            return await read
        return await asyncio.wait_for(read, timeout=self._remaining(deadline))

    async def _read_frame_body(self, stream: _Stream, length: int, deadline: float | None) -> bytes:
        read = stream.reader.readexactly(length)
        if deadline is None:
            return await read
        return await asyncio.wait_for(read, timeout=self._remaining(deadline))

    def _remaining(self, deadline: float) -> float:
        assert self._loop is not None
        return max(0.0, deadline - self._loop.time())

    async def _run_stream(self, stream: _Stream) -> None:
        """Send our preamble, then read frames until the stream ends.

        Owns the stream from open to close: whatever ends it, the peer is
        reported lost if and only if it was announced, and the socket is
        closed exactly once.
        """
        self._streams.add(stream)
        reason = "peer closed the stream"
        loop = asyncio.get_running_loop()
        try:
            # Ours first, without waiting for theirs: a peer that never speaks
            # cannot hold the stream half-open on purpose.
            assertion = bytes(self._protocol.identity_assertion(signed_data=[]))
            # One deadline for the whole preamble exchange, our write and
            # their frame, prefix and body: a deadline on the prefix alone
            # lets four bytes hold the slot.
            deadline: float | None = loop.time() + self._preamble_timeout
            stream.writer.write(frame(assertion))
            try:
                await asyncio.wait_for(stream.writer.drain(), timeout=self._remaining(deadline))
            except asyncio.TimeoutError:
                reason = "preamble write stalled"
                return
            # Every later frame goes through this stream's own writer, so
            # the preamble is first on the wire by construction.
            stream.writer_task = asyncio.ensure_future(self._write_loop(stream))
            first = True
            while True:
                if stream.superseded:
                    reason = "superseded"
                    break
                try:
                    prefix = await self._read_frame_prefix(stream, deadline)
                except asyncio.TimeoutError:
                    reason = "no preamble within the deadline"
                    break
                except (asyncio.IncompleteReadError, ConnectionError, OSError):
                    break
                length = int.from_bytes(prefix, "big")

                # Bounds on the prefix, before a byte of the body.
                if length == 0 or length > MAX_FRAME_BYTES:
                    reason = f"refused frame length {length}"
                    break
                if first and length < PREAMBLE_FLOOR:
                    reason = f"refused preamble length {length}"
                    self._record_refusal(stream)
                    break

                try:
                    body = await self._read_frame_body(stream, length, deadline)
                except asyncio.TimeoutError:
                    reason = "no preamble within the deadline"
                    break
                except (asyncio.IncompleteReadError, ConnectionError, OSError):
                    break
                self._bytes_received += LENGTH_PREFIX_LEN + length

                if first:
                    if not self._accept_preamble(stream, bytes(body)):
                        reason = "preamble refused"
                        break
                    first = False
                    deadline = None
                    continue

                # Checked after the read as well as before it: a newer stream
                # may have taken the announcement over while this one waited.
                if stream.superseded or stream.peer is None:
                    reason = "superseded"
                    break
                self._frames_received += 1
                try:
                    self._protocol.wifi_direct_message_received(
                        sender_id=stream.peer, data=list(body)
                    )
                except Exception as exc:
                    self._emit_diagnostic(
                        "error",
                        "Error handing a frame to the core",
                        {"peer": stream.peer, "error": str(exc)},
                    )
        except asyncio.CancelledError:
            reason = "transport stopping"
            raise
        except Exception as exc:
            reason = f"stream error: {exc}"
        finally:
            await self._close_stream(stream, reason)

    def _accept_preamble(self, stream: _Stream, body: bytes) -> bool:
        """Steps one to four of the chapter's verification, then the
        one-announced-stream-per-address rule, then the announcement."""
        try:
            derived = verify_identity_assertion(list(body))
        except Exception as exc:
            self._record_refusal(stream)
            self._emit_diagnostic(
                "warning",
                "Peer preamble did not verify",
                {"endpoint": stream.endpoint, "error": str(exc)},
            )
            return False
        stream.learned = derived
        if stream.expected is not None and derived != stream.expected:
            # Exact, never normalised, and the claim is never announced.
            self._record_refusal(stream)
            self._emit_diagnostic(
                "warning",
                "Peer preamble names another address",
                {"endpoint": stream.endpoint, "expected": stream.expected, "derived": derived},
            )
            return False
        if derived == self.local_address:
            # Counted, so a peer entry that points at this host climbs to the
            # refusal ceiling instead of reconnecting at the ladder's pace
            # forever.
            self._record_refusal(stream)
            self._emit_diagnostic(
                "debug", "Refusing a stream to ourselves", {"endpoint": stream.endpoint}
            )
            return False
        self._refusals.pop(stream.endpoint, None)

        older = self._by_peer.get(derived)
        if older is not None:
            # One announced stream per address, and both ends must keep the
            # same one: the stream the lower address opened wins, and among
            # two of that kind the newer one. If that is this stream, it
            # takes the announcement over and the older one closes silently;
            # if not, this one closes silently. The core sees neither.
            if not self._new_stream_wins(stream, derived):
                self._streams_refused += 1
                self._emit_diagnostic(
                    "info", "Refusing a second stream for an announced address", {"peer": derived}
                )
                return False
            self._streams_superseded += 1
            older.peer = None
            older.superseded = True
            stream.peer = derived
            stream.was_announced = True
            self._by_peer[derived] = stream
            self._emit_diagnostic(
                "info", "A newer stream supersedes the announced one", {"peer": derived}
            )
            asyncio.ensure_future(self._close_stream(older, "superseded"))
            return True

        stream.peer = derived
        stream.was_announced = True
        self._by_peer[derived] = stream
        try:
            self._protocol.wifi_direct_peer_connected(peer_id=derived)
        except Exception as exc:
            self._emit_diagnostic(
                "error", "wifi_direct_peer_connected failed", {"peer": derived, "error": str(exc)}
            )
        self._emit_diagnostic("info", "Peer announced", {"peer": derived, "endpoint": stream.endpoint})
        # Something may already be queued for this address.
        self._schedule_drain()
        return True

    def _new_stream_wins(self, new: _Stream, peer: str) -> bool:
        """The tie-break both ends compute alike: the stream opened by the
        lower of the two addresses is the one to keep.

        This helps only the lower address past its own stale stream. The
        higher address's reconnect is the losing kind while the lower side
        still holds a stream it believes live, which is why every stream
        carries TCP keepalive: a dead peer's stream ends within a bound, and
        the reconnect wins from then on.
        """
        local = self.local_address
        if local is None:
            # No address of our own to order by; keep what is announced.
            return False
        we_open = local < peer
        return new.outbound == we_open

    def _record_refusal(self, stream: _Stream) -> None:
        # Only an outbound endpoint is ever connected to again; an inbound
        # one is a new ephemeral port each time.
        if stream.outbound:
            self._refusals[stream.endpoint] = self._refusals.get(stream.endpoint, 0) + 1

    async def _close_stream(self, stream: _Stream, reason: str) -> None:
        if stream not in self._streams:
            return
        self._streams.discard(stream)
        announced = stream.peer is not None and self._by_peer.get(stream.peer) is stream
        if announced:
            self._by_peer.pop(stream.peer, None)
        task = stream.writer_task
        if task is not None and task is not asyncio.current_task():
            task.cancel()
        _discard_socket(stream.writer)
        # A stream that never proved a peer was never announced, and there
        # is nothing to report.
        if announced:
            try:
                self._protocol.wifi_direct_peer_disconnected(peer_id=stream.peer)
            except Exception:
                logger.debug("wifi_direct_peer_disconnected failed for %s", stream.peer)
            self._emit_diagnostic("info", "Peer lost", {"peer": stream.peer, "reason": reason})
            self._loss.set()
        else:
            self._emit_diagnostic(
                "debug", "Unannounced stream closed", {"endpoint": stream.endpoint, "reason": reason}
            )

    async def _write_loop(self, stream: _Stream) -> None:
        """Write ``stream``'s queued frames, one at a time. A peer that takes
        nothing for the write deadline aborts the stream: the peer is
        reported lost, and nothing else waited on it."""
        reason = "write failed"
        try:
            while True:
                data = await stream.outbound_queue.get()
                stream.queued_bytes -= len(data)
                stream.writer.write(data)
                stream.write_blocked = True
                try:
                    await self._drain_while_moving(stream)
                finally:
                    stream.write_blocked = False
                self._bytes_sent += len(data)
                self._frames_sent += 1
        except asyncio.CancelledError:
            return
        except asyncio.TimeoutError:
            reason = "write stalled past the deadline"
        except Exception as exc:
            reason = f"write failed: {exc}"
        self._emit_diagnostic(
            "warning", "Peer stream write failed", {"peer": stream.peer, "reason": reason}
        )
        await self._close_stream(stream, reason)

    async def _drain_while_moving(self, stream: _Stream) -> None:
        """Wait for ``stream``'s write buffer to flush, for as long as the
        peer keeps taking bytes. Raises :class:`asyncio.TimeoutError` once a
        whole ``write_timeout`` passes with the buffer no smaller: the
        deadline is on progress, so a slow link is never mistaken for a
        dead one."""
        transport = stream.writer.transport
        last = transport.get_write_buffer_size()
        while True:
            try:
                await asyncio.wait_for(stream.writer.drain(), timeout=self._write_timeout)
                return
            except asyncio.TimeoutError:
                size = transport.get_write_buffer_size()
                if size >= last:
                    raise
                last = size

    # -- outbound drain ---------------------------------------------------------

    def on_messages_available(self) -> None:
        """``WifiDirectTransportCallback.on_messages_available``: the core
        queued a body toward an announced peer.

        Invoked from a Rust callback thread, so the drain is scheduled onto
        the event loop rather than run here.
        """
        self._schedule_drain()

    def _schedule_drain(self) -> None:
        loop = self._loop
        if loop is not None and not loop.is_closed() and loop.is_running():
            loop.call_soon_threadsafe(self._drain)

    def _drain(self) -> None:
        """Move every body the core holds onto its stream's own queue.

        Never awaits: the core's queue is one FIFO for every peer, and a
        drain that waited on one peer's socket would hold every other
        peer's traffic behind it. Each stream's writer does the waiting.
        """
        while True:
            try:
                message = self._protocol.wifi_direct_get_next_message()
            except Exception as exc:
                logger.warning("wifi_direct_get_next_message failed: %s", exc)
                return
            if message is None:
                return
            stream = self._by_peer.get(message.recipient_id)
            if stream is None or stream.writer_task is None:
                # The stream closed between the queue and the drain; the
                # core's retry path re-offers the message.
                self._emit_diagnostic(
                    "debug", "No announced stream for a queued body", {"peer": message.recipient_id}
                )
                continue
            try:
                data = frame(bytes(message.data))
            except ValueError as exc:
                # A body no receiver would accept. Refused here, and the
                # stream, which did nothing wrong, stays up.
                self._frames_dropped += 1
                self._emit_diagnostic(
                    "error", "Refusing to frame a body", {"peer": message.recipient_id, "error": str(exc)}
                )
                continue
            if (
                stream.write_blocked
                and stream.queued_bytes
                and stream.queued_bytes + len(data) > MAX_QUEUED_BYTES
            ):
                # The peer is not taking what it already has. Buffering
                # without bound would only move the stall into memory; the
                # write deadline ends a peer that has stopped, and the retry
                # path re-offers this. Only while blocked: a burst handed
                # over before the writer ran says nothing about the peer.
                self._frames_dropped += 1
                self._emit_diagnostic(
                    "warning", "Peer stream queue full, dropping a body", {"peer": message.recipient_id}
                )
                continue
            stream.queued_bytes += len(data)
            stream.outbound_queue.put_nowait(data)

    # -- discovery hooks --------------------------------------------------------

    def _on_record_resolved(self, name: str, entries: list[PeerEntry]) -> None:
        """A DNS-SD record named a host, under ``name``. Its ``addr`` is a
        hint the preamble proves; nothing is announced from the record. One
        connector per record, trying the record's addresses in turn."""
        if not entries:
            return
        claimed = entries[0].expected_address
        if claimed is not None and claimed == self.local_address:
            return
        self._ensure_connector(f"dns-sd:{name}", entries)

    def _on_record_removed(self, name: str) -> None:
        """A DNS-SD record went away: stop reconnecting toward it once its
        current stream, if any, ends. A live stream is not cut, because a
        record expiring says nothing about the stream."""
        self._retire_connector(f"dns-sd:{name}")


def advertised_addresses(listen_host: str) -> list[str]:
    """The addresses a DNS-SD record should carry for a listener on
    ``listen_host``: that address when it names one, otherwise every
    non-loopback address of every interface (``ifaddr`` comes with the
    ``lan`` extra)."""
    try:
        bound = ipaddress.ip_address(listen_host)
    except ValueError:
        bound = None
    if bound is not None and not bound.is_unspecified:
        return [str(bound)]

    import ifaddr

    want_v4 = bound is None or bound.version == 4 or listen_host == "::"
    want_v6 = bound is None or bound.version == 6
    found: list[str] = []
    for adapter in ifaddr.get_adapters():
        for ip in adapter.ips:
            if isinstance(ip.ip, tuple):
                if not want_v6:
                    continue
                address = ipaddress.ip_address(ip.ip[0])
            else:
                if not want_v4:
                    continue
                address = ipaddress.ip_address(ip.ip)
            if address.is_loopback:
                continue
            text = str(address)
            if text not in found:
                found.append(text)
    return found


class _LanDiscovery:
    """DNS-SD advertiser and browser over the optional ``zeroconf`` package.

    Imported lazily so the base install carries no LGPL dependency; a manager
    that asks for discovery without it fails at :meth:`start` with the
    install hint.
    """

    def __init__(self, manager: PeerStreamManager) -> None:
        self._manager = manager
        self._zc: Any = None
        self._info: Any = None
        self._browser: Any = None
        self._pending: set[asyncio.Task[None]] = set()

    async def start(self) -> None:
        try:
            from zeroconf import IPVersion, ServiceInfo, ServiceStateChange
            from zeroconf.asyncio import AsyncServiceBrowser, AsyncServiceInfo, AsyncZeroconf
        except ImportError as exc:
            raise TransportError(
                "LAN discovery needs the optional dependency: pip install 'offline-protocol-sdk[lan]'"
            ) from exc

        self._zc = AsyncZeroconf(ip_version=IPVersion.All)
        manager = self._manager

        if manager.advertise:
            address = manager.local_address
            port = manager.listen_port
            if address is None or port is None:
                raise TransportError("advertising needs an address and a listening port")
            addresses = advertised_addresses(manager.listen_host)
            if not addresses:
                raise TransportError("advertising found no interface address to publish")
            self._info = ServiceInfo(
                SERVICE_TYPE,
                service_instance_name(address),
                port=port,
                properties=txt_record(address),
                server=service_host_name(address),
                parsed_addresses=addresses,
            )
            await self._zc.async_register_service(self._info)

        if manager.discover:
            zc = self._zc.zeroconf

            def on_change(zeroconf: Any, service_type: str, name: str, state_change: Any) -> None:
                if state_change is ServiceStateChange.Removed:
                    manager._on_record_removed(name)
                    return
                info = AsyncServiceInfo(service_type, name)

                async def resolve() -> None:
                    if await info.async_request(zeroconf, 3000):
                        manager._on_record_resolved(name, peers_from_record(info))

                task = asyncio.ensure_future(resolve())
                self._pending.add(task)
                task.add_done_callback(self._pending.discard)

            self._browser = AsyncServiceBrowser(zc, SERVICE_TYPE, handlers=[on_change])

    async def stop(self) -> None:
        for task in list(self._pending):
            task.cancel()
        if self._browser is not None:
            await self._browser.async_cancel()
            self._browser = None
        if self._zc is not None:
            if self._info is not None:
                await self._zc.async_unregister_service(self._info)
                self._info = None
            await self._zc.async_close()
            self._zc = None


def txt_record(address: str) -> dict[str, str]:
    """The TXT record the chapter fixes: ``txtvers`` first, then ``addr``."""
    return {"txtvers": TXT_VERSION, "addr": address}


def peers_from_record(info: Any) -> list[PeerEntry]:
    """Turn a resolved DNS-SD record into peer entries, one per address the
    host published, each carrying the record's ``addr`` as the claim to
    verify against. A record without a well-formed ``addr`` yields nothing:
    the chapter makes the entry mandatory, and a host that omits it is not
    advertising this service.

    Link-local IPv6 addresses come with their scope (``fe80::1%en0``); an
    unscoped one cannot be connected to.
    """
    properties = getattr(info, "properties", None) or {}
    raw = properties.get(b"addr")
    if raw is None:
        return []
    try:
        claimed = raw.decode("utf-8") if isinstance(raw, (bytes, bytearray)) else str(raw)
    except UnicodeDecodeError:
        return []
    if not claimed.startswith("off1"):
        return []
    port = getattr(info, "port", None)
    if not port:
        return []
    hosts: list[str] = []
    for accessor in ("parsed_scoped_addresses", "parsed_addresses"):
        parsed = getattr(info, accessor, None)
        if not callable(parsed):
            continue
        try:
            hosts = [str(h) for h in parsed()]
        except Exception:
            hosts = []
        if hosts:
            break
    if not hosts:
        server = getattr(info, "server", None)
        if server:
            hosts = [str(server).rstrip(".")]
    return [PeerEntry(host=h, port=int(port), expected_address=claimed) for h in hosts]
