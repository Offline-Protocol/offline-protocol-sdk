"""What the daemon needs from a backbone, and the presence bookkeeping every
backbone shares.

The backbone is the gateway's property (ADR 0026): a device never sees one,
and learns of it only as a ``backbone_<kind>_v1`` capability token. This
module is the seam between the device-facing daemon (``gateway.py``) and
whatever carries frames between gateways. ``reticulum.py`` is the reference
implementation; ``InMemoryNetwork`` below joins gateways inside one process,
for tests.

The contract's four obligations on a backbone ("What a backbone owes a
gateway") map onto this interface as:

1. gateway-to-gateway presence: :meth:`Backbone.claim` answers from what peer
   gateways pushed, and :meth:`Backbone.local_presence_changed` pushes ours;
2. arbitrary-size framing: :meth:`Backbone.forward` takes a whole frame, and
   how it is segmented is the implementation's business;
3. a provisioned peer list: each implementation's own configuration;
4. a declared rate class: :attr:`Backbone.rate_class`, applied by the daemon
   at its own egress.
"""

from __future__ import annotations

import abc
import asyncio
import time
from collections import OrderedDict
from dataclasses import dataclass
from enum import Enum
from typing import Callable, Protocol


#: ``admit(peer) -> bool``; see :meth:`Backbone.forward`.
Admit = Callable[[str], bool]


class ForwardResult(Enum):
    """What a peer gateway said about a frame it was handed."""

    #: Written to a session attached at the peer gateway.
    DELIVERED = "delivered"
    #: The peer gateway has no session for the recipient.
    UNREACHABLE = "unreachable"
    #: The peer gateway refused the frame (budget, size). A plain failure.
    REFUSED = "refused"
    #: No answer in time, or no link to the peer. A plain failure.
    TIMEOUT = "timeout"


@dataclass(frozen=True)
class BackboneFrame:
    """One device frame, as it crosses between gateways.

    ``content`` is the ciphertext the device submitted, untouched. The
    gateways see it and the routing metadata around it, never a plaintext
    (invariant 4).
    """

    recipient: str
    sender: str
    content: bytes
    message_id: str | None
    reply_to_msg: str | None


@dataclass(frozen=True)
class Claim:
    """A peer gateway's statement that it has, or had, a session for an
    address. A claim, never settlement: it may open a path, never close one."""

    peer: str
    online: bool
    last_seen_ms: int | None
    received_at: float
    #: The claiming gateway's number for the change this claim reflects.
    #: Later changes carry larger numbers; see :class:`PresenceTable`.
    seq: int = 0


class BackboneHooks(Protocol):
    """What a backbone may ask of the daemon it serves."""

    async def deliver_from_backbone(self, frame: BackboneFrame, peer: str) -> tuple[ForwardResult, str]:
        """Hand a frame from ``peer`` to a local session. Returns the result
        and, for a refusal, its reason token."""

    #: Random per daemon process, so a peer can tell a restarted gateway's
    #: numbering, which starts again from zero, from a stale message.
    presence_epoch: str

    def local_snapshot(self) -> tuple[int, dict[str, int]]:
        """The current presence number, and every address with a bound
        session here mapped to the number of the change that bound it."""

    def remote_presence_changed(self, address: str) -> None:
        """A peer gateway's claim about ``address`` changed."""


class Backbone(abc.ABC):
    """A wide-area carrier between gateways."""

    #: The ``<kind>`` in the ``backbone_<kind>_v1`` token, lowercase.
    kind: str
    #: ``"scarce"`` or ``"broad"``, per the contract's table of kinds. A kind
    #: not in that table is scarce.
    rate_class: str = "scarce"

    @abc.abstractmethod
    async def start(self, hooks: BackboneHooks) -> None: ...

    @abc.abstractmethod
    async def stop(self) -> None: ...

    @abc.abstractmethod
    def claim(self, address: str) -> Claim | None:
        """The freshest claim any peer gateway has made about ``address``."""

    @abc.abstractmethod
    async def forward(
        self, frame: BackboneFrame, timeout: float, admit: Admit | None = None
    ) -> tuple[ForwardResult, str]:
        """Carry ``frame`` to the peer gateway that claims its recipient.

        Answers within ``timeout`` seconds: the daemon owes the device a
        verdict, and the reference clients fail a frame that has none in 60.

        ``admit(peer)`` is called once, at the moment the frame is about to
        be put on the backbone toward ``peer``, and nowhere else: it charges
        the scarce budgets, so a frame that never reaches the backbone
        (queued past its time, no link, no claim) is never charged. When it
        returns ``False`` the frame is not sent and the answer is
        ``REFUSED`` with ``budget_exceeded``.
        """

    @abc.abstractmethod
    def local_presence_changed(self, address: str, online: bool, last_seen_ms: int | None, seq: int) -> None:
        """Tell the peer gateways that ``address`` attached here or left, as
        change number ``seq``."""


