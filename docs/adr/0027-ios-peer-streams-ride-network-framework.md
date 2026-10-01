# 0027. iOS peer streams ride Network framework and keep the lower-opened stream

**Status:** Accepted

## Context

The engine has one slot for a platform-established stream to one peer, the
one the FFI calls `wifi_direct` ([stream framing](../spec/stream-framing.md)).
On iOS it was filled by MultipeerConnectivity, and that was the one carrier
that did not implement the chapter as written:

- **It was message-oriented.** The chapter describes a byte stream with a
  length before every body. Multipeer delivers whole messages, so the iOS
  framing carried a special case in which each message had to be exactly one
  frame.
- **It spoke to nothing else.** A Multipeer peer is another Multipeer app. It
  cannot reach the Python `PeerStreamManager`, which advertises and dials
  `_offlineprotocol._tcp` on a LAN.
- **It squatted the service type.** Multipeer publishes `offlineprotocol` as
  `_offlineprotocol._tcp` with its own protocol behind it. A Python host
  browsing the same LAN finds that record, with `txtvers` and `addr` in it,
  and dials a socket that does not speak the chapter.
- **It capped a session at eight peers.** The manager kept to seven.

Network framework does what the chapter describes. `NWListener` and
`NWBrowser` publish and find `_offlineprotocol._tcp` with a TXT record, and
`NWConnection` is a TCP stream. With `includePeerToPeer` all three also run
over AWDL, the peer-to-peer Wi-Fi that Multipeer itself runs on, so two
iPhones with no access point still reach each other. It is also the
framework Apple's Wi-Fi Aware support is built on.

Moving there changes one property the mobile policy leaned on: who dials.
In a Multipeer session the browser invites, and the chapter's mobile rule for
two streams to one address, "the newer supersedes", was written for carriers
where both ends do not dial. Over Network framework both ends browse, and both
can dial.

## Decision

1. **The iOS peer-stream manager is Network framework, TCP, with
   `includePeerToPeer`.** Discovery is DNS-SD `_offlineprotocol._tcp` with a
   TXT record whose first entry is `txtvers=1` and which carries `addr`, the
   same record the Python manager publishes. `NSBonjourServices` needs only
   `_offlineprotocol._tcp`.
2. **Of two streams for one address, both ends keep the one the lower
   address opened, and the newer of two such.** This is the Python manager's
   rule. Addresses compare by their UTF-8 bytes. A Rust guard pins the Swift
   and Python copies together
   (`ios_and_python_peer_streams_keep_the_same_stream`).
3. **Both ends dial, the higher address after five seconds.** The lower
   address's stream is the one that will be kept, so it goes first, and the
   common case opens one stream per pair. The higher address still dials,
   which covers discovery that only one side sees.
4. **The link is plain TCP.** The chapter already says link encryption forms
   no part of the trust argument, and Android and Python carry none. Every
   message inside a frame is sealed or signed at the layer that owns it.
5. **The change is a clean cut.** A Multipeer build and a Network-framework
   build do not see each other. The SDK is before 1.0 and the slot started
   carrying traffic recently, so running both carriers for one release was
   not worth doubling the manager.
6. **Wi-Fi Aware is not part of this.** The manager chooses its carrier in
   one function, `makeParameters()`, which is where that work would start.

### Why not "the lower address dials, and the newer stream wins"

That option keeps the old rule and gates the dial instead: dial only a peer
whose advertised `addr` is higher than ours. Between two iPhones it holds one
dialer per pair and needs no change to `PeerStreamLinks`. It was rejected
because it breaks the migration's main gain:

- **Against a Python host it reconnects forever.** Python dials everything it
  discovers and keeps the stream the lower address opened. When the iPhone is
  the lower address, both ends dial, and the iPhone keeps whichever stream
  proved last. About half the time each end keeps the stream the other
  closes, both streams close, and the pair tries again. No error is raised
  on either side. This is the failure the Python manager's rule was written
  to prevent.
- **It needs discovery in both directions.** If only the higher address sees
  the other, nobody dials.
- **It leaves a replayed preamble more room.** Under "newer wins", every
  replayed inbound stream supersedes the real one
  ([R16](../security/threat-model.md#r16-the-identity-assertion-is-static-and-replayable-on-every-carrier)).
  Under the lower-opened rule, a receiver that is the lower address refuses
  it.

Neither option handles a half-open stream better than the other. Both need
TCP keepalive to end a stream whose peer died while idle, so keepalive is not
a cost of the choice. The manager uses the Python manager's timers: 15
seconds idle, 5 between probes, 3 probes.

## Consequences

- An iPhone and a Python host on one LAN find each other and keep one
  stream. Android does not change: its Wi-Fi Direct group has one dialer and
  keeps "newer supersedes", which the chapter allows as local policy. iOS and
  Android still do not talk directly.
- The stream budget is 16 open streams, as on Android, instead of seven.
  Unlike a Wi-Fi Direct group, the listener is open to the whole LAN, so at
  most 12 are inbound and at most 4 come from one remote host, as the Python
  manager bounds its own.
- The first contact between two iPhones may open two streams if the lower
  one's dial takes longer than five seconds. One closes without a report,
  which the chapter's one-stream-per-address rule already covers.
- The higher address's reconnect past a half-open stream waits for keepalive,
  up to about thirty seconds, because the lower side still holds a stream it
  believes live. The lower address's reconnect supersedes its own stale
  stream at once.
- On a shared network a device on the path can read what a frame's envelope
  leaves in clear, which Multipeer's session encryption used to hide.
  Payloads are sealed either way. The threat model records it with the other
  carriers.
- A local-network denial is reported as a diagnostic. Multipeer failed
  silently there.
- When this was accepted, the manager had been run on one iPhone against a
  Python host on a shared network, in both address orders. Two iPhones over
  AWDL with no access point, the case Multipeer covered, had not been run on
  devices. It is owed before a release claims it.

## What would undo this

Wi-Fi Aware on iOS 26 changes the carrier, not this decision: it runs through
the same `NWListener`, `NWBrowser` and `NWConnection`, and would reuse the
rule. Going back to "newer supersedes" on iOS would need every carrier that
reaches an iPhone to have a single dialer, and a Python host does not.
