# Peer-stream framing

## What this chapter is for

A peer stream is a byte stream the platform established to exactly one other
device: a Wi-Fi Direct group socket or a TCP connection over a LAN on
Android, a TCP connection over a LAN or over AWDL (Apple's peer-to-peer Wi-Fi)
on iOS, a TCP connection over a LAN or over a routed mesh on a host, and on
any of them a WebRTC data channel a page dialled from the device's code
([below](#dialling-a-listener-from-a-browser)). To the protocol
engine these are one transport, registered in the slot the FFI names
`wifi_direct` for historical reasons, and this chapter is what makes them one:
it specifies the only two things a stream does not get from its platform for
free.

[Bluetooth LE framing](ble-framing.md) says Wi-Fi Direct needs no framing
chapter because a stream has nothing to fragment. That is still true, and this
chapter fragments nothing. What a stream lacks that GATT supplies is **who the
peer is** (the Identity characteristic) and **where one message ends** (an ATT
write has a boundary; a stream does not). Without the first, no frame can be
attributed and the receiving core refuses all of them, which is why the
mobile Wi-Fi Direct managers dropped every inbound frame until they adopted
this chapter. Without the second, two implementations that agree on every
message still cannot read each other's bytes.

Two layers are specified here:

1. **The preamble.** The first frame in each direction, which proves the
   peer's address.
2. **The message frame.** How a message crosses the stream after that.

What sits above is [the message model and wire format](wire-format.md): a
frame body is one message in a hop-local encoding, and this layer neither
reads nor constrains it. What sits below is the stream itself, which this
chapter does not restate: how the platform discovers, connects, encrypts or
reconnects is its own business, with one caveat about encryption stated in
the invariants.

## Invariants

Five things hold regardless of the platform, and a change that breaks one is
a wire break rather than a refactor:

- **A stream carries one peer.** Its address is proved once, by the preamble,
  and every later frame on that stream belongs to that address. A stream
  whose preamble did not verify carries nothing: no frame from it reaches the
  core, ever, and the stream is closed. There is no accept-but-flag state,
  because what the preamble proves becomes the transport-verified peer id
  the receiving core matches `Message.sender` against.
- **The first frame in each direction is the identity assertion.** Each side
  sends its own without waiting for the other's, so a peer that never speaks
  cannot hold the stream half-open on purpose, and each side verifies what it
  receives before announcing the peer. A frame that arrives before the
  assertion closes the stream.
- **Every frame is length-prefixed.** A `u32` big-endian length, then that
  many bytes. There is no path on this transport that carries a bare message,
  and no frame carries two. A length of zero or a length above the ceiling
  closes the stream before any byte of the body is read.
- **The framing is not authenticated.** The prefix comes from whoever holds
  the other end of the stream. The preamble authenticates the peer, and the
  message inside each later frame authenticates itself at the layer that owns
  it ([control messages](control-messages.md), [encryption envelopes](encryption-envelopes.md)).
  Every bound in this chapter exists to keep a hostile length from costing
  memory, never to establish trust.
- **Link encryption forms no part of the trust argument.** A platform MAY
  encrypt the stream (TLS on a LAN socket, the session encryption a peer-to-peer
  framework applies, the mutual TLS a routed mesh terminates). That protects
  the hop's confidentiality and nothing this chapter relies on. An
  implementation MUST NOT skip the preamble because the link was encrypted,
  and MUST NOT treat a certificate, a session identifier or a socket address
  as the peer's identity. The address is what the preamble proves, and only
  that.

## The preamble

The body of the first frame in each direction is the identity assertion
specified in [Bluetooth LE framing](ble-framing.md#the-identity-assertion):
`public_key(32) ‖ signature(64) ‖ signed_data`, the same bytes a Bluetooth LE
peripheral serves from its Identity characteristic, built by the same
implementation and verified by the same one. Nothing about it is stream
specific, and that is the point: a device that can pair over one carrier can
pair over the other with the same key and the same code.

A receiver runs the four verification steps of that section. Steps one to
three (parse, verify the signature under the key, derive the address) are
`verify_identity_assertion` and produce the address or a refusal. Step four,
comparing the derived address to a claim, applies where a claim exists:

- If the stream was opened toward a known address (a peer list entry, an
  invite, a discovery record's `addr`, see below), the derived address MUST
  equal it exactly, and a mismatch closes the stream. The comparison is
  exact, never case-folded, for the reason the Bluetooth LE chapter gives.
- If no claim exists (an inbound connection, a peer list entry naming only a
  host and port), the derived address is the peer's id. There is nothing to
  compare, and nothing is lost: the address is self-certifying.

In both cases the address announced to the core is the **derived** one. A
claim is a hint about who to expect, never a name to announce.

`signed_data` carries no meaning here either. A sender MAY leave it empty and
a receiver MUST NOT interpret it, for the reason that chapter states: nothing
domain-separates it, so giving it meaning is a wire break.

The preamble is static and replayable, exactly as on Bluetooth LE. A device
that can open a stream to a receiver can send it a preamble copied from
another peer and have the stream labelled with that peer's address. What it
cannot do is produce the frames that address must sign, so the exposure is a
stream whose holder cannot use it, and a receiver MUST NOT treat a verified
preamble as evidence that the peer is live, recent, or the only holder of
that address.

Each side announces the peer to its core once its own verification of the
peer's preamble succeeds. Whether the peer verified ours is not observable and
not needed: a peer that refused our preamble closes the stream, and the close
is the signal.

A receiver MAY close a stream on which no preamble has arrived within a local
deadline. The deadline is policy, not wire format, and exists so an opened and
silent stream cannot hold a slot forever.

## The message frame

After the preamble, every frame body is exactly one message in the hop-local
encoding the two peers negotiated: the JSON floor, or binary v1 where both
advertised it, as [the wire format](wire-format.md) specifies. It is the same
value a Bluetooth LE receiver holds after reassembly, and it is handed to the
core the same way, attributed to the address the preamble proved.

A frame is one message and a message is one frame. A message MUST NOT span
frames, because the receiver has no header to reassemble by, and a frame MUST
NOT carry two, because the body's encoding does not self-delimit on the JSON
floor.

Position decides what a body is. The first body on a stream is the preamble;
every later body is a message. A message whose bytes happen to parse as an
assertion is still a message, and a second assertion sent after the first is
a message the core will refuse as malformed. No byte in the body announces
its kind, and none is needed.

## Sizing

```
length   u32, big-endian
body     length bytes
```

The ceiling on `length` is the transport message ceiling,
`DEFAULT_MAX_MESSAGE_SIZE`, 1 MiB (1,048,576 bytes), the same bound every
carrier applies to one message. A frame is never larger than the message it
carries plus four bytes, so the ceiling is not a new limit; it is the existing
one, read off the prefix instead of measured after the fact.

The prefix is big-endian because the one shipped implementation of this
framing already was: Android's `DataOutputStream.writeInt` writes network
order, and the Android manager was written against it. A carrier that is
already message-oriented delivers whole messages
and needs no prefix to find a boundary; it still wraps each message as one
frame, so that one receive path serves every carrier and the ceiling is read
off the same four bytes everywhere, and a message whose prefix disagrees
with the rest of it is refused, since there is no stream position to
resynchronise on. Stating the byte order matters because the mistake it
prevents is invisible to a
round-trip test: an implementation that writes the prefix little-endian reads
its own frames perfectly and reads every conforming peer's first frame as a
length in the hundreds of millions, which the ceiling then refuses. The
symptom is a peer that connects and immediately closes, with no error on
either side that names the byte order.

The floor on a message body is one byte. A zero-length body is refused because
no encoding produces an empty message, and a frame that announces one is
either a broken sender or a probe.

The floor on the preamble body is 96 bytes, the assertion's own floor. A
receiver MAY refuse a preamble frame on its prefix alone when the length is
under 96, without reading the body.

The ceiling is inclusive: a body of exactly 1,048,576 bytes is a valid frame.
Two faults in an earlier Android reader are recorded here because both are
invisible until a conforming peer exists, and both are easy to write again. It
refused a length of exactly the ceiling (`length < 1024 * 1024`), so the
largest message a conforming sender may send was dropped. And on a refused
length it did not close the stream: it read the next four bytes, which were
the start of the body it skipped, as the next length, so every frame after the
first refusal was read from the wrong offset. A receiver that refuses a length
and keeps reading has no defined state to resume from.

## What a receiver owes

In order, for every frame:

1. Read exactly four bytes. Refuse `length == 0` and
   `length > DEFAULT_MAX_MESSAGE_SIZE`, and close the stream, before reading
   a byte of the body. A receiver that allocates from the prefix before
   checking it has handed the peer a 4 GiB allocation for four bytes.
2. Read exactly `length` bytes. A stream that ends before they arrive is
   closed with nothing delivered; a partial body is never handed upward.
3. If no preamble has been accepted on this stream, this body is the
   preamble. Verify it as above. On any failure, close the stream and
   announce nothing. On success, announce the derived address to the core as
   a connected peer.
4. Otherwise hand the body to the core attributed to the proved address.
   What the core does with it (decode, verify, route, refuse) is above this
   layer.

On the stream's end, for any reason, the receiver reports the peer lost to
the core if and only if it was announced. A stream that never proved a peer
was never announced, and there is nothing to report.

A receiver holds at most one announced stream per address. When a preamble
proves an address the receiver has already announced, it MUST either refuse
the new stream or close the older one, and in either case the core sees one
announcement and, later, one loss for that address: a stream refused or
superseded while another stream for the same address is live is never
reported as a loss. Which stream to keep is local policy. The count is not,
because a receiver that announces twice and reports the first close tells the
core a reachable peer is gone, and a copied preamble
([R16](../security/threat-model.md#r16-the-identity-assertion-is-static-and-replayable-on-every-carrier))
is enough to make it do so. The engine keys its peer-stream links by address,
so a second announcement is not a second link, and the first loss report
removes the only one.

The bounds this gives a receiver are the ones that matter. Memory per stream
is at most one body in flight, `DEFAULT_MAX_MESSAGE_SIZE + 4` bytes, because a
frame is read whole before the next prefix. The number of streams a receiver
accepts, and the preamble deadline, are local policy, and a conforming
implementation chooses its own. The mobile managers use ten seconds and
sixteen open streams, of which at most twelve are inbound and four come from
one remote address, because a listener on a shared network is open to every
device on it; the Python manager's choices are in its bridge rules.
Which of two streams for one address to keep is policy too, but every
implementation in this repository that dials over a LAN keeps the same one:
the stream opened by the lower address (addresses compared by their UTF-8
bytes), and the newer of two such. Both ends of a pair dial and must agree without talking: a host
dials every peer it lists or discovers, and so do an iPhone and an Android
phone ([ADR 0027](../adr/0027-ios-peer-streams-ride-network-framework.md),
[ADR 0028](../adr/0028-android-peer-streams-join-the-lan.md)). Two ends that
compute the rule differently each keep the stream the other closes and
reconnect forever. The rule costs the higher address an immediate reconnect
past its own half-open stream: the lower end refuses the new one until
keepalive ends the old.

A stream a page dialled from a code (below) is the exception on the
listener's side, because only the page can open one: there is no
simultaneous dial for the lower-address rule to settle. Of two dialled
streams proving one address, the listener keeps the newer. Under the
lower-address rule it would keep the older whenever its own address is the
lower, and a page that reloaded, or came back after a Wi-Fi drop, would be
refused until the dead stream timed out, up to 15 s. Against a stream of
another kind proving the same address, the existing stream stays.

The browser node is the exception, and only meets itself: it keeps whichever
link verified first and refuses every later one for that address
([web bridge W3](../bridges/web.md#w3-the-page-moves-bytes-the-engine-decides-what-they-mean)).
Its carriers do not dial on discovery. A tab carrier opens one link per
remote instance, and a WebRTC link exists only because two people exchanged
codes or a page dialled a listener's code, so two links to one address are a
second tab with the same identity or a code used twice, and the first link
is the one both ends already use. A page that dials a code again closes its
earlier dialled link to that address first: it keeps its first link and the
listener keeps its newest, so a page that kept the old link while the
listener kept the new one would leave each end closing the link the other
kept.
A WebRTC data channel is a peer stream like any other: reliable and ordered,
with message boundaries the framing ignores. The browser cuts outbound bytes
into chunks of at most 16 KiB, a size every browser accepts, and the reader
joins them, so a frame is never limited by a browser's message ceiling.

## Finding a peer on a LAN

Nothing in this chapter requires discovery: a stream opened to a configured
host and port is complete as specified. Where a LAN offers DNS-SD
(RFC 6763), an implementation that advertises MUST use the service type
`_offlineprotocol._tcp` (fifteen characters, the maximum a service label
allows) and a TXT record that carries `txtvers=1`, first where the
advertiser controls the order (RFC 6763 section 6.7), and
`addr=<off1…>`, the advertiser's canonical address. A browser MUST look
entries up by key and MUST NOT depend on their order: Android's `NsdManager`
keeps a record's attributes in a map and does not promise one.

A framework that publishes DNS-SD on the implementation's behalf is bound by
the same rule, and MUST NOT publish `_offlineprotocol._tcp` for a service
that does not speak this chapter: a browser that finds the record dials it.
The iOS manager publishes the record itself through Network framework, so an
app's `NSBonjourServices` must list `_offlineprotocol._tcp`, or iOS
local-network privacy blocks discovery. The iOS manager once used
MultipeerConnectivity, which published the same type in front of its own
protocol, so a host on the same LAN found an iPhone it could not speak to.
The Android manager publishes and browses the same record through
`NsdManager` on the Wi-Fi network it is on (scoped to that network from
Android 13; below it NSD browses every interface), with the same instance
name (a digest of the address), and binds its dials to that network, since a socket
left to the default network goes out over cellular on a Wi-Fi network with
no internet ([ADR 0028](../adr/0028-android-peer-streams-join-the-lan.md)).

The `addr` entry is a hint the preamble proves. It tells a browser which
device it is about to connect to, so the derived address of the preamble can
be compared to it (step four above) and a device advertising an address it
does not hold is refused at the first frame. An implementation MUST NOT
announce a peer from a discovery record alone, and MUST NOT route to an
address it has only seen in one. The instance name is implementation chosen
and carries no meaning; an implementation SHOULD NOT put the address there,
since the TXT entry already carries it and one copy is one place to get it
wrong.

On Android, Wi-Fi Direct also carries the record over Wi-Fi P2P service
discovery when an application lets the SDK form the group
(`wifiDirect.autoAccept`). The record adds one entry for that use: `app`, a
tag of the application id that keeps applications' devices apart. The `addr` entry
keeps its meaning here: the group the record leads to is only a link, and the
stream opened over it proves its peer like any other. The group's name and
passphrase are derived from the application id, which is not a secret, so the
passphrase keeps honest applications apart and admits anyone who knows the id
([threat model R22](../security/threat-model.md#r22-an-android-wi-fi-direct-group-admits-anyone-who-knows-the-application-id)).
The record is broadcast over the air to any device in radio range, the same
exposure as a Bluetooth LE advertisement.

A record that carries a `sid` entry is not a peer hint. It is a service
instance under the [DNS-SD mapping](dns-sd-mapping.md), published under the
subtype `_svc._sub` of this type and listed under the base type as well by
responders that follow RFC 6763 section 7.1. It names the same host and the
same address as the peer record, so a browser that took it as one would open
a second connector to one host per service published there. A browser
looking for peers MUST ignore any record that carries `sid`.

What a device advertises on a LAN is visible to every device on it. That is
the same exposure as a Bluetooth LE advertisement, and the same answer: the
address is public by design, and what it does not reveal (who the human is,
who they talk to) is protected above this layer.

Where two devices share no network, one may bring a network up for the other
on request, with its credentials sent inside their session: the fast link
([ADR 0038](../adr/0038-a-fast-link-is-offered-inside-the-session.md)). Nothing
in this chapter changes over it. There is no discovery record: the joiner
dials the port the offer named, at the address it named (an iPhone) or at the
group owner's own address (an Android, which stays on its own Wi-Fi as well),
with the host's address as its claim (step four above), and the host admits
no other proved address on the network it made.

## Dialling a listener from a browser

A page cannot open a TCP connection, so it cannot reach the listeners above.
A native device that wants pages on its network to reach it opens a WebRTC
Direct listener instead
([ADR 0039](../adr/0039-a-browser-dials-a-native-device-from-a-code.md)), and
shows a code a page dials. The stream this produces is one reliable, ordered
data channel, and nothing above changes on it: the preamble goes first in
each direction, every frame is length-prefixed under the same ceiling, and
the address in the code is a claim the preamble must prove (step four).

Four things hold for every dial:

- **The code is the whole setup.** No server, relay or second code is
  involved, and the listener never sees the page's session description.
- **The page never edits its own local description.** It builds the
  listener's description from the code and nothing else. A dial that
  depends on editing the local ICE username or password breaks wherever the
  browser refuses that edit, as libwebrtc is moving to do.
- **The listener keeps state only for an authenticated check.** Any other
  datagram gets no reply and no allocation.
- **A DTLS certificate is never an identity.** The page checks the
  listener's certificate against the code. The listener does not check the
  page's, since it has nothing to compare it with. Who is on either end is
  what the preamble proves, as the invariants above require.

### The dial code

The code is a URL fragment, placed after the page the application wants
opened:

```
https://<the application's page>#op-direct=1&h=<host>&p=<port>&c=<certhash>&u=<ufrag>&w=<password>&a=<address>
```

| Key | Value |
|-----|-------|
| `op-direct` | `1`, the version. A dialler refuses any other value as unsupported |
| `h` | The listener's IP address: IPv4 dotted decimal, or IPv6 in RFC 5952 text with no brackets and no zone. It MUST be private (RFC 1918 or `fc00::/7`) or link-local (`169.254.0.0/16` or `fe80::/10`), and never a name |
| `p` | The UDP port, decimal with no leading zero, 1 to 65535: one port, one spelling |
| `c` | SHA-256 of the listener's DER certificate, base64url with no padding: exactly 43 characters, the last one's two spare low bits zero, so one hash has one spelling |
| `u` | The listener's ICE username fragment: 4 to 32 characters of `A-Z a-z 0-9`. A listener writes 16 |
| `w` | The listener's ICE password: 22 to 64 characters of `A-Z a-z 0-9`. A listener writes 32, from a cryptographically secure generator |
| `a` | The listener's canonical address (`off1…`), the claim the preamble must prove |

The code is a fragment and not a query string because a browser never sends
a fragment to the server. A query string would put the listener's network
address and ICE password in the log of whatever server serves the page.

The host must be an IP literal on the local network, because a name would be
resolved by the browser and could point the dial anywhere. A public address
would let a code send a page's traffic off the network it is on. Loopback is
refused too: Firefox does not dial it by default, so a code naming it would
work in some browsers and not others, and a page on the device that shows
the code can dial the device's own network address instead.

Every value comes from an alphabet that needs no percent-encoding (the IPv6
colon is allowed in a fragment). So a dialler refuses a value outside its
alphabet, `%` included, and never decodes one. A dialler looks keys up by
name and does not depend on their order; a listener writes `op-direct` first,
so that a person or a scanner can recognise the code. A key a dialler does
not know is ignored, and a repeated key, a missing key or a fragment over
512 characters is refused. The fragment is read as `&`-separated parts,
each `key=value`: a part with no `=`, or an empty part such as a trailing
`&` leaves, refuses the code, and so does a `%` anywhere in it, an unknown
key's value included. A hash is refused unless it is the one canonical
base64url spelling of its 32 bytes (the spare low bits of its last
character are zero), so that one certificate has one code.

An IPv4 code with a 44-character address is about 185 characters. With a
page URL of up to 60 characters, it fits a version 11 QR code at error
correction level M (251 bytes in byte mode).

### The page's side

The page creates an `RTCPeerConnection` with no ICE servers, and one data
channel negotiated on both sides (`negotiated: true, id: 0`, ordered,
reliable). A dial is a connection on the local network or nothing, and an
ICE server would learn only who is dialling whom. The page then:

1. creates an offer and sets it as its local description unchanged;
2. sets this answer as the remote description, with `<mid>` copied from the
   offer:

   ```
   v=0
   o=- 0 0 IN IP4 <h>
   s=-
   t=0 0
   a=ice-lite
   m=application <p> UDP/DTLS/SCTP webrtc-datachannel
   c=IN IP4 <h>
   a=mid:<mid>
   a=ice-options:ice2
   a=ice-ufrag:<u>
   a=ice-pwd:<w>
   a=fingerprint:sha-256 <c as upper-case hex byte pairs joined by colons>
   a=setup:passive
   a=sctp-port:5000
   a=max-message-size:16384
   a=candidate:1467250027 1 UDP 1467250027 <h> <p> typ host
   a=end-of-candidates
   ```

   An IPv6 host writes `IP6` in place of `IP4` on the `o=` and `c=` lines.
   `a=setup:passive` makes the listener the DTLS server, so the page's
   certificate is the one that goes unchecked.

3. waits for the channel to open, then sends its preamble and reads the
   listener's, with `a` as the claim.

Before dialling a code whose address it already has a dialled link to, the
page closes that link (see
[what a receiver owes](#what-a-receiver-owes)).

### The listener's side

A listener has one UDP port, one certificate, one ICE username and password,
and many dials. A dial is keyed by the page's source address and the page's
own username, the second half of the STUN USERNAME. Pages that dial one code
at once are told apart by that key.

1. **Admission.** A datagram that belongs to no dial is admitted only if it
   is a STUN binding request whose USERNAME is `<u>:<the page's username>`,
   and whose MESSAGE-INTEGRITY verifies under `<w>`. Anything else is
   dropped, with no reply. The listener learns the page's address from the
   request's source (a peer-reflexive candidate), and never needs the page's
   candidates, which a browser may hide behind mDNS names. A check is
   routed to a dial by the page's username, never by its source alone, so
   two pages behind one address are never handed each other's dial. Once a
   dial ends, the listener refuses its page's username: a page that missed
   the close (step 5) keeps sending consent checks, and admitting them as a
   new dial would answer them from a connection that never completes, so
   the page would never learn its stream had ended. A page that dials
   again has a new username, because a browser makes new ICE credentials
   for every connection. A check whose transaction id the dial has already
   admitted from another source is dropped, and one it has admitted from
   the same source is answered but does not count as consent (step 4): the
   id is under the check's integrity, so only the page makes a new one, and
   a device that saw a check could otherwise replay it from an address of
   its choosing, or keep a gone page's stream alive.
2. **Answering.** The listener is ICE-lite and controlled. It answers on
   the first pair an admitted check arrived on, without waiting for
   USE-CANDIDATE: it cannot run checks of its own, so that check is all the
   validation it will ever have, and waiting for a nomination delayed DTLS
   by about a second on real Safari. A later nomination moves the pair.
3. **The stream.** The stream is channel 0. A listener closes any other
   channel the page opens. Outbound bytes are cut into chunks of at most
   16 KiB, which every browser accepts. Both readers join chunks, because
   the framing ignores the channel's message boundaries.
4. **Bounds.** These are local policy, as above. The listeners in this
   repository hold at most four dials that have not yet proved an address,
   and drop a dial that has not proved one within 10 s of its first
   admitted check. A proved stream counts against the same caps as any
   inbound stream. A listener ends a stream after 15 s with no admitted
   check, because the page's consent checks are the only liveness signal an
   ICE-lite agent receives.
5. **Ending.** When a listener ends a stream whose channel is open, for any
   reason (its host closed the stream or the listener, refused it at a
   cap, or a newer dial superseded it), it closes the connection with a
   DTLS close_notify, which every browser treats as the end of its link.
   Without it the page learns only as its checks go unanswered, 15 to 20 s
   later, and holds a link it believes is up until then. The host sends
   what the close queues before it closes the socket.

The listeners in this repository cannot bind an IPv4 link-local address:
str0m 0.25 refuses one as a host candidate. A code may still name one, and
an IPv6 link-local address works.

The certificate, username, password and port are fresh each time a
listener opens, and closing it ends its streams. So a code admits dials
only while the listener that wrote it is open.

## Position among the transports

The engine has one slot for a platform-established peer stream, and it is the
one the FFI calls `wifi_direct`: `wifi_direct_peer_connected(peer_id)` is
where a verified preamble's address is announced, `wifi_direct_message_received(sender_id, data)`
is where a message body is handed upward attributed to it,
`wifi_direct_get_next_message()` is where outbound bodies are drained,
addressed by `recipient_id`, and `wifi_direct_peer_disconnected(peer_id)`
reports the end of an announced stream. The name is debt, recorded rather
than paid: a Wi-Fi Direct group socket, an AWDL stream, a LAN socket
and a routed-mesh socket are the same thing to the engine, a stream to one
peer whose identity the platform proved, and renaming the slot is a separate
breaking change with an alias period.

`wifi_direct_status_changed(is_connected)` reports the layer, and a link is
usable only while the layer is up. A platform may announce a stream before it
reports the layer: every listener in this repository accepts before its
manager reports the layer up, and the Python manager starts DNS-SD in between,
about three seconds on a Raspberry Pi. The engine keeps such a link and, when
the layer comes up, announces it to discovery a second time, so such a peer
produces two `neighbor_discovered` events. A bridge must not collapse them:
the second is the one that pushes the key package. The first
announcement's discovery ran against a layer with no usable link, so its key
package push failed, and a failed push is left for the next discovery to
retry. Before the second announcement the layer coming up revealed the link
silently, and a pair whose other push was also lost (on a superseded stream)
formed no session until an application sent something: a lab pairing waited
past 90 seconds.

Every stream carries the same replay exposure as the Bluetooth LE Identity
characteristic, reachable from further away: anything that can open a stream
to a receiver can present a copied preamble. The threat model records it as
[R16](../security/threat-model.md#r16-the-identity-assertion-is-static-and-replayable-on-every-carrier).

The transport selector treats the slot as a bandwidth-weighted link: its
scoring profile weights bandwidth most heavily and does not score energy. A peer stream carries whole messages in one write,
so nothing here interacts with the fragment bounds of the Bluetooth LE chapter
or with the message-count budgets of the relay carriers.

The engine counts the slot as an available carrier only while a stream has
proved a peer; a stream layer that is up with nothing proved is not a
carrier. And its transport queues a message toward an address only while a
stream has proved that address, refusing any other recipient as not
reachable on this carrier, which the selector treats as "try the next one".
Both are what make the invariants above safe to register. A stream layer
that is up but has exchanged no preamble (the mobile managers, before they
adopted this chapter) would otherwise win the selection on bandwidth and hold every direct message
in a queue the platform has no proved stream to write to, and would be
chosen for a file transfer whose chunks it then refuses one by one. For the
same reason a file transfer is pinned to the slot only when a stream has
proved the recipient itself.

## Conformance vectors

`crates/offline-protocol-transport/tests/data/stream-framing-v1.vectors.json`
is generated by `tools/spec-vectors/generate.py` from this chapter. It carries
a preamble frame whose body is RFC 8032 section 7.1 test vector 1 (the same
assertion the identity assertion vectors pin, so a verifier already checked
against those verifies this preamble), one framed message whose body is a
binary v1 frame from the wire vectors, an ordered exchange showing the
preamble first and the message second, a prefix of exactly the ceiling that
a receiver must accept, and refusals: a zero length, a length one over the
ceiling, a preamble one byte under the floor, and a message
frame arriving before any preamble.

The refusals for the length bounds carry only the four-byte prefix
(`carries_body: false`), because a conforming receiver refuses on the prefix
and never reads a body. `carries_body` says what the vector holds, not what a
receiver must read: the under-floor preamble carries its 95 bytes, and a
receiver may still refuse it on the prefix alone. A vector
that carried the body would be pinning bytes the receiver must not have
read.

The Rust reference for the framing and the preamble position is
`PeerStreamReader` in `offline-protocol-transport` (`stream_framing.rs`),
which a host that owns its own sockets feeds each read's bytes and acts on
the events it yields. `tests/stream_framing_vectors.rs` in that crate is the
consumer that checks it against this file, with the one real verifier behind
it. The platform managers implement the same chapter in their own language;
bodies cross the FFI whole, so nothing below the FFI reads a prefix for them.
Each replays this file in its own suite: `PeerStreamFramingTest` on Android,
`PeerStreamFramingTests` on iOS, and `test_peer_stream_manager.py` in Python.

`crates/offline-protocol-webrtc-direct/tests/data/webrtc-direct-dial-v1.vectors.json`,
from the same generator, pins [the dial code](#the-dial-code). It lives in
the crate that implements the code rather than beside the framing vectors,
so a packaged build of that crate carries its own vectors. It carries codes
a dialler must accept (IPv4, IPv6, keys out of order, an unknown key, each
range's edges, a fragment of exactly 512 characters, and the code given as
a bare fragment, as a `#fragment` and inside a whole URL) with the values
it must read and the fragment a listener writes for them, and codes it
must refuse, each breaking one rule: each missing key, each repeated key,
another version, a public, loopback or named host, a host not in its one
RFC 5952 spelling, a port of 0, a hash one character short, a username or
password one character outside its bounds or its alphabet, a
percent-encoded value, a part that is not `key=value` (a trailing `&`, an
empty part, a part with no `=`, an empty code), a `%` in an unknown key's
value, and a fragment over 512 characters. Each refusal
names the rule it breaks. The listener's code in `offline-protocol-webrtc-direct`
(`tests/dial_code_vectors.rs`, through `DialCode::parse` and
`DialCode::to_fragment`) and the page's `parseDirectCode` both replay it,
so the two implementations of the format cannot drift apart unnoticed.

A vector that fails means the framing moved, which needs a versioned preamble
and a new file. Editing an expected value to make a test pass converts a
caught break into a shipped one.
