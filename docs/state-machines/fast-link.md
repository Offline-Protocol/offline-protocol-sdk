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
   over a live direct link (Bluetooth LE whenever the peer is linked there,
   never down the stream the frame may be ending), and never enters the outbox, a relay or a
   neighbour's custody. A frame that arrives in plaintext is never routed here.
   Nor is one that reached this device before its session did: it waits
   undecrypted with every other sealed body, for up to a day, and is consumed
   when the session is ready, never acted on.
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
| `join` | Joins the given network and, once it is on it, dials `host:port` (an iPhone knows it is on it when its Wi-Fi path's gateway is `host`, and ends the join `failed` after 10 looks, 2 s apart, that find otherwise; a `host` that is already its gateway before the prompt, over 4 looks, ends the join `invalid`, since the dial would go to the user's own router) (Android dials the group owner's own address with that port, since it stays on its own Wi-Fi), claiming the given peer | the stream's preamble, through `wifi_direct_peer_connected`, or `wifi_direct_fast_link_ended(link, reason)` |
| `close` | Ends the stream it opened for the link and leaves or removes the network | nothing |

A platform declares what it can do with
`wifi_direct_set_fast_link_roles(can_host, can_join, accept_unrequested)`.
Until it does, all are false and every request is refused. The third is the
application's `wifiDirect.fastLink` setting, not the platform's: with it
false (`requestedOnly`), a peer's `request` and an `offer` this device did
not ask for are answered `declined`, so only links this application asked
for go ahead and no join prompt or group appears that it did not want.

## The host

| State | Entered when | Leaves to |
|-------|--------------|-----------|
| `hosting` | The application asked and this device can host, or a peer's `request` arrived and it can | `offered` on the platform's report; `closed` (`host_failed`) after 20 s without one, or on the platform's `ended` |
| `offered` | The platform reported the network up; the `offer` is sent | `up` when a stream proves the peer; `closed` (`expired`) 120 s after the offer was made, or (`superseded`) when the platform reports a stream other than the link's proved the peer first |
| `up` | A stream proved the peer while the link was `offered` | `closed` on the stream's loss (`lost`), after 60 s idle (`idle`), on `close` from either side |

## The joiner

| State | Entered when | Leaves to |
|-------|--------------|-----------|
| `requested` | The application asked and this device can only join; the `request` is sent | `joining` on the peer's `offer`; `closed` (`no_offer`) after 30 s, or on the peer's `close` |
| `joining` | An `offer` arrived, by request or not, and this device can join | `up` when a stream proves the host; `closed` (`expired`) at the offer's lifetime, measured from its arrival, or on the platform's `ended` (`superseded` when a stream other than the link's proved the host first) |
| `up` | A stream proved the host while the link was `joining` | as for the host |

## Frames that cross a link

**A `request` arrives.** A device that cannot host answers `close`
(`cannot_host`); one holding a link with another peer answers `close` (`busy`).
One that already offered this peer a link sends the same offer again, since
the request means the first one was lost, with only the lifetime it has left
(the joiner counts an offer's lifetime from its arrival) and, when the
request has an id of its own, naming that request in `re`. Otherwise it starts hosting under
the request's link id, unless two links it hosted in a row for that peer's
requests carried nothing (no body of 16 KiB or more, no transfer) and ended by
expiring, going idle, losing their stream, or at the peer's hand, whatever
reason it gives: then it answers `busy` for ten minutes, so a peer cannot keep
a group on the air, and the device's own group formation locked out, by
asking again each time. A link that came up counts the same as one never
joined unless it carried something: otherwise a peer could join, carry
nothing, let the link idle out and ask again, without end. A miss more than
ten minutes before the next is forgotten, so two in a row means two close
together. One miss
is forgiven, so a user who did not tap in time can ask again, and this
device's own request to the peer ends the pause.

When an offer answers this device's request under a different link id, the
request's id is remembered as ended, so a host that acts on the request late
cannot bring up a second network and prompt the user for it.

**An `offer` arrives.** A device that cannot join answers `close`
(`cannot_join`), and one holding a link with another peer answers `busy`. For
ten minutes after this device's user declined a peer's network, that peer's
offers are answered `declined` and never shown: each join on iOS is a system
prompt, and without the pause a peer could put up another the moment one was
refused. Two offers in a row that the peer made unasked and that ended
having carried nothing, up or not, start the same pause, whatever the peer's reason or
the offer's expiry, unless this device ended them itself (its application
closing the link, leaving the foreground, another stream reaching the peer
first): a prompt the user ignores is never
declined, and without this the peer could prompt again once per offer
lifetime. One is forgiven, a link that carries something clears the count, and the
user's own request to that peer ends the pause. A second offer
from the peer while one is being joined or is up is answered `busy` and the
first stands: an honest host's new network follows its own `close`, and a
swap would let a peer re-offer while each prompt is on screen, so that no
decline is ever reported and the pause never begins. An
offer that fails validation (below) is answered `close` (`invalid`) and nothing
else happens. When both devices are hosting for each other, the lower address
keeps hosting: the higher one closes its own link and joins, and the lower one
ignores the higher one's offer, which its owner is about to withdraw. A host
whose link is already up answers any offer `busy`, as a joiner does: the
tie-break is for two hosts racing, and applied once a link is up it would
swap a working link for a second group, which an Android joins with no
prompt. So does a host whose link the peer asked for: a peer that asks
cannot host, so its offer is never honest, and acted on it ended the hosted
link uncounted and joined an offer taken as asked for, so a peer that asked
and then offered, over and over, met neither the host backoff nor the pause
on unasked offers.

**A `close` arrives.** It ends the link with that id, if there is one, and is
never answered.

**A request or offer arrives for a link this device ended** within the last
ten minutes (the longest an offer lives, plus the clock slack). It is late. A
`close` never is: it ends only the link held under its id, and treated as
late it was dropped for a link adopted through `re` (below), whose id this
device had ended once, so the host's close never stopped the join. A
request is dropped; an offer is answered `close` (`expired`), so the host
takes its network down, and is never joined. On two phones a request
delayed 63 s reached the host after the requester had given up, and before
this rule its offer prompted the requester's user to join a network that had
already been removed. The one exception is an offer that names, in its
`re`, the request this device is waiting on now: a host still offering the
old network sends it again for a request under a new id (the requester's
`close` for the old one was lost too), and refusing it ended the user's
second attempt `no_offer` with nothing shown. An offer that is merely late,
however much the user has asked again since, is refused as above: let
through, it prompted for a network the host had already removed, and the
second network's offer, then refused as late, started the host's backoff
against a user who had only asked twice.

An offer is valid when its name is 9 to 32 bytes starting `DIRECT-`, as a
Wi-Fi Direct group's is, its passphrase 8 to 63
printable ASCII characters, its host a private or link-local IP literal, its
port not zero, its lifetime at most 300 s, and the time it states it was made no
more than 300 s in the future or more than its lifetime plus 300 s in the
past by the receiver's clock. The second bound is generous because two phones'
clocks disagree; what the receiver enforces is its own deadline, the offer's
lifetime from the moment it arrived.

## Ending a link

Every end, whatever its cause, does the same three things: a `close` directive
for this device's platform when the platform has taken a directive for the
link (one it has not taken yet is withdrawn instead, so a paused platform
never brings up, or prompts for, a network after its link ended), a
`close` frame for the peer when the peer knows of the link and the end did not
come from the peer, and one `fast_link_changed` event with state `closed` and
the reason.

The idle limit counts a body of at least 16 KiB carried by the slot in either
direction for this peer as activity, and does not run while a file transfer
to the peer is in flight over the link. A transfer's carrier is fixed when it
starts, so one begun over Bluetooth before the link came up stays there and
does not hold the link. Smaller bodies (presence, receipts, chat) do not
count: Bluetooth LE carries them well, and on two phones an application's
presence update every 15 s held a link up indefinitely when they did. A link
that carries nothing large for 60 s is costing the user their own Wi-Fi for
no reason.

A link that was up and ends fails every file transfer to the peer that was
pinned to the slot, at once, with `media_send_failed`, and withdraws its
chunks. A transfer's carrier is fixed when it starts, so it would otherwise
retry on a slot that no longer reaches the peer until its retries ran out,
and its chunks are sized for the stream, larger than Bluetooth carries. The
application sends it again over what remains.

Pausing the engine ends a link with `background`: a paused engine runs none
of these timers, and a host's group would otherwise stay on the air until the
application returned, with nothing bounding it. Stopping it ends a link with
`closed`, for the same reason and one more: an engine started again would
find the old link still held and answer every request `busy`. A platform
whose transport stops reports the link ended `closed` before it reports its
streams lost: reported after, the engine has already seen the link's stream
drop and ended it `lost`, a miss charged to a peer that offered it.

A host whose process dies ends nothing: an Android group belongs to the
system, not the process, and stays on the air. The host's platform keeps the
group's name on disk until it sees the group gone, and on its next start
removes an owned group of that name rather than joining the socket layer to
it as an ordinary group, which left it up with no timer and refused every
later link.

A joiner's platform that is about to lose the network reports it as `ended`
first: on iOS, when the application pauses the engine on leaving the
foreground, after which iOS disconnects at 15 s whatever the engine thinks.
Without the report the host learns from TCP keepalive, about 40 s later. An
application that leaves the foreground without pausing still ends the link:
its transport reports itself down, which ends an up link `lost`, and a join
still in progress ends `expired` at its offer's lifetime.

## Events

`fast_link_changed` carries the peer, the link id, the role (`host` or
`join`), the state (one of the six above) and, on `closed`, the reason. It is
how the application learns the outcome of `request_fast_link`, which returns
as soon as the first step is taken. The application matches it by peer, of
which a device holds one link at a time, not by the id the request returned:
when the host answers with a network it already had (a request asked again
after a lost offer), the link takes the host's id and the request's id sees
no further event. It never carries a network name or a
passphrase.

## What the shapes prevent

- **An offer through the outbox** reaches the joiner after the network is gone
  and prompts its user for nothing. Hence ephemeral frames and a lifetime.
- **A host that admits whoever proves an address** on its network is an access
  point for anyone who learns the passphrase, which is what R22 is for group
  formation. Hence the admitted peer, and a passphrase only the two hold.
- **Two links at once** are two networks on a phone that can join one, and the
  second join drops the first without either side being told.
- **Another stream taken for the link's own.** Two phones back on one home
  network meet over it while an offer is pending. Counted as the link's
  stream, it marked the link `up`, the iPhone never dialled the group its user
  had just joined, and leaving the home network for it lost both 40 s later.
  Hence a platform that sees a stream other than the link's own prove the
  link's peer first ends the link `superseded` before announcing the stream,
  and answers a directive for a peer a stream already reaches the same way;
  the engine answers a `request` or `offer` from such a peer `superseded`
  itself, with no link, since the peer's own `superseded` can overtake this
  device's report and would count against the peer. A stream over the
  group itself is the link's own however it was dialled: an Android below 13
  advertises on the group's interface, and an iPhone's general dial can
  prove the host there before the link's own dial, over a link-local
  address before DHCP, so once the iPhone has joined only a stream whose path
  has a gateway that is not the host gives way.
  Such an end is the platform's own and never counts against the peer, nor
  does the peer's own `superseded` for a link not yet up here while another
  stream reaches the peer (its close can overtake this device's report;
  counted, two meetings on a home network paused the peer). A peer's
  `superseded` for a link up here is any close: the link's own stream
  carried it. The engine also ends a join its platform has not yet taken
  `superseded` when a stream proves the peer, since such a link has no
  stream of its own. A link this device superseded fails no transfer on the
  slot: the transfers there are the other stream's.
- **A joiner that falls silent** when iOS takes the network away leaves the
  host's stream half open until keepalive ends it. Hence the platform's
  report.
