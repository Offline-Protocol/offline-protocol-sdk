# Peer-stream framing

## What this chapter is for

A peer stream is a byte stream the platform established to exactly one other
device: a Wi-Fi Direct group socket on Android, a Multipeer session on iOS, a
TCP connection over a LAN or over a routed mesh on a host. To the protocol
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
already message-oriented, such as a Multipeer session, delivers whole messages
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
implementation chooses its own. The mobile managers use ten seconds and, on
Android, sixteen open sockets; the Python manager's choices are in its bridge
rules. Which of two streams for one address to keep is policy too, and the
implementations differ for a reason: the Python manager keeps the stream
opened by the lower address, because two hosts that each list the other dial
at once and must agree without talking, while the mobile managers keep the
newer, because on a phone the duplicate is almost always the same peer
reconnecting past a half-open stream, and neither Wi-Fi Direct nor Multipeer
has both ends dial.

## Finding a peer on a LAN

Nothing in this chapter requires discovery: a stream opened to a configured
host and port is complete as specified. Where a LAN offers DNS-SD
(RFC 6763), an implementation that advertises MUST use the service type
`_offlineprotocol._tcp` (fifteen characters, the maximum a service label
allows) and a TXT record whose first entry is `txtvers=1` and which carries
`addr=<off1…>`, the advertiser's canonical address.

A framework that publishes DNS-SD on the implementation's behalf is bound by
the same rule. A Multipeer advertiser MUST use the service type
`offlineprotocol`, which the framework publishes as `_offlineprotocol._tcp`
and which fits its fifteen-character limit, and SHOULD carry `addr` in the
discovery dictionary it advertises. Multipeer publishes the type over both
TCP and UDP, so an app's `NSBonjourServices` must list
`_offlineprotocol._tcp` and `_offlineprotocol._udp`, or iOS local-network
privacy blocks discovery without an error. The iOS manager once advertised
`offline-proto`, which no documented entry named.

The `addr` entry is a hint the preamble proves. It tells a browser which
device it is about to connect to, so the derived address of the preamble can
be compared to it (step four above) and a device advertising an address it
does not hold is refused at the first frame. An implementation MUST NOT
announce a peer from a discovery record alone, and MUST NOT route to an
address it has only seen in one. The instance name is implementation chosen
and carries no meaning; an implementation SHOULD NOT put the address there,
since the TXT entry already carries it and one copy is one place to get it
wrong.

What a device advertises on a LAN is visible to every device on it. That is
the same exposure as a Bluetooth LE advertisement, and the same answer: the
address is public by design, and what it does not reveal (who the human is,
who they talk to) is protected above this layer.

## Position among the transports

The engine has one slot for a platform-established peer stream, and it is the
one the FFI calls `wifi_direct`: `wifi_direct_peer_connected(peer_id)` is
where a verified preamble's address is announced, `wifi_direct_message_received(sender_id, data)`
is where a message body is handed upward attributed to it,
`wifi_direct_get_next_message()` is where outbound bodies are drained,
addressed by `recipient_id`, and `wifi_direct_peer_disconnected(peer_id)`
reports the end of an announced stream. The name is debt, recorded rather
than paid: a Wi-Fi Direct group socket, a Multipeer session, a LAN socket
and a routed-mesh socket are the same thing to the engine, a stream to one
peer whose identity the platform proved, and renaming the slot is a separate
breaking change with an alias period.

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

A vector that fails means the framing moved, which needs a versioned preamble
and a new file. Editing an expected value to make a test pass converts a
caught break into a shipped one.