class PresenceTable:
    """Claims received from peer gateways, keyed by address and by peer.

    **Ordering.** Presence messages from one peer can arrive out of order:
    Reticulum runs each request on a thread of its own, and a snapshot large
    enough to travel as a resource lands after a one-packet update sent later.
    Applied in arrival order, a snapshot taken before an attach would erase it
    and leave an offline claim that closes a path the contract says a gateway
    may only open. So every change carries the sending gateway's number for
    it, every snapshot carries the number current when it was taken, and a
    claim is replaced only by something newer: an entry with a larger number,
    or, for an address the snapshot omits, a snapshot taken after the change
    that claim reflects. The numbers restart with the sending process, so
    each peer's messages also carry an epoch, and a new epoch discards
    everything that peer said before.

    Every claim a peer made is dropped when the link to that peer goes,
    because a claim from a gateway this one can no longer reach opens no path.
    Bounded: past ``capacity`` addresses the least recently updated go.
    """

    def __init__(self, capacity: int = 65536, clock: Callable[[], float] = time.monotonic) -> None:
        self._claims: OrderedDict[str, dict[str, Claim]] = OrderedDict()
        self._epochs: dict[str, str] = {}
        #: Per peer, the newest snapshot applied. A snapshot speaks for every
        #: address, including ones it holds no claim about (forgotten when a
        #: link dropped, or never heard of), so nothing older than it may
        #: create a claim: a late resend of an old attach would otherwise
        #: bring back a device that has since left.
        self._floors: dict[str, int] = {}
        self._capacity = capacity
        self._clock = clock

    def update(
        self,
        peer: str,
        address: str,
        online: bool,
        last_seen_ms: int | None,
        seq: int,
        epoch: str = "",
        *,
        _in_snapshot: bool = False,
    ) -> list[str]:
        """Records a claim unless one at least as new is held. Returns the
        addresses whose best claim changed as a result."""
        changed = self._enter_epoch(peer, epoch)
        held = self._claims.get(address, {}).get(peer)
        if held is not None and held.seq >= seq:
            return changed
        # A snapshot's own entries are current by definition, whatever their
        # number: one listing a device that attached long ago must restore it
        # after its claim was forgotten.
        if held is None and not _in_snapshot and seq <= self._floors.get(peer, 0):
            return changed
        before = self.claim(address)
        by_peer = self._claims.pop(address, {})
        by_peer[peer] = Claim(peer, online, last_seen_ms, self._clock(), seq)
        self._claims[address] = by_peer
        while len(self._claims) > self._capacity:
            self._claims.popitem(last=False)
        if _differs(before, self.claim(address)):
            changed.append(address)
        return changed

    def snapshot(self, peer: str, online: dict[str, int], snapshot_seq: int, epoch: str = "") -> list[str]:
        """Applies ``peer``'s full list of attached addresses, each with the
        number of the change that attached it, taken at ``snapshot_seq``.
        Returns the addresses whose best claim changed."""
        changed = self._enter_epoch(peer, epoch)
        for address in [a for a, by in self._claims.items() if peer in by and a not in online]:
            claim = self._claims[address][peer]
            # Newer than the snapshot: the change it reflects happened after
            # the snapshot was taken, so the snapshot's silence says nothing.
            if not claim.online or claim.seq >= snapshot_seq:
                continue
            before = self.claim(address)
            self._claims[address][peer] = Claim(peer, False, claim.last_seen_ms, self._clock(), snapshot_seq)
            if _differs(before, self.claim(address)):
                changed.append(address)
        for address, seq in online.items():
            changed += self.update(peer, address, True, None, seq, epoch, _in_snapshot=True)
        # Raised after the snapshot's own entries, which are all numbered at
        # or below it, are in.
        self._floors[peer] = max(self._floors.get(peer, 0), snapshot_seq)
        return changed

    def drop_peer(self, peer: str) -> list[str]:
        """Forgets every claim ``peer`` made. Returns the addresses whose best
        claim changed."""
        changed = []
        for address in [a for a, by in self._claims.items() if peer in by]:
            before = self.claim(address)
            by_peer = self._claims[address]
            del by_peer[peer]
            if not by_peer:
                del self._claims[address]
            if _differs(before, self.claim(address)):
                changed.append(address)
        return changed

    def claim(self, address: str) -> Claim | None:
        """An online claim if any peer makes one (the newest such), otherwise
        the newest offline one."""
        by_peer = self._claims.get(address)
        if not by_peer:
            return None
        online = [c for c in by_peer.values() if c.online]
        pool = online or list(by_peer.values())
        return max(pool, key=lambda c: c.received_at)

    def _enter_epoch(self, peer: str, epoch: str) -> list[str]:
        if self._epochs.get(peer) == epoch:
            return []
        self._epochs[peer] = epoch
        self._floors.pop(peer, None)
        return self.drop_peer(peer)


