# 0030. One Bluetooth LE link per peer pair

**Status:** Accepted
**See also:** [BLE framing, invariants](../spec/ble-framing.md#invariants),
[Transport lifecycle](../state-machines/transport-lifecycle.md)

## Context

Android allowed one client link beside one server link to the same peer, one
per direction, and refused only a second client link (the macOS rotation case,
#548). Its 120-second "forced BLE refresh" restarted the advertisement, and the
stack gives every advertising set a new random address. iOS reported a peer
lost whenever a dial to one of its identifiers dropped, unless that same
identifier had been connected.

In run 5 of the 2026-10-19 volume test (iPhone 11, Galaxy M36) the iPhone held
a link to Android's server. Its random address rotated, Android saw a new
device, dialled it, verified it as the iPhone and kept it: two Link Layer
connections between the same two radios. Two minutes later the iPhone's write
buffer on the first link stopped draining, and the stall policy tore that link
down. The iPhone kept writing on the second link for ten seconds, until its
redial of the first link's address timed out. CoreBluetooth reports a
cancelled connect as a disconnect, the identifier still mapped to Android, and
the bridge reported Android lost. The core then refused every send to a peer
the iPhone was connected to, for ten minutes, and about 2,500 queued messages
per phone failed as the outbox filled.

No reconnect could end it. The Link Layer does not complete a second
connection between two devices that already have one (Core 5.4, Vol 6 Part B,
1.1.1), and every dial the iPhone made came from the address Android's second
link held. Seven of Android's advertising addresses in a row timed out, and
the dials that timed out before the stall show the same thing. The one that
completed followed the iPhone's own address rotation.

## Decision

One usable link per peer pair, and no peer is lost while it has one.

- Android closes a client link that verifies as a peer it already holds a
  usable link to: a client link of its own, or the peer's link to its server
  with notifications subscribed. Before, only a second client link counted.
- Android's periodic refresh restarts the advertisement only while no link is
  up. Membership changes and disconnects still restart it.
- iOS never reports a peer lost, and never redials, when the identifier that
  dropped belongs to a peer connected and verified on another identifier or
  subscribed to its server from one. The dropped identifier is retired.

## Consequences

A second link no longer forms between two phones, so a link torn down by the
stall policy is redialled at an address pair the radios accept. A peer stays a
neighbour, and sendable, as long as any link to it is up.

The partial versions each leave a failure. The iOS rule alone keeps the
second link, so whenever the iPhone's link dies while Android's is up, the
reconnect waits for an address rotation, up to fifteen minutes. The Android
rules alone leave the false loss on the iPhone whenever two links form anyway
(a peer the bridge does not control, such as a Mac, rotating its address).

The write-buffer stall itself is not explained. It has been seen only while
two links existed (runs 2, 3 and 5; never in run 4, which had one); the stall
teardown logs the peer's other live links so the next run can confirm or
refute that.

## What would undo this

Allowing a client link beside a subscribed server link again, or restarting
the advertisement on a timer while linked. Either brings back the address
churn that makes a second link form.
