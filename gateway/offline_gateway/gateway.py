"""The device-facing gateway daemon: the five verbs plus Deliver, over the
wire in ``wire.py`` (``docs/spec/gateway-contract.md``).

Invariants this module keeps. Each is a rule of the contract, and each names
what breaking it costs:

* **A session is bound only by a proof this gateway checked.** The signature,
  the derivation (both inside the SDK's ``verify_gateway_address_proof``) and
  a challenge minted for this connection and spent once. Skip one and a
  device draws another device's traffic to itself.
* **Every ``SendMessage`` gets exactly one verdict.** Silence turns a routing
  decision into a client timeout, and a second verdict settles an id twice.
* **``StatusUpdate(connected)`` follows ``Capabilities`` and is never sent
  while a declaration is unanswered.** A client may treat a session
  announced before it is bound as a failed attach.
* **An unbound session is never a recipient.** Nothing is delivered over it,
  and it may only submit and be told ``attach_required``.
* **A verdict is a claim about routing, never about delivery.** Only the
  recipient's end-to-end acknowledgement settles a message, so this daemon
  holds nothing and promises nothing it cannot keep: no ``pushed``, no
  ``stored``.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
import os
import time
import uuid
from collections import OrderedDict
from typing import Any, Callable

from . import wire
from .backbone import Backbone, BackboneFrame, ForwardResult
from .budget import BucketMap
from .config import GatewayConfig

logger = logging.getLogger(__name__)

#: ``verify(address, public_key, signature, challenge)`` returns the bound
#: address or raises. The default is the SDK's.
Verifier = Callable[[str, bytes, bytes, bytes], str]

#: Declarations a session may get wrong before it is dropped. A real client
#: makes one; a session that keeps guessing is probing.
MAX_DECLARE_FAILURES = 3
#: Peers one session may be watching at once.
MAX_WATCHED = 256
#: Addresses whose last-seen time is remembered after they leave.
MAX_LAST_SEEN = 65536
#: Frame types logged at debug level as they are queued: the routing trail.
_TRACED = frozenset({"MessageSent", "DeliveryError", "MessageReceived"})


def sdk_verifier() -> Verifier:
    """The SDK's verifier, imported on first use so the daemon's own tests
    need no native library."""
    try:
        from offline_protocol_sdk.offline_protocol import verify_gateway_address_proof
    except ImportError as exc:  # pragma: no cover - depends on the install
        raise RuntimeError(
            "the offline-protocol-sdk package with verify_gateway_address_proof is required; "
            "install the SDK build from this repository (bindings/python)"
        ) from exc

    def verify(address: str, public_key: bytes, signature: bytes, challenge: bytes) -> str:
        return verify_gateway_address_proof(address, list(public_key), list(signature), list(challenge))

    return verify


class Session:
    """One device connection."""

    _ids = 0

    def __init__(self, gateway: "Gateway", writer: asyncio.StreamWriter, max_queued: int) -> None:
        Session._ids += 1
        self.id = Session._ids
        self.gateway = gateway
        self.writer = writer
        self.peer_name = writer.get_extra_info("peername")
        self.host = self.peer_name[0] if isinstance(self.peer_name, tuple) and self.peer_name else None
        #: Minted at connect and spent by the one declaration it binds.
        self.challenge = os.urandom(wire.CHALLENGE_LENGTH)
        self.challenge_sent = False
        self.challenge_spent = False
        self.declare_failures = 0
        #: The proved address, once bound.
        self.address: str | None = None
        #: ``StatusUpdate(connected)`` has been sent.
        self.announced = False
        self.watching: OrderedDict[str, None] = OrderedDict()
        self.closed = False
        self._queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue(max_queued)
        self._tasks: set[asyncio.Task[Any]] = set()

    def __repr__(self) -> str:
        return f"<session {self.id} {self.address or 'unbound'} {self.peer_name}>"

    def send(self, frame: dict[str, Any]) -> bool:
        """Queues a frame. ``False`` when the session is gone or so far behind
        that it is dropped: a device that cannot keep up must not hold memory
        for every frame addressed to it."""
        if self.closed:
            return False
        if logger.isEnabledFor(logging.DEBUG) and frame.get("type") in _TRACED:
            logger.debug(
                "%r <- %s %s %s",
                self,
                frame["type"],
                frame.get("message_id"),
                frame.get("reason") or frame.get("sender") or "",
            )
        try:
            self._queue.put_nowait(frame)
        except asyncio.QueueFull:
            logger.warning("%r is not reading; dropping it", self)
            self.close()
            return False
        return True

    def spawn(self, coro: Any) -> None:
        task = asyncio.ensure_future(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    def close(self) -> None:
        """Stops the session. Frames already queued are still written (a
        refusal's reason is worth delivering), then the socket closes. A
        session too far behind to take the sentinel is aborted instead."""
        if self.closed:
            return
        self.closed = True
        for task in list(self._tasks):
            task.cancel()
        try:
            self._queue.put_nowait(None)
        except asyncio.QueueFull:
            self.writer.transport.abort()

    async def write_loop(self) -> None:
        try:
            while True:
                frame = await self._queue.get()
                if frame is None:
                    return
                line = wire.encode(frame)
                if len(line) > wire.MAX_LINE_BYTES:
                    # The clients abandon the connection past this, so a
                    # longer line would cost the session, not just the frame.
                    logger.error("refusing to write a %d-byte line to %r", len(line), self)
                    continue
                self.writer.write(line)
                await self.writer.drain()
        except (ConnectionError, OSError):
            pass
        finally:
            self.closed = True
            # Closing the transport is also what ends the read loop, which is
            # blocked reading and never sees the flag.
            self.writer.close()


class Gateway:
    def __init__(
        self,
        config: GatewayConfig,
        verifier: Verifier | None = None,
        backbone: Backbone | None = None,
        wall_clock: Callable[[], float] = time.time,
    ) -> None:
        config.validate()
        self.config = config
        self._verify = verifier or sdk_verifier()
        self.backbone = backbone
        self._wall_clock = wall_clock
        self.sessions: set[Session] = set()
        #: Address to the one session bound to it.
        self.bound: dict[str, Session] = {}
        self._last_seen: OrderedDict[str, int] = OrderedDict()
        #: Presence numbering for the backbone: every bind and unbind is one
        #: change, numbered in order, so a peer can tell a late message from
        #: a current one (``PresenceTable``). The epoch tells a peer this
        #: process's numbering from a previous run's.
        self.presence_epoch = os.urandom(8).hex()
        self._presence_seq = 0
        self._bound_seq: dict[str, int] = {}
        self._device_budget = BucketMap(config.device_per_minute / 60.0, config.device_burst)
        self._backbone_budget = BucketMap(config.host_backbone_bytes_per_minute / 60.0, config.host_backbone_burst_bytes)
        self._peer_budget = BucketMap(config.peer_per_minute / 60.0, config.peer_burst)
        self._egress_budget = BucketMap(config.peer_egress_bytes_per_minute / 60.0, config.peer_egress_burst_bytes)
        self._server: asyncio.base_events.Server | None = None
        self._connections: set[asyncio.Task[Any]] = set()
        #: Backbone forwards in flight. Held by the gateway, not the session:
        #: a device that disconnects mid-forward does not cancel it. The frame
        #: is partly on air already, finishing it is what reaches the
        #: recipient, and the device's retry of it is a duplicate the core
        #: removes. Cancelling would throw that airtime away and spend it
        #: again on the retry.
        self._forwards: set[asyncio.Task[Any]] = set()

    # -- Lifecycle -------------------------------------------------------

    @property
    def capability_tokens(self) -> list[str]:
        tokens = ["gateway_v1"]
        if self.backbone is not None:
            tokens.append(f"backbone_{self.backbone.kind}_v1")
        return tokens

    async def start(self) -> int:
        """Starts listening and the backbone. Returns the bound port."""
        if self.backbone is not None:
            await self.backbone.start(self)
        self._server = await asyncio.start_server(
            self._accept,
            self.config.listen_host,
            self.config.listen_port,
            # One byte over the line bound, so a line of exactly the bound
            # with its newline is read and anything longer overruns.
            limit=wire.MAX_LINE_BYTES + 1,
        )
        port = self._server.sockets[0].getsockname()[1]
        logger.info(
            "gateway listening on %s:%d, capabilities %s",
            self.config.listen_host,
            port,
            ",".join(self.capability_tokens),
        )
        return port

    async def stop(self) -> None:
        if self._server is not None:
            self._server.close()
        for session in list(self.sessions):
            session.close()
        for task in list(self._forwards):
            task.cancel()
        for task in list(self._connections):
            task.cancel()
        if self._connections:
            await asyncio.gather(*self._connections, return_exceptions=True)
        if self._server is not None:
            with contextlib.suppress(Exception):
                await self._server.wait_closed()
        if self.backbone is not None:
            await self.backbone.stop()

    # -- A connection ----------------------------------------------------

    async def _accept(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        task = asyncio.current_task()
        if task is not None:
            self._connections.add(task)
        try:
            if not self._admit(writer):
                writer.close()
                return
            session = Session(self, writer, self.config.max_queued_frames)
            self.sessions.add(session)
            writer_task = asyncio.ensure_future(session.write_loop())
            if self.config.attach_grace:
                session.spawn(self._grace(session))
            try:
                await self._read_loop(session, reader)
            finally:
                session.close()
                self.sessions.discard(session)
                self._unbind(session)
                await asyncio.gather(writer_task, return_exceptions=True)
                logger.debug("%r closed", session)
        finally:
            if task is not None:
                self._connections.discard(task)

    def _admit(self, writer: asyncio.StreamWriter) -> bool:
        """Whether a new connection may open, making room for it if it can.

        A session that has proved nothing must not be able to hold a slot a
        device needs: anyone on the venue network can open sockets, and the
        verdict-only grace keeps an idle one alive indefinitely. So unbound
        sessions have their own, smaller pool, and when it or the whole table
        is full a newcomer evicts the oldest unbound session rather than
        being refused. A device that attaches leaves that pool. Each host is
        also capped, since every device on a hotspot has an address of its
        own and one host holding many sessions is not many devices.
        """
        peer = writer.get_extra_info("peername")
        host = peer[0] if isinstance(peer, tuple) and peer else None
        if host is not None:
            from_host = sum(1 for s in self.sessions if s.host == host and not s.closed)
            if from_host >= self.config.max_sessions_per_host:
                logger.warning("refusing a connection from %s: %d sessions open from it", host, from_host)
                return False
        unbound = sorted((s for s in self.sessions if s.address is None and not s.closed), key=lambda s: s.id)
        open_sessions = sum(1 for s in self.sessions if not s.closed)
        while unbound and (
            len(unbound) >= self.config.max_unbound_sessions or open_sessions >= self.config.max_sessions
        ):
            oldest = unbound.pop(0)
            logger.info("evicting %r to make room", oldest)
            oldest.close()
            open_sessions -= 1
        if open_sessions >= self.config.max_sessions:
            logger.warning("refusing a connection: %d bound sessions open", open_sessions)
            return False
        return True

    async def _read_loop(self, session: Session, reader: asyncio.StreamReader) -> None:
        while not session.closed:
            try:
                line = await reader.readuntil(b"\n")
            except asyncio.LimitOverrunError:
                logger.warning("%r sent a line over %d bytes; closing", session, wire.MAX_LINE_BYTES)
                return
            except (asyncio.IncompleteReadError, ConnectionError, OSError):
                return
            frame = wire.decode(line)
            if frame is None:
                continue
            handler = self._handlers.get(frame["type"])
            if handler is not None:
                handler(self, session, frame)

    async def _grace(self, session: Session) -> None:
        """Announces a session that never declared, verdict-only, so a client
        that does not attach can still submit and be told why it is refused."""
        await asyncio.sleep(self.config.attach_grace)
        if not session.announced and session.address is None and not session.closed:
            session.send(wire.capabilities(self.capability_tokens))
            session.send(wire.status_update("connected"))
            session.announced = True

    # -- Attach ----------------------------------------------------------

    def _on_identify(self, session: Session, frame: dict[str, Any]) -> None:
        if session.challenge_spent:
            return
        session.challenge_sent = True
        session.send(wire.challenge(session.challenge))

    def _on_declare(self, session: Session, frame: dict[str, Any]) -> None:
        # Every path below answers synchronously, with no await between the
        # declaration and its answer, so the grace timer cannot announce the
        # session while the declaration is unanswered.
        if session.address is not None:
            session.send(wire.address_error("this session is already bound"))
            return
        if not session.challenge_sent or session.challenge_spent:
            session.send(wire.address_error("no challenge outstanding: send Identify first"))
            return
        address = wire.address_field(frame.get("address"))
        public_key = wire.b64decode(frame.get("public_key"))
        signature = wire.b64decode(frame.get("signature"))
        if address is None or public_key is None or signature is None:
            self._refuse_declaration(session, "malformed declaration")
            return
        try:
            bound = self._verify(address, public_key, signature, session.challenge)
        except Exception as exc:  # the SDK raises its own error type
            logger.info("%r: declaration for %s refused: %s", session, address, exc)
            self._refuse_declaration(session, "address proof does not verify")
            return
        if bound != address:  # the verifier returns the claimed address or raises
            self._refuse_declaration(session, "address proof does not verify")
            return
        session.challenge_spent = True
        self._bind(session, address)
        session.send(wire.address_declared(address))
        session.send(wire.capabilities(self.capability_tokens))
        session.send(wire.status_update("connected"))
        session.announced = True
        logger.info("%r bound", session)

    def _refuse_declaration(self, session: Session, reason: str) -> None:
        session.declare_failures += 1
        session.send(wire.address_error(reason))
        if session.declare_failures >= MAX_DECLARE_FAILURES:
            logger.warning("%r: %d refused declarations; closing", session, session.declare_failures)
            session.close()

    def _bind(self, session: Session, address: str) -> None:
        previous = self.bound.get(address)
        self.bound[address] = session
        session.address = address
        if previous is not None and previous is not session:
            # The same key proved itself on a new connection: the old one is a
            # socket the device has already given up on. Closing it here, after
            # the rebind, means its own teardown finds the address taken and
            # leaves it alone.
            logger.info("%s reattached; closing %r", address, previous)
            previous.close()
            return
        self._presence_seq += 1
        self._bound_seq[address] = self._presence_seq
        self._presence_changed(address)
        if self.backbone is not None:
            self.backbone.local_presence_changed(address, True, None, self._presence_seq)

    def _unbind(self, session: Session) -> None:
        address = session.address
        if address is None or self.bound.get(address) is not session:
            return
        del self.bound[address]
        now = self._now_ms()
        self._last_seen.pop(address, None)
        self._last_seen[address] = now
        while len(self._last_seen) > MAX_LAST_SEEN:
            self._last_seen.popitem(last=False)
        self._presence_seq += 1
        self._bound_seq.pop(address, None)
        self._presence_changed(address)
        if self.backbone is not None:
            self.backbone.local_presence_changed(address, False, now, self._presence_seq)

    # -- Submit and Verdict ----------------------------------------------

    def _on_send(self, session: Session, frame: dict[str, Any]) -> None:
        message_id = wire.message_id_field(frame.get("message_id")) or uuid.uuid4().hex
        recipient = wire.address_field(frame.get("recipient"))
        if recipient is None:
            session.send(wire.delivery_error(message_id, None, f"{wire.MALFORMED_REQUEST}: no recipient"))
            return
        if session.address is None:
            session.send(wire.delivery_error(message_id, recipient, wire.ATTACH_REQUIRED))
            return
        content = wire.content_field(frame)
        if content is None:
            session.send(wire.delivery_error(message_id, recipient, f"{wire.MALFORMED_REQUEST}: no content"))
            return
        if len(content) > self.config.max_frame_bytes:
            session.send(wire.delivery_error(message_id, recipient, wire.FRAME_TOO_LARGE))
            return
        if not self._device_budget.take(session.address):
            session.send(wire.delivery_error(message_id, recipient, wire.BUDGET_EXCEEDED))
            return
        reply_to = wire.message_id_field(frame.get("reply_to_msg"))

        target = self.bound.get(recipient)
        if target is not None and target.address is not None:
            delivered = target.send(wire.message_received(session.address, content, message_id, reply_to))
            if delivered:
                session.send(wire.message_sent(message_id, recipient))
                return

        claim = self.backbone.claim(recipient) if self.backbone is not None else None
        if self.backbone is None or claim is None or not claim.online:
            where = "this gateway" if self.backbone is None else "this gateway or its peers"
            session.send(
                wire.delivery_error(
                    message_id, recipient, f"{wire.RECIPIENT_UNREACHABLE}: not attached to {where}"
                )
            )
            return
        # The scarce class's bound, which is also how media stays off it. A
        # broad backbone carries whatever the device link accepted.
        if self.backbone.rate_class != "broad" and len(content) > self.config.max_backbone_frame_bytes:
            session.send(wire.delivery_error(message_id, recipient, wire.FRAME_TOO_LARGE))
            return
        # The scarce class's own budgets, in bytes. A broad backbone carries
        # what the device budget above already admitted. Two of them, because
        # the scarce thing is the airtime toward one peer, which every device
        # here shares: the per-host budget, which validation keeps below the
        # per-peer one, stops one device taking all of it, and the per-peer
        # budget stops all of them together asking for more than the link
        # carries. Both are checked before either is charged, so a refusal
        # costs the device nothing.
        if self.backbone.rate_class != "broad":
            sender = session.host or session.address
            peer, size = claim.peer, len(content)
            if not (self._backbone_budget.has(sender, size) and self._egress_budget.has(peer, size)):
                session.send(wire.delivery_error(message_id, recipient, wire.BUDGET_EXCEEDED))
                return
            self._backbone_budget.take(sender, size)
            self._egress_budget.take(peer, size)
        backbone_frame = BackboneFrame(recipient, session.address, content, message_id, reply_to)
        task = asyncio.ensure_future(self._forward(session, backbone_frame))
        self._forwards.add(task)
        task.add_done_callback(self._forwards.discard)

    async def _forward(self, session: Session, frame: BackboneFrame) -> None:
        assert self.backbone is not None
        message_id = frame.message_id or uuid.uuid4().hex
        try:
            result, reason = await self.backbone.forward(frame, self.config.backbone_verdict_timeout)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("backbone forward failed")
            result, reason = ForwardResult.TIMEOUT, "backbone error"
        if result is ForwardResult.DELIVERED:
            session.send(wire.message_sent(message_id, frame.recipient))
        elif result is ForwardResult.UNREACHABLE:
            session.send(
                wire.delivery_error(message_id, frame.recipient, f"{wire.RECIPIENT_UNREACHABLE}: {reason}")
            )
        elif result is ForwardResult.REFUSED:
            # The peer's own token (budget_exceeded, frame_too_large): a plain
            # failure, never the unreachable verdict.
            token = reason if reason in (wire.BUDGET_EXCEEDED, wire.FRAME_TOO_LARGE) else "peer_refused"
            session.send(wire.delivery_error(message_id, frame.recipient, token))
        else:
            session.send(wire.delivery_error(message_id, frame.recipient, f"{wire.BACKBONE_TIMEOUT}: {reason}"))

    # -- Presence --------------------------------------------------------

    def _on_check_presence(self, session: Session, frame: dict[str, Any]) -> None:
        # Presence answers say who is in the zone. A session that has not
        # proved an address is told nothing.
        if session.address is None:
            return
        peers = frame.get("peers")
        if not isinstance(peers, list):
            return
        asked: list[str] = []
        for peer in peers[: wire.MAX_PRESENCE_PEERS]:
            address = wire.address_field(peer)
            if address is not None and address not in asked:
                asked.append(address)
        for address in asked:
            session.watching.pop(address, None)
            session.watching[address] = None
            while len(session.watching) > MAX_WATCHED:
                session.watching.popitem(last=False)
            session.send(self._presence_frame(address))

    def _presence_frame(self, address: str) -> dict[str, Any]:
        if address in self.bound:
            return wire.presence_status(address, True, self._now_ms())
        claim = self.backbone.claim(address) if self.backbone is not None else None
        if claim is not None and claim.online:
            return wire.presence_status(address, True, self._now_ms())
        seen = self._last_seen.get(address)
        if claim is not None and claim.last_seen_ms is not None:
            seen = max(seen or 0, claim.last_seen_ms)
        return wire.presence_status(address, False, seen)

    def _presence_changed(self, address: str) -> None:
        """Pushes the current answer to every session watching ``address``."""
        frame: dict[str, Any] | None = None
        for session in list(self.sessions):
            if address in session.watching and session.address is not None:
                frame = frame or self._presence_frame(address)
                session.send(frame)

    def _now_ms(self) -> int:
        return int(self._wall_clock() * 1000)

    _handlers: dict[str, Callable[["Gateway", Session, dict[str, Any]], None]] = {
        "Identify": _on_identify,
        "DeclareAddress": _on_declare,
        "SendMessage": _on_send,
        "CheckPresence": _on_check_presence,
    }

    # -- Backbone hooks --------------------------------------------------

    async def deliver_from_backbone(self, frame: BackboneFrame, peer: str) -> tuple[ForwardResult, str]:
        if not self._peer_budget.take(peer):
            return ForwardResult.REFUSED, wire.BUDGET_EXCEEDED
        bound = self.config.max_frame_bytes
        if self.backbone is not None and self.backbone.rate_class != "broad":
            bound = self.config.max_backbone_frame_bytes
        if len(frame.content) > bound:
            return ForwardResult.REFUSED, wire.FRAME_TOO_LARGE
        target = self.bound.get(frame.recipient)
        if target is None:
            return ForwardResult.UNREACHABLE, "not attached to the peer gateway"
        # Re-originated as an ordinary frame from the device that sent it,
        # attributed by the gateway that forwarded it. The sender is a claim:
        # what authenticates the frame is the frame (MLS), on this carrier as
        # on every other.
        if target.send(wire.message_received(frame.sender, frame.content, frame.message_id, frame.reply_to_msg)):
            return ForwardResult.DELIVERED, ""
        return ForwardResult.UNREACHABLE, "the recipient's session closed"

    def local_snapshot(self) -> tuple[int, dict[str, int]]:
        return self._presence_seq, dict(self._bound_seq)

    def remote_presence_changed(self, address: str) -> None:
        self._presence_changed(address)