def _differs(a: Claim | None, b: Claim | None) -> bool:
    if a is None or b is None:
        return (a is None) != (b is None)
    return (a.online, a.peer) != (b.online, b.peer)


# -- An in-process backbone, for tests ---------------------------------------


class InMemoryNetwork:
    """Joins in-process backbones as if every pair had a live link. Tests cut
    and restore a pair's link with :meth:`cut` and :meth:`join`."""

    def __init__(self) -> None:
        self.members: dict[str, "InMemoryBackbone"] = {}
        self._cut: set[frozenset[str]] = set()

    def linked(self, a: str, b: str) -> bool:
        return a != b and frozenset((a, b)) not in self._cut and a in self.members and b in self.members

    def cut(self, a: str, b: str) -> None:
        self._cut.add(frozenset((a, b)))
        for x, y in ((a, b), (b, a)):
            member = self.members.get(x)
            if member is not None:
                member._link_lost(y)

    def join(self, a: str, b: str) -> None:
        self._cut.discard(frozenset((a, b)))
        for x, y in ((a, b), (b, a)):
            member = self.members.get(x)
            if member is not None:
                member._link_up(y)


class InMemoryBackbone(Backbone):
    kind = "memory"
    rate_class = "broad"

    def __init__(self, network: InMemoryNetwork, name: str, rate_class: str = "broad") -> None:
        self.network = network
        self.name = name
        self.rate_class = rate_class
        self.table = PresenceTable()
        self.hooks: BackboneHooks | None = None
        #: Seconds each forward waits before it is carried, to exercise the
        #: daemon's timeout handling.
        self.delay = 0.0

    async def start(self, hooks: BackboneHooks) -> None:
        self.hooks = hooks
        self.network.members[self.name] = self
        for other in list(self.network.members):
            if self.network.linked(self.name, other):
                self._link_up(other)
                self.network.members[other]._link_up(self.name)

    async def stop(self) -> None:
        self.network.members.pop(self.name, None)
        for other in list(self.network.members.values()):
            other._link_lost(self.name)
        self.hooks = None

    def claim(self, address: str) -> Claim | None:
        return self.table.claim(address)

    async def forward(
        self, frame: BackboneFrame, timeout: float, admit: Admit | None = None
    ) -> tuple[ForwardResult, str]:
        claim = self.table.claim(frame.recipient)
        if claim is None or not claim.online or not self.network.linked(self.name, claim.peer):
            return ForwardResult.UNREACHABLE, "no peer gateway claims the recipient"
        if admit is not None and not admit(claim.peer):
            return ForwardResult.REFUSED, "budget_exceeded"
        peer = self.network.members[claim.peer]
        try:
            return await asyncio.wait_for(peer._receive(frame, self.name), timeout)
        except asyncio.TimeoutError:
            return ForwardResult.TIMEOUT, "no answer from the peer gateway"

    def local_presence_changed(self, address: str, online: bool, last_seen_ms: int | None, seq: int) -> None:
        assert self.hooks is not None
        epoch = self.hooks.presence_epoch
        for other in list(self.network.members.values()):
            if self.network.linked(self.name, other.name):
                other._claimed(self.name, address, online, last_seen_ms, seq, epoch)

    async def _receive(self, frame: BackboneFrame, sender: str) -> tuple[ForwardResult, str]:
        if self.delay:
            await asyncio.sleep(self.delay)
        if self.hooks is None:
            return ForwardResult.TIMEOUT, "stopped"
        return await self.hooks.deliver_from_backbone(frame, sender)

    def _claimed(
        self, peer: str, address: str, online: bool, last_seen_ms: int | None, seq: int, epoch: str
    ) -> None:
        for changed in self.table.update(peer, address, online, last_seen_ms, seq, epoch):
            if self.hooks is not None:
                self.hooks.remote_presence_changed(changed)

    def _link_up(self, peer: str) -> None:
        other = self.network.members.get(peer)
        if other is None or other.hooks is None or self.hooks is None:
            return
        snapshot_seq, online = other.hooks.local_snapshot()
        for address in self.table.snapshot(peer, online, snapshot_seq, other.hooks.presence_epoch):
            self.hooks.remote_presence_changed(address)

    def _link_lost(self, peer: str) -> None:
        if self.hooks is None:
            return
        for address in self.table.drop_peer(peer):
            self.hooks.remote_presence_changed(address)

