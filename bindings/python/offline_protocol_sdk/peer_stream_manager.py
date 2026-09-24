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
  forever. Between two streams of the winning kind (a peer that reconnects
  while its old stream is still half-open here) the newer one supersedes
  the older, so a stale stream never blocks a reconnect; the cost, that a
  copied preamble can force the real peer to reconnect, is bounded by the
  same rule to exactly that, and the threat model records it as R16.

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
import logging
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

#: Seconds a freshly opened stream has to deliver its preamble.
PREAMBLE_TIMEOUT = 10.0
#: Streams this host holds open at once, announced or not.
MAX_STREAMS = 64
#: Outbound reconnect ladder for a configured peer.
RECONNECT_INITIAL_DELAY = 1.0
RECONNECT_MAX_DELAY = 30.0
#: Ceiling on the cooldown after repeated preamble refusals from one endpoint,
#: doubled per refusal from ``RECONNECT_INITIAL_DELAY``. A peer that cannot
#: prove its address fails the same way every time.
REFUSAL_BACKOFF_MAX = 600.0


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

    __slots__ = ("reader", "writer", "endpoint", "expected", "outbound", "peer", "learned", "write_lock")

    def __init__(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
        endpoint: str,
        expected: str | None,
        outbound: bool,
    ) -> None:
        self.reader = reader
        self.writer = writer
        self.endpoint = endpoint
        self.expected = expected
        #: Whether this host opened the stream. Decides the tie-break.
        self.outbound = outbound
        #: The address this stream is announced under, while it is.
        self.peer: str | None = None
        #: The address the preamble derived, kept even when the stream was
        #: not the one announced, so a connector knows whom it reached.
        self.learned: str | None = None
        self.write_lock = asyncio.Lock()


def frame(body: bytes) -> bytes:
    """Wrap one body as one frame, refusing what a receiver would refuse."""
    if not body:
        raise ValueError("a frame body is at least one byte")
    if len(body) > MAX_FRAME_BYTES:
        raise ValueError(f"frame body of {len(body)} bytes is above the {MAX_FRAME_BYTES}-byte ceiling")
    return len(body).to_bytes(LENGTH_PREFIX_LEN, "big") + body


