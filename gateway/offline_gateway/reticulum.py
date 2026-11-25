"""The Reticulum backbone: how gateways reach each other over LoRa (through an
RNode), packet radio, TCP, I2P or anything else Reticulum carries.

This is the subsection "The Reticulum backbone" of the gateway contract, made
concrete:

* **Destinations.** Each gateway announces exactly one destination, under its
  own Reticulum identity (``offline_gateway.backbone.v1``). No device has a
  Reticulum identity and no gateway holds a device key, so a device is
  reachable across the backbone only through the gateway it is attached to.
* **Peers** are provisioned destination hashes. A link from anyone else is
  torn down once it identifies, and every request handler refuses a caller
  that is not a provisioned peer.
* **Framing.** Frames routinely exceed the 465-byte Reticulum MDU, so they
  travel as link requests, which Reticulum carries as resources (sequenced,
  checked, any size) whenever they do not fit one packet.
* **Presence** is the contract's Presence verb between gateways: a snapshot
  of attached addresses when a link comes up, then pushed changes.
* **Rate class** ``scarce``: the daemon applies the per-device backbone
  budget and the backbone frame bound at its own egress.

Two links per pair, one dialled by each side, because a Reticulum request
travels only from a link's initiator to the destination it dialled: each
gateway makes its requests on the link it opened, and serves the peer's on
the link the peer opened.

Gateway-to-gateway requests (msgpack, by Reticulum):

``/forward``  ``{"r": recipient, "s": sender, "c": bytes, "m": id, "p": reply_to}``
              -> ``{"o": "delivered" | "unreachable" | "refused", "w": reason}``
``/presence`` ``{"snap": bool, "on": [address], "off": [[address, last_seen_ms]]}``
              -> ``True``
``/snapshot`` ``None`` -> ``[address]``, every address attached at the callee

Reticulum calls back on its own threads. Everything that touches the daemon
or this module's tables hops onto the event loop first.
"""

from __future__ import annotations

import asyncio
import logging
import os
import time
from typing import Any

from .backbone import Backbone, BackboneFrame, BackboneHooks, Claim, ForwardResult, PresenceTable
from .config import ReticulumConfig
from . import wire

logger = logging.getLogger(__name__)
rns_logger = logging.getLogger("RNS")

APP_NAME = "offline_gateway"
ASPECTS = ("backbone", "v1")

PATH_FORWARD = "/forward"
PATH_PRESENCE = "/presence"
PATH_SNAPSHOT = "/snapshot"

#: Seconds to wait for a path to a peer after asking for one.
PATH_WAIT = 20.0
#: Seconds a link may take to come up. Long: a LoRa link request and its
#: proof are two packets at a few hundred bits per second, plus any hops.
LINK_ESTABLISH_WAIT = 90.0
#: Seconds presence changes are gathered before one request carries them, so
#: a burst of attaches costs one packet per peer rather than one each.
PRESENCE_COALESCE = 2.0
#: Seconds a request handler waits for the event loop to answer it.
HANDLER_WAIT = 10.0
#: A forward's refusal reason when the caller has not identified as a
#: provisioned peer on this link.
NOT_A_PEER = "not a provisioned peer"


