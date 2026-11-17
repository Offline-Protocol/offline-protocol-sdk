# Fast link

Two devices that share a confirmed session but meet only over Bluetooth LE
bring up a Wi-Fi network between them, and a peer stream over it fills the
`wifi_direct` slot. The decision and its reasons are
[ADR 0038](../adr/0038-a-fast-link-is-offered-inside-the-session.md); the frame
is in [control messages](../spec/control-messages.md#fast-link-frames). This
document is the runtime behaviour: what each side holds, what moves it, and
what each shape prevents.

## Invariants

1. **Credentials leave a device only inside the confirmed 1:1 session with the
   peer they are for.** A fast-link frame is sealed or not sent, travels once
   over a live direct link, and never enters the outbox, a relay or a
   neighbour's custody. A frame that arrives in plaintext is never routed here.
2. **A link never announces a peer.** Only a stream's preamble does, exactly as
   on every other network. The joiner's stream is opened with the host's
   address as its claim, and the host refuses any other proved address on the
   network it made.
3. **One link per device at a time.** A second request, from either side, is
   answered `busy`.
4. **Whichever side ends a link tells its own platform and the peer.** A lost
   `close` is bounded by the other side's own timers: the offer's lifetime, the
   stream's loss, or the idle limit.
5. **No event, diagnostic or telemetry record carries the passphrase.** It
   exists in the sealed frame, in the directive handed to the platform, and in
   the platform's network configuration, and nowhere else.

Invariant 1 is why the frames are ephemeral rather than reliable. A reliable
offer that reached the joiner from the outbox after a reconnect would name a
network that no longer exists, and the joiner would prompt its user to join
nothing.

## Who does what

The **engine** holds the link, assigns roles, makes credentials, seals and
reads the frames, runs every timer and decides when the link ends. The
**platform** brings a network up or down and opens the stream when the engine
tells it to, and reports what happened. They talk through a queue of
directives the platform drains when its transport callback fires, the same
wake-up that drains outbound stream bodies:

| Directive | The platform | Then reports |
|-----------|--------------|--------------|
| `host` | Brings up a network with the given name and passphrase, admitting the given peer only | `wifi_direct_fast_link_hosting(link, host, port)`, or `wifi_direct_fast_link_ended(link, reason)` |
| `join` | Joins the given network and dials `host:port`, claiming the given peer | the stream's preamble, through `wifi_direct_peer_connected`, or `wifi_direct_fast_link_ended(link, reason)` |
| `close` | Ends the stream it opened for the link and leaves or removes the network | nothing |

A platform declares what it can do with
`wifi_direct_set_fast_link_roles(can_host, can_join)`. Until it does, both are
false and every request is refused.

## The host

| State | Entered when | Leaves to |
|-------|--------------|-----------|
| `hosting` | The application asked and this device can host, or a peer's `request` arrived and it can | `offered` on the platform's report; `closed` (`host_failed`) after 20 s without one, or on the platform's `ended` |
| `offered` | The platform reported the network up; the `offer` is sent | `up` when a stream proves the peer; `closed` (`expired`) 120 s after the offer was made |
| `up` | A stream proved the peer while the link was `offered` | `closed` on the stream's loss (`lost`), after 60 s idle (`idle`), on `close` from either side |

## The joiner

| State | Entered when | Leaves to |
|-------|--------------|-----------|
| `requested` | The application asked and this device can only join; the `request` is sent | `joining` on the peer's `offer`; `closed` (`no_offer`) after 30 s, or on the peer's `close` |
| `joining` | An `offer` arrived, by request or not, and this device can join | `up` when a stream proves the host; `closed` (`expired`) at the offer's lifetime, measured from its arrival, or on the platform's `ended` |
| `up` | A stream proved the host while the link was `joining` | as for the host |

## Frames that cross a link

**A `request` arrives.** A device that cannot host answers `close`
(`cannot_host`); one holding a link with another peer answers `close` (`busy`).
One that already offered this peer a link sends the same offer again, since
the request means the first one was lost. Otherwise it starts hosting under
the request's link id.

**An `offer` arrives.** A device that cannot join answers `close`
(`cannot_join`), and one holding a link with another peer answers `busy`. An
offer that fails validation (below) is answered `close` (`invalid`) and nothing
else happens. When both devices are hosting for each other, the lower address
keeps hosting: the higher one closes its own link and joins, and the lower one
ignores the higher one's offer, which its owner is about to withdraw.

**A `close` arrives.** It ends the link with that id, if there is one, and is
never answered.

An offer is valid when its name is 1 to 32 bytes, its passphrase 8 to 63
printable ASCII characters, its host a private or link-local IP literal, its
port not zero, its lifetime at most 300 s, and the time it states it was made no
more than 300 s in the future or more than its lifetime plus 300 s in the
past by the receiver's clock. The second bound is generous because two phones'
clocks disagree; what the receiver enforces is its own deadline, the offer's
lifetime from the moment it arrived.

## Ending a link

Every end, whatever its cause, does the same three things: a `close` directive
for this device's platform when the platform holds anything for the link, a
`close` frame for the peer when the peer knows of the link and the end did not
come from the peer, and one `fast_link_changed` event with state `closed` and
the reason.

The idle limit counts a body carried by the slot in either direction for this
peer as activity, and does not run while a file transfer to the peer is in
flight. A link that carries nothing for 60 s is costing the user their own
Wi-Fi for no reason.

A joiner's platform that is about to lose the network reports it as `ended`
first: on iOS, leaving the foreground, after which iOS disconnects at 15 s
whatever the engine thinks. Without the report the host learns from TCP
keepalive, about 40 s later.

## Events

`fast_link_changed` carries the peer, the link id, the role (`host` or
`join`), the state (one of the six above) and, on `closed`, the reason. It is
how the application learns the outcome of `request_fast_link`, which returns
as soon as the first step is taken. It never carries a network name or a
passphrase.

## What the shapes prevent

- **An offer through the outbox** reaches the joiner after the network is gone
  and prompts its user for nothing. Hence ephemeral frames and a lifetime.
- **A host that admits whoever proves an address** on its network is an access
  point for anyone who learns the passphrase, which is what R22 is for group
  formation. Hence the admitted peer, and a passphrase only the two hold.
- **Two links at once** are two networks on a phone that can join one, and the
  second join drops the first without either side being told.
- **A joiner that falls silent** when iOS takes the network away leaves the
  host's stream half open until keepalive ends it. Hence the platform's
  report.