def service_instance_name(address: str) -> str:
    """The DNS-SD instance name for an address: a short digest, not the
    address itself, which the TXT record already carries."""
    digest = hashlib.sha256(address.encode("utf-8")).hexdigest()[:16]
    return f"op-{digest}.{SERVICE_TYPE}"


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
        Where to accept streams. Port ``0`` binds an ephemeral port, readable
        afterwards as :attr:`listen_port`; ``listen_port=None`` accepts
        nothing and only connects out.
    peers:
        Static peer entries, as :class:`PeerEntry` values or strings
        :meth:`PeerEntry.parse` accepts. Each is connected with a reconnect
        ladder for as long as the manager runs.
    advertise, discover:
        Publish this host over DNS-SD, and connect to hosts found there.
        Either requires the optional ``zeroconf`` dependency.
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

        self._loop: asyncio.AbstractEventLoop | None = None
        self._server: asyncio.base_events.Server | None = None
        #: Every open stream, announced or not.
        self._streams: set[_Stream] = set()
        #: The one announced stream per proved address.
        self._by_peer: dict[str, _Stream] = {}
        self._connector_tasks: dict[str, asyncio.Task[None]] = {}
        #: endpoint -> consecutive preamble refusals, for the backoff.
        self._refusals: dict[str, int] = {}
        self._draining = False
        self._discovery: _LanDiscovery | None = None

        # Metrics
        self._bytes_sent = 0
        self._bytes_received = 0
        self._frames_sent = 0
        self._frames_received = 0
        self._streams_refused = 0

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
            self._ensure_connector(entry)
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
        for task in list(self._connector_tasks.values()):
            task.cancel()
        self._connector_tasks.clear()
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
        for stream in list(self._streams):
            await self._close_stream(stream, "transport stopping")
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
        if len(self._streams) >= self._max_streams:
            self._streams_refused += 1
            self._emit_diagnostic("warning", "Stream refused: at capacity", {"endpoint": endpoint})
            writer.close()
            return
        # An inbound connection carries no claim: the derived address is the
        # peer's id, and there is nothing to compare.
        await self._run_stream(_Stream(reader, writer, endpoint, expected=None, outbound=False))

    def _ensure_connector(self, entry: PeerEntry) -> None:
        key = entry.endpoint
        existing = self._connector_tasks.get(key)
        if existing is not None and not existing.done():
            return
        self._connector_tasks[key] = asyncio.ensure_future(self._connector(entry))

    async def _connector(self, entry: PeerEntry) -> None:
        """Keep one stream open toward a configured peer, with a ladder."""
        delay = RECONNECT_INITIAL_DELAY
        # The address this endpoint turned out to hold, once a preamble said.
        reached: str | None = entry.expected_address
        try:
            while self._state in (TransportState.RUNNING, TransportState.STARTING):
                if reached is not None and reached in self._by_peer:
                    # The peer is announced over another stream (it reached us
                    # first, or its stream won the tie-break). Nothing to open
                    # until that one ends.
                    await asyncio.sleep(RECONNECT_MAX_DELAY)
                    continue
                try:
                    reader, writer = await asyncio.wait_for(
                        asyncio.open_connection(entry.host, entry.port),
                        timeout=PREAMBLE_TIMEOUT,
                    )
                except (OSError, asyncio.TimeoutError) as exc:
                    self._emit_diagnostic(
                        "debug", "Peer connect failed", {"endpoint": entry.endpoint, "error": str(exc)}
                    )
                    await asyncio.sleep(delay)
                    delay = min(delay * 2, RECONNECT_MAX_DELAY)
                    continue
                delay = RECONNECT_INITIAL_DELAY
                stream = _Stream(
                    reader, writer, entry.endpoint, expected=entry.expected_address, outbound=True
                )
                await self._run_stream(stream)
                if stream.learned is not None:
                    reached = stream.learned
                # The stream ended (peer closed, refusal, or our stop); the
                # refusal backoff, if any, decides how soon we try again.
                await asyncio.sleep(self._cooldown(entry.endpoint))
        except asyncio.CancelledError:
            return

    def _cooldown(self, endpoint: str) -> float:
        refusals = self._refusals.get(endpoint, 0)
        if refusals == 0:
            return RECONNECT_INITIAL_DELAY
        return min(RECONNECT_INITIAL_DELAY * (2 ** min(refusals, 16)), REFUSAL_BACKOFF_MAX)

    async def _run_stream(self, stream: _Stream) -> None:
        """Send our preamble, then read frames until the stream ends.

        Owns the stream from open to close: whatever ends it, the peer is
        reported lost if and only if it was announced, and the socket is
        closed exactly once.
        """
        self._streams.add(stream)
        reason = "peer closed the stream"
        try:
            # Ours first, without waiting for theirs: a peer that never speaks
            # cannot hold the stream half-open on purpose.
            assertion = bytes(self._protocol.identity_assertion(signed_data=[]))
            await self._write(stream, frame(assertion))

            while True:
                first = stream.peer is None
                try:
                    if first:
                        prefix = await asyncio.wait_for(
                            stream.reader.readexactly(LENGTH_PREFIX_LEN),
                            timeout=self._preamble_timeout,
                        )
                    else:
                        prefix = await stream.reader.readexactly(LENGTH_PREFIX_LEN)
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
                    self._record_refusal(stream.endpoint)
                    break

                try:
                    body = await stream.reader.readexactly(length)
                except (asyncio.IncompleteReadError, ConnectionError, OSError):
                    break
                self._bytes_received += LENGTH_PREFIX_LEN + length

                if first:
                    if not self._accept_preamble(stream, bytes(body)):
                        reason = "preamble refused"
                        break
                    continue

                self._frames_received += 1
                try:
                    self._protocol.wifi_direct_message_received(
                        sender_id=stream.peer, data=list(body)
                    )
                except Exception as exc:
                    self._emit_diagnostic(
                        "error", "Error handing a frame to the core", {"peer": stream.peer, "error": str(exc)}
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
            self._record_refusal(stream.endpoint)
            self._emit_diagnostic(
                "warning", "Peer preamble did not verify", {"endpoint": stream.endpoint, "error": str(exc)}
            )
            return False
        stream.learned = derived
        if stream.expected is not None and derived != stream.expected:
            # Exact, never normalised, and the claim is never announced.
            self._record_refusal(stream.endpoint)
            self._emit_diagnostic(
                "warning",
                "Peer preamble names another address",
                {"endpoint": stream.endpoint, "expected": stream.expected, "derived": derived},
            )
            return False
        if derived == self.local_address:
            self._emit_diagnostic("debug", "Refusing a stream to ourselves", {"endpoint": stream.endpoint})
            return False
        self._refusals.pop(stream.endpoint, None)

        older = self._by_peer.get(derived)
        if older is not None:
            # One announced stream per address, and both ends must keep the
            # same one: the stream the lower address opened wins, and among
            # two of that kind the newer one. If that is this stream, it
            # takes the announcement over and the older one closes silently;
            # if not, this one closes silently. The core sees neither.
            self._streams_refused += 1
            if not self._new_stream_wins(stream, derived):
                self._emit_diagnostic(
                    "info", "Refusing a second stream for an announced address", {"peer": derived}
                )
                return False
            older.peer = None
            stream.peer = derived
            self._by_peer[derived] = stream
            self._emit_diagnostic("info", "A newer stream supersedes the announced one", {"peer": derived})
            asyncio.ensure_future(self._close_stream(older, "superseded"))
            return True

        stream.peer = derived
        self._by_peer[derived] = stream
        try:
            self._protocol.wifi_direct_peer_connected(peer_id=derived)
        except Exception as exc:
            self._emit_diagnostic("error", "wifi_direct_peer_connected failed", {"peer": derived, "error": str(exc)})
        self._emit_diagnostic("info", "Peer announced", {"peer": derived, "endpoint": stream.endpoint})
        # Something may already be queued for this address.
        self._schedule_drain()
        return True

    def _new_stream_wins(self, new: _Stream, peer: str) -> bool:
        """The tie-break both ends compute alike: the stream opened by the
        lower of the two addresses is the one to keep."""
        local = self.local_address or ""
        we_open = local < peer
        return new.outbound == we_open

    def _record_refusal(self, endpoint: str) -> None:
        self._refusals[endpoint] = self._refusals.get(endpoint, 0) + 1

    async def _close_stream(self, stream: _Stream, reason: str) -> None:
        if stream not in self._streams:
            return
        self._streams.discard(stream)
        announced = stream.peer is not None and self._by_peer.get(stream.peer) is stream
        if announced:
            self._by_peer.pop(stream.peer, None)
        try:
            stream.writer.close()
        except Exception:
            pass
        # A stream that never proved a peer was never announced, and there
        # is nothing to report.
        if announced:
            try:
                self._protocol.wifi_direct_peer_disconnected(peer_id=stream.peer)
            except Exception:
                logger.debug("wifi_direct_peer_disconnected failed for %s", stream.peer)
            self._emit_diagnostic("info", "Peer lost", {"peer": stream.peer, "reason": reason})
        else:
            self._emit_diagnostic("debug", "Unannounced stream closed", {"endpoint": stream.endpoint, "reason": reason})

    async def _write(self, stream: _Stream, data: bytes) -> None:
        async with stream.write_lock:
            stream.writer.write(data)
            await stream.writer.drain()

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
            loop.call_soon_threadsafe(asyncio.ensure_future, self._drain())

    async def _drain(self) -> None:
        if self._draining:
            return
        self._draining = True
        try:
            while True:
                try:
                    message = self._protocol.wifi_direct_get_next_message()
                except Exception as exc:
                    logger.warning("wifi_direct_get_next_message failed: %s", exc)
                    break
                if message is None:
                    break
                stream = self._by_peer.get(message.recipient_id)
                if stream is None:
                    # The stream closed between the queue and the drain; the
                    # core's retry path re-offers the message.
                    self._emit_diagnostic(
                        "debug", "No announced stream for a queued body", {"peer": message.recipient_id}
                    )
                    continue
                body = bytes(message.data)
                try:
                    await self._write(stream, frame(body))
                    self._bytes_sent += LENGTH_PREFIX_LEN + len(body)
                    self._frames_sent += 1
                except Exception as exc:
                    self._emit_diagnostic(
                        "warning", "Failed to write a frame", {"peer": message.recipient_id, "error": str(exc)}
                    )
                    await self._close_stream(stream, f"write failed: {exc}")
        finally:
            self._draining = False

    # -- discovery hooks --------------------------------------------------------

    def _on_peer_discovered(self, entry: PeerEntry) -> None:
        """A DNS-SD record named a host. Its ``addr`` is a hint the preamble
        proves; nothing is announced from the record."""
        if entry.expected_address is not None and entry.expected_address == self.local_address:
            return
        self._ensure_connector(entry)


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
            self._info = ServiceInfo(
                SERVICE_TYPE,
                service_instance_name(address),
                port=port,
                properties=txt_record(address),
            )
            await self._zc.async_register_service(self._info)

        if manager.discover:
            zc = self._zc.zeroconf

            def on_change(zeroconf: Any, service_type: str, name: str, state_change: Any) -> None:
                if state_change is not ServiceStateChange.Added:
                    return
                info = AsyncServiceInfo(service_type, name)

                async def resolve() -> None:
                    if await info.async_request(zeroconf, 3000):
                        for entry in peers_from_record(info):
                            manager._on_peer_discovered(entry)

                asyncio.ensure_future(resolve())

            self._browser = AsyncServiceBrowser(zc, SERVICE_TYPE, handlers=[on_change])

    async def stop(self) -> None:
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
    parsed = getattr(info, "parsed_addresses", None)
    if callable(parsed):
        try:
            hosts = list(parsed())
        except Exception:
            hosts = []
    if not hosts:
        server = getattr(info, "server", None)
        if server:
            hosts = [str(server).rstrip(".")]
    return [PeerEntry(host=h, port=int(port), expected_address=claimed) for h in hosts]