class ReticulumBackbone(Backbone):
    kind = "reticulum"
    rate_class = "scarce"

    def __init__(self, config: ReticulumConfig, max_frame_bytes: int = 16 * 1024) -> None:
        self.config = config
        self.max_frame_bytes = max_frame_bytes
        self.peers: set[bytes] = {bytes.fromhex(p) for p in config.peers}
        self.table = PresenceTable()
        self.hooks: BackboneHooks | None = None
        self.destination_hash: bytes | None = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._identity: Any = None
        self._destination: Any = None
        #: The link this gateway dialled to each peer, which carries its requests.
        self._out: dict[bytes, Any] = {}
        #: Links peers dialled to this gateway, by the peer they identified as.
        self._in: dict[bytes, Any] = {}
        self._tasks: list[asyncio.Task[Any]] = []
        self._pending_on: set[str] = set()
        self._pending_off: dict[str, int | None] = {}
        self._flush: asyncio.TimerHandle | None = None
        self._running = False

    # -- Lifecycle -------------------------------------------------------

    async def start(self, hooks: BackboneHooks) -> None:
        import RNS

        self.hooks = hooks
        self._loop = asyncio.get_running_loop()
        self._running = True
        RNS.Reticulum(
            configdir=self.config.configdir,
            loglevel=self.config.loglevel,
            logdest=lambda line: rns_logger.info("%s", line),
        )
        self._identity = _load_identity(RNS, self.config.identity_path)
        self._destination = RNS.Destination(
            self._identity, RNS.Destination.IN, RNS.Destination.SINGLE, APP_NAME, *ASPECTS
        )
        self.destination_hash = self._destination.hash
        # A forward carries one frame and its routing, msgpack-encoded; past
        # this it is not a frame the daemon would have accepted.
        self._destination.set_max_request_size(self.max_frame_bytes + 1024)
        self._destination.set_link_established_callback(self._inbound_link)
        for path, handler in (
            (PATH_FORWARD, self._handle_forward),
            (PATH_PRESENCE, self._handle_presence),
            (PATH_SNAPSHOT, self._handle_snapshot),
        ):
            self._destination.register_request_handler(path, handler, allow=RNS.Destination.ALLOW_ALL)
        logger.info(
            "reticulum backbone up: this gateway's destination is %s; %d peer(s) provisioned",
            self._destination.hash.hex(),
            len(self.peers),
        )
        if getattr(RNS.Transport, "STAMP_DESTINATIONS", False) and self._destination.stamp is None:
            # Reticulum mints it on its own worker thread and stores it, so
            # this costs one wait per identity, not one per start.
            logger.info(
                "minting this gateway's announce stamp, a one-time proof of work: "
                "peers cannot find this gateway until it is done (seconds to minutes, by CPU)"
            )
        self._tasks.append(asyncio.ensure_future(self._announce_loop()))
        for peer in self.peers:
            self._tasks.append(asyncio.ensure_future(self._dial_loop(peer)))

    async def stop(self) -> None:
        self._running = False
        for task in self._tasks:
            task.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        self._tasks.clear()
        if self._flush is not None:
            self._flush.cancel()
        for link in list(self._out.values()) + list(self._in.values()):
            try:
                link.teardown()
            except Exception:  # pragma: no cover - best effort at shutdown
                pass
        self._out.clear()
        self._in.clear()
        self.hooks = None

    # -- Routing ---------------------------------------------------------

    def claim(self, address: str) -> Claim | None:
        return self.table.claim(address)

    async def forward(self, frame: BackboneFrame, timeout: float) -> tuple[ForwardResult, str]:
        claim = self.table.claim(frame.recipient)
        if claim is None or not claim.online:
            return ForwardResult.UNREACHABLE, "no peer gateway claims the recipient"
        link = self._active_out(bytes.fromhex(claim.peer))
        if link is None:
            return ForwardResult.TIMEOUT, "no link to the peer gateway"
        request = {
            "r": frame.recipient,
            "s": frame.sender,
            "c": frame.content,
            "m": frame.message_id,
            "p": frame.reply_to_msg,
        }
        try:
            response = await self._request(link, PATH_FORWARD, request, timeout)
        except asyncio.TimeoutError:
            return ForwardResult.TIMEOUT, "no answer from the peer gateway"
        if not isinstance(response, dict):
            return ForwardResult.TIMEOUT, "unreadable answer from the peer gateway"
        outcome = response.get("o")
        reason = response.get("w") if isinstance(response.get("w"), str) else ""
        if outcome == ForwardResult.REFUSED.value and reason == NOT_A_PEER:
            # The peer does not know who this link is: the identify was lost.
            link.teardown()
            return ForwardResult.TIMEOUT, "the peer gateway did not recognise this link; redialling"
        for result in ForwardResult:
            if result.value == outcome and result is not ForwardResult.TIMEOUT:
                return result, reason
        return ForwardResult.TIMEOUT, "unreadable answer from the peer gateway"

    def local_presence_changed(self, address: str, online: bool, last_seen_ms: int | None) -> None:
        if online:
            self._pending_off.pop(address, None)
            self._pending_on.add(address)
        else:
            self._pending_on.discard(address)
            self._pending_off[address] = last_seen_ms
        if self._flush is None and self._loop is not None:
            self._flush = self._loop.call_later(PRESENCE_COALESCE, self._flush_presence)

    def _flush_presence(self) -> None:
        self._flush = None
        if not (self._pending_on or self._pending_off):
            return
        update = {
            "snap": False,
            "on": sorted(self._pending_on),
            "off": [[a, t] for a, t in sorted(self._pending_off.items())],
        }
        self._pending_on.clear()
        self._pending_off.clear()
        for peer in list(self.peers):
            link = self._active_out(peer)
            if link is not None:
                self._spawn(self._send_presence(link, peer, update))

    async def _send_presence(self, link: Any, peer: bytes, update: dict[str, Any]) -> None:
        try:
            await self._request(link, PATH_PRESENCE, update, LINK_ESTABLISH_WAIT)
        except asyncio.TimeoutError:
            # The next snapshot, on the next link, carries what this missed.
            logger.warning("presence update to %s went unanswered", peer.hex())

    # -- Links -----------------------------------------------------------

    async def _announce_loop(self) -> None:
        while self._running:
            self._destination.announce()
            await asyncio.sleep(self.config.announce_interval)

    async def _dial_loop(self, peer: bytes) -> None:
        """Keeps one outbound link to ``peer`` open, for this gateway's own
        requests to it."""
        import RNS

        while self._running:
            if self._active_out(peer) is not None:
                await asyncio.sleep(self.config.reconnect_interval)
                continue
            if not RNS.Transport.has_path(peer):
                RNS.Transport.request_path(peer)
                deadline = time.monotonic() + PATH_WAIT
                while not RNS.Transport.has_path(peer) and time.monotonic() < deadline:
                    await asyncio.sleep(0.5)
            identity = RNS.Identity.recall(peer)
            if identity is None:
                logger.info("no path to peer gateway %s yet", peer.hex())
                await asyncio.sleep(self.config.reconnect_interval)
                continue
            destination = RNS.Destination(identity, RNS.Destination.OUT, RNS.Destination.SINGLE, APP_NAME, *ASPECTS)
            established = asyncio.Event()
            loop = self._loop
            assert loop is not None

            def on_established(link: Any, established: asyncio.Event = established) -> None:
                loop.call_soon_threadsafe(established.set)

            def on_closed(link: Any, peer: bytes = peer) -> None:
                loop.call_soon_threadsafe(self._out_link_closed, peer, link)

            link = RNS.Link(destination, established_callback=on_established, closed_callback=on_closed)
            try:
                await asyncio.wait_for(established.wait(), LINK_ESTABLISH_WAIT)
            except asyncio.TimeoutError:
                logger.info("link to peer gateway %s did not come up", peer.hex())
                link.teardown()
                await asyncio.sleep(self.config.reconnect_interval)
                continue
            link.identify(self._identity)
            self._out[peer] = link
            logger.info("backbone link to %s established", peer.hex())
            await self._exchange_snapshots(link, peer)

    async def _exchange_snapshots(self, link: Any, peer: bytes) -> None:
        assert self.hooks is not None
        mine = {"snap": True, "on": self.hooks.local_addresses(), "off": []}
        try:
            await self._request(link, PATH_PRESENCE, mine, LINK_ESTABLISH_WAIT)
            theirs = await self._request(link, PATH_SNAPSHOT, None, LINK_ESTABLISH_WAIT)
        except asyncio.TimeoutError:
            # Most likely the identify packet was lost, and the peer refuses
            # every request on a link it cannot attribute. Nothing on this
            # link would ever be answered, so it goes, and the dial loop
            # opens a fresh one that identifies again.
            logger.warning("snapshot exchange with %s went unanswered; redialling", peer.hex())
            link.teardown()
            return
        if isinstance(theirs, list):
            online = [a for a in theirs if wire.address_field(a) is not None]
            self._apply(peer, {"snap": True, "on": online, "off": []})

    def _out_link_closed(self, peer: bytes, link: Any) -> None:
        if self._out.get(peer) is link:
            del self._out[peer]
            logger.info("backbone link to %s closed", peer.hex())
            self._forget(peer)

    def _inbound_link(self, link: Any) -> None:
        """A peer dialled in. It is served once it identifies as a
        provisioned peer, and torn down if it identifies as anyone else."""
        loop = self._loop
        assert loop is not None

        def on_identified(link: Any, identity: Any) -> None:
            loop.call_soon_threadsafe(self._inbound_identified, link, identity)

        def on_closed(link: Any) -> None:
            loop.call_soon_threadsafe(self._inbound_closed, link)

        link.set_remote_identified_callback(on_identified)
        link.set_link_closed_callback(on_closed)

    def _inbound_identified(self, link: Any, identity: Any) -> None:
        peer = self._peer_of(identity)
        if peer is None:
            logger.warning("a link identified as an unprovisioned gateway; closing it")
            link.teardown()
            return
        previous = self._in.get(peer)
        self._in[peer] = link
        if previous is not None and previous is not link:
            previous.teardown()

    def _inbound_closed(self, link: Any) -> None:
        for peer, held in list(self._in.items()):
            if held is link:
                del self._in[peer]
                # The peer can no longer push changes, so what it claimed is
                # going stale. Forgetting it costs a re-snapshot on the next
                # link; keeping it opens paths on stale claims.
                self._forget(peer)

    def _active_out(self, peer: bytes) -> Any:
        import RNS

        link = self._out.get(peer)
        if link is not None and link.status == RNS.Link.ACTIVE:
            return link
        return None

    def _peer_of(self, identity: Any) -> bytes | None:
        import RNS

        if identity is None:
            return None
        peer = RNS.Destination.hash(identity, APP_NAME, *ASPECTS)
        return peer if peer in self.peers else None

    # -- Requests --------------------------------------------------------

    async def _request(self, link: Any, path: str, data: Any, timeout: float) -> Any:
        loop = self._loop
        assert loop is not None
        future: asyncio.Future[Any] = loop.create_future()

        def settle(value: Any, failed: bool) -> None:
            if future.done():
                return
            if failed:
                future.set_exception(asyncio.TimeoutError())
            else:
                future.set_result(value)

        receipt = link.request(
            path,
            data=data,
            response_callback=lambda r: loop.call_soon_threadsafe(settle, r.response, False),
            failed_callback=lambda r: loop.call_soon_threadsafe(settle, None, True),
            timeout=timeout,
        )
        if receipt is False:
            raise asyncio.TimeoutError()
        return await asyncio.wait_for(future, timeout + 1)

    def _spawn(self, coro: Any) -> None:
        task = asyncio.ensure_future(coro)
        self._tasks.append(task)
        task.add_done_callback(lambda t: self._tasks.remove(t) if t in self._tasks else None)

    # Request handlers. Reticulum runs each on a thread of its own; each hops
    # onto the event loop for anything it reads or changes.

    def _handle_forward(self, path, data, request_id, link_id, remote_identity, requested_at):
        peer = self._peer_of(remote_identity)
        if peer is None:
            return {"o": ForwardResult.REFUSED.value, "w": NOT_A_PEER}
        frame = _frame_from(data)
        if frame is None:
            return {"o": ForwardResult.REFUSED.value, "w": wire.MALFORMED_REQUEST}
        result, reason = self._on_loop(self._deliver(frame, peer), (ForwardResult.REFUSED, "busy"))
        return {"o": result.value, "w": reason}

    async def _deliver(self, frame: BackboneFrame, peer: bytes) -> tuple[ForwardResult, str]:
        if self.hooks is None:
            return ForwardResult.REFUSED, "stopping"
        return await self.hooks.deliver_from_backbone(frame, peer.hex())

    def _handle_presence(self, path, data, request_id, link_id, remote_identity, requested_at):
        peer = self._peer_of(remote_identity)
        if peer is None or not isinstance(data, dict):
            return None
        assert self._loop is not None
        self._loop.call_soon_threadsafe(self._apply, peer, data)
        return True

    def _handle_snapshot(self, path, data, request_id, link_id, remote_identity, requested_at):
        if self._peer_of(remote_identity) is None:
            return None
        return self._on_loop(self._local_addresses(), [])

    async def _local_addresses(self) -> list[str]:
        return self.hooks.local_addresses() if self.hooks is not None else []

    def _on_loop(self, coro: Any, fallback: Any) -> Any:
        assert self._loop is not None
        future = asyncio.run_coroutine_threadsafe(coro, self._loop)
        try:
            return future.result(HANDLER_WAIT)
        except Exception:
            future.cancel()
            return fallback

    # -- Presence bookkeeping (on the loop) ------------------------------

    def _apply(self, peer: bytes, data: dict[str, Any]) -> None:
        key = peer.hex()
        changed: list[str] = []
        online = [a for a in _list(data.get("on")) if wire.address_field(a) is not None]
        if data.get("snap") is True:
            changed += self.table.snapshot(key, online)
        else:
            for address in online:
                if self.table.update(key, address, True, None):
                    changed.append(address)
        for entry in _list(data.get("off")):
            if isinstance(entry, list) and len(entry) == 2 and wire.address_field(entry[0]) is not None:
                seen = entry[1] if isinstance(entry[1], int) and not isinstance(entry[1], bool) else None
                if self.table.update(key, entry[0], False, seen):
                    changed.append(entry[0])
        self._announce_changes(changed)

    def _forget(self, peer: bytes) -> None:
        self._announce_changes(self.table.drop_peer(peer.hex()))

    def _announce_changes(self, addresses: list[str]) -> None:
        if self.hooks is None:
            return
        for address in dict.fromkeys(addresses):
            self.hooks.remote_presence_changed(address)


