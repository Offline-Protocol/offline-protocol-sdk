# 0028. Android peer streams join the LAN and keep the lower-opened stream

**Status:** Accepted

## Context

The engine's peer-stream slot (`wifi_direct` in the FFI) is filled by three
managers that speak one [stream chapter](../spec/stream-framing.md): Android
over a Wi-Fi P2P group, iOS over Network framework on a LAN or AWDL, and the
Python host on a LAN. Only iOS and Python met. Android discovered peers over
Wi-Fi P2P service discovery alone, which an iPhone cannot hear and a host does
not speak, so an Android phone and an iPhone on the same Wi-Fi network never
formed a stream, and ADR 0027 recorded "iOS and Android still do not talk
directly".

The Android listener was already open to that network. It binds every
interface on port 8988 for the whole session, so any device on a shared
network could open streams to it, bounded only by the 16-stream cap.

Android also kept the opposite duplicate-stream rule, "the newer supersedes",
because inside a group only the client dials. On a LAN both ends dial, and
ADR 0027 names what two different rules do there: each end keeps the stream
the other closes, and the pair reconnects forever with no error on either side.

## Decision

1. **Android advertises and browses `_offlineprotocol._tcp` on the Wi-Fi
   network** through `NsdManager`, with the record iOS and Python publish
   (`txtvers=1`, `addr`, no `app`), whenever `wifiDirect.enabled` is set. No
   new switch: iOS does the same under the same flag.
2. **Android keeps the stream the lower address opened, and the newer of two
   such, on every carrier.** One rule, not one per carrier: a pair in one P2P
   group and on one LAN has two dialers on one link table, keyed by address.
   Addresses compare by their UTF-8 bytes, not `String.compareTo`, which
   compares UTF-16 units and orders differently past the BMP.
   `every_peer_stream_manager_keeps_the_same_stream` pins the Kotlin, Swift
   and Python copies together.
3. **Keepalive is 15 seconds idle, 5 between probes, 3 probes**, the iOS and
   Python timers, set with `Os.setsockoptInt` because `java.net.Socket`
   cannot set them.
4. **Inbound streams are bounded as on iOS**: 16 open, at most 12 inbound,
   at most 4 from one remote address, so a listener open to a whole network
   cannot take the slots this device dials with.

## Consequences

- An Android phone reaches an iPhone or a Python host on the same Wi-Fi
  network. With no shared network they still do not meet; that needs Wi-Fi
  Aware on both.
- A group client that is the higher address can no longer supersede its own
  half-open stream. Its reconnect is refused until keepalive ends the stale
  one, about thirty seconds, where "newer supersedes" reconnected at once.
  The lower address's reconnect still supersedes at once.
- An Android device with `wifiDirect.enabled` now announces its address to
  every device on the Wi-Fi network it joins, as an iPhone already does
  (threat model R16). The envelope fields a frame leaves in clear are
  readable on that network, as for the other LAN carriers.
- A mixed-version fleet does not loop: an older Android never dials on a LAN,
  and inside a group there is still one dialer.

## What would undo this

Going back to "newer supersedes" on Android needs every carrier that can
reach an Android device to have a single dialer, and a LAN does not. Taking
the LAN path out again would leave Android and iOS meeting over Bluetooth LE
and the relays only.