def _load_identity(RNS: Any, path: str) -> Any:
    if os.path.exists(path):
        identity = RNS.Identity.from_file(path)
        if identity is None:
            raise RuntimeError(f"{path} is not a Reticulum identity")
        return identity
    identity = RNS.Identity()
    directory = os.path.dirname(os.path.abspath(path))
    os.makedirs(directory, exist_ok=True)
    identity.to_file(path)
    # The private key of this gateway's backbone identity: whoever holds it
    # can answer for every device attached here.
    os.chmod(path, 0o600)
    logger.info("created a new gateway identity at %s", path)
    return identity


def _list(value: Any) -> list[Any]:
    return value if isinstance(value, list) else []


def _frame_from(data: Any) -> BackboneFrame | None:
    if not isinstance(data, dict):
        return None
    recipient = wire.address_field(data.get("r"))
    sender = wire.address_field(data.get("s"))
    content = data.get("c")
    if recipient is None or sender is None or not isinstance(content, (bytes, bytearray)):
        return None
    return BackboneFrame(
        recipient=recipient,
        sender=sender,
        content=bytes(content),
        message_id=wire.message_id_field(data.get("m")),
        reply_to_msg=wire.message_id_field(data.get("p")),
    )


def destination_hash_for(identity_path: str) -> str:
    """The destination hash a gateway with this identity file announces: what
    its peers put in their ``peers`` list. Creates the identity if absent."""
    import RNS

    identity = _load_identity(RNS, identity_path)
    return RNS.Destination.hash(identity, APP_NAME, *ASPECTS).hex()
