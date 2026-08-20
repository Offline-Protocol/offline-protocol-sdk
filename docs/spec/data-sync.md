# Document replication

Replicated documents are the protocol's second application class. Messaging is
synced events; this is synced state, a document any member of a space can edit
while offline, merging deterministically when the replicas meet again.

This chapter specifies the wire contract: the frames, the exchange they form,
what bounds them, and how bytes too large for a frame travel instead. It does
not specify the merge algorithm. Two implementations interoperate here by
carrying the same frames and the same document encoding, both named by the
version this chapter defines, and a conforming implementation is free to reach
that encoding however it likes.

## Invariants

These come first because they survive every refactor of the mechanisms below.

**A space is an MLS scope.** A 1:1 session or a group *is* the space.
Membership, authorization and encryption are the roster that already exists.
There is no second membership system, and therefore no way for two of them to
disagree about who is in the room.

**The space is derived, never declared.** No frame names its space. A receiver
derives it from the authenticated wire sender, or from the group whose key
opened the ciphertext. A peer that cannot name a space cannot reach a document
shared with somebody else.

**Deltas are idempotent and commutative.** At-least-once, unordered,
partition-tolerant delivery is exactly enough, because duplicates and
reordering are absorbed by the merge. Nothing here needs ordering,
exactly-once, or a session stream, and an implementation that adds them has
built machinery this layer does not use.

**Every leg ends.** No answer restarts an exchange. The failure this prevents
has no symptom on either device except traffic that never stops, which is why
it is a MUST for every frame kind added later.

**Blob bytes never enter a document.** A document holds a reference; the bytes
travel the media path. A document is bounded by one sealed record, so a layer
that inlined blobs could not carry the blobs people actually send.

`__DATA_V1__` frames replicate documents. They travel only inside the
decrypted MLS plaintext, never as an outer wire prefix, and only toward peers
whose key package advertised `data_versions`
([capability negotiation](capability-negotiation.md)).

A frame rides one of two scopes, and the frame shape is identical in both:

- **A 1:1 session**, sealed to the peer the space is named after.
- **A group**, sealed once under the group key and carried by the group
  message path. This requires `data_versions` entry 2 from *every* member,
  not just the recipient, for the reason given under
  [Group scope](#group-scope).

## Shape

The body is a JSON object. Its first field is the schema version and its `k`
field names the kind:

```
__DATA_V1__{"v":1,"k":"vv","reply":false,"partial":false,"docs":{"<doc>":"<base64 version>"}}
__DATA_V1__{"v":1,"k":"delta","doc":"<name>","blob":"<base64>"}
__DATA_V1__{"v":1,"k":"snap","doc":"<name>","blob":"<base64>"}
__DATA_V1__{"v":1,"k":"need_snap","doc":"<name>"}
__DATA_V1__{"v":1,"k":"need_blob","hash":"<64 lowercase hex>"}
__DATA_V1__{"v":1,"k":"blob_gone","hash":"<64 lowercase hex>"}
```

`vv`, `delta`, `snap` and `need_snap` are gated on `data_versions` entry 1.
`need_blob` and `blob_gone` are gated on entry 3, and are covered under
[Attachments](#attachments).

Base64 is the standard alphabet with padding. `reply` and `partial` default to
false when absent, so a sender MAY omit them; a receiver MUST treat an absent
field as false rather than as unknown.

A receiver MUST read `v` before attempting to parse the body, and MUST consume
a frame whose version it does not know without surfacing it. Reversing that
order makes every future frame format indistinguishable from corruption.

`v` covers the document encoding as well as the frame schema. The engine
guarantees only that new code reads old encodings, so an encoding change is a
new version here, and a peer that has not advertised it is not sent one.

## The space is never on the wire

Neither the space nor the peer appears in any frame. On a 1:1 session a
receiver derives the space from the authenticated wire sender, and a sender
from the recipient; the two replicas therefore name the same space
differently, each by the other's address. In a group the space is the group
whose key opened the ciphertext, so both replicas name it identically.

In both cases the space is derived, never declared, and that is what bounds
reach: a peer cannot name a space, so it cannot reach a document shared with
anybody else. Reaching a group's documents requires being able to encrypt
under that group's key, which is membership.

## Group scope

A group space replicates over the group send path: one MLS encryption serves
the whole roster, and the frame is delivered per member on the ordinary
ladder.

Three rules make that safe and bounded.

**Every member must advertise entry 2.** An implementation that advertises
only entry 1 replicates 1:1 correctly and does not intercept `__DATA_V1__`
inside a group ciphertext, so it would surface the frame to its user as
literal text. Because one ciphertext reaches the whole roster, a single such
member means no member may be sent a group replication frame. A sender MUST
NOT send one unless every other member is known to speak entry 2.

**A change received from a group MUST NOT be pushed back into it.** The group
ciphertext already reached every member. Re-broadcasting turns one edit into
N² frames and gets worse as the group grows; anti-entropy still closes real
gaps.

**Offers and answers are addressed to one member.** Anti-entropy between two
members is a conversation between two devices. A sender MUST address version
offers, deltas answering them, and snapshots to the member that asked, so the
1.5-round-trip termination rule below holds unchanged. Only a local commit is
sent to the whole roster.

With one exception, which a sender MUST implement: a group has a single
sender ratchet per epoch, so an addressed frame advances the generation every
*other* member has to reach, and a member that never observes one falls out
of MLS's forward-distance window and stops decrypting that sender entirely.
A sender MUST therefore bound how many frames it encrypts for a group without
delivering one to the whole roster, and deliver the next one roster-wide when
that bound is reached. A promoted frame is an ordinary roster-wide delivery
and is subject to the all-members gate above like any other. Where the bound
is reached and the gate is closed, a sender MUST withhold the frame rather
than send it addressed: a roster-wide frame some member renders as text is
the worse outcome, and spending further generations that only one member can
observe strands the rest for messaging as well as replication. Receivers need no special
handling, since every body is either idempotent or answerable with nothing.
See [the group message lifecycle](../state-machines/group-message-lifecycle.md#addressed-frames-still-advance-everyones-ratchet)
for the bound this implementation uses.

Directed frames are still encrypted under the group key rather than a
pairwise session: two members of a group need not have a 1:1 session with
each other, and requiring one would make replication depend on a handshake
that may never happen. Every member is entitled to the contents, so
addressing is a traffic decision, not a confidentiality one.

## The exchange

An offer (`reply: false`) lists every document the sender holds for that peer
with its version. The receiver answers with what the sender is missing,
followed by one offer of its own carrying `reply: true`. An offer marked
`reply` MUST NOT be answered with another offer. Without that rule the
replicas converge correctly and then exchange offers forever.

A document named in an offer that the receiver has never seen is created
locally, and created empty. Whatever created it MUST also ask for its contents
on the same leg, because the peer will not volunteer them again: it has
already named everything it holds. The counter-offer carries that question
when the frame provoked one. When it did not, which is any offer marked
`reply`, the receiver MUST send a targeted offer naming exactly the documents
it created, marked `reply: true` and `partial: true`. A receiver that creates
a document and asks for nothing leaves it empty for as long as the link stays
up, and nothing on either device reports it.

Deletion has no representation in this version. A document deleted locally is
absent from the deleting side's next offer and still present on the peer's, so
the rules above recreate it and refill it: the deletion is undone rather than
propagated. Removing content from both replicas means emptying the document,
whose internal deletions replicate as ordinary changes. A future version that
adds tombstones needs a new `v`, because a peer without them recreates
everything the peer with them deletes.

A document *absent* from an offer is read as one the sender has never seen,
and answered with the whole document. That inference needs the complete list,
so a sender MUST set `partial` on any frame carrying less than everything it
holds: every frame of an offer split across several, and a targeted offer
about a single document. A receiver MUST NOT draw the inference from a frame
marked `partial`. Without the flag, a peer holding more documents than one
frame carries is sent the entire space, in full, on every exchange, while
perfectly in sync.

## Every leg ends

Every chain of answers is finite, and no answer restarts an exchange. This is
a MUST for any frame kind added later, because the failure it prevents has no
symptom on either device except traffic that never stops.

| Inbound | Answer |
|---------|--------|
| Offer (`reply: false`) | Catch-up for each stale document, then one `reply: true` offer |
| Offer (`reply: true`) | Catch-up for each stale document, plus one targeted offer naming any document this frame caused the receiver to create |
| `delta` that applies, is already held, or is unreadable | Nothing |
| `delta` held behind a missing predecessor | One targeted offer (`reply: true`, `partial: true`) for that document |
| `delta` needing trimmed history | `need_snap` for that document |
| `snap` | Nothing, in every outcome |
| `need_snap` | One `snap`, and only for a document already held |

A receiver MAY also emit a `delta` of its own local changes when a frame
arrives for a document it holds unflushed edits to, and this does not break
the property: those changes were owed to the peer before the frame arrived, so
flushing them is not an answer and cannot recur.

A receiver MAY also emit one version offer when that flush failed and the
frame's change then applied. The failure can leave the local edit pending, in
which case the import folds it into the imported change and suppresses the
pair as an echo, so the offer is what tells the peer to ask. It cannot recur
either: it costs a storage failure that recovered inside one frame, since a
failure still in force fails the import too and no offer is sent.

The one chain longer than a single hop is that targeted offer: it draws
catch-up and nothing further. It terminates because it names only documents
the peer has itself just offered, so the peer creates nothing from it and has
nothing to ask for in turn.

`need_snap` says that no run of changes can close the gap, because the
receiver compacted away the history the changes are built on. Answering such
a refusal with a version offer instead does not terminate: the sender
recomputes changes since the receiver's version, which is the same refused
delta, and the two trade it indefinitely.

A `snap` answer closes the gap when the sender holds everything the receiver
kept. When the two replicas forked below a point the receiver compacted away,
it does not and cannot: the ancestors were deleted on the receiving side, so
no frame carries them back. That import is refused and reported, and the
replicas stay apart. See
[ADR 0019](../adr/0019-remote-document-imports-are-contained-not-trusted.md).

## Sizes

A space accepts at most 1024 documents on a peer's say-so. Every unfamiliar
name in an offer, and every blob naming an unfamiliar document, becomes
stored state, and nothing else bounds how many names one exchange can carry.
The ceiling applies only to documents a peer names; an application creating
its own is not subject to it.

A frame carries at most 32 KiB of document bytes before base64. The figure is
the mesh ceiling, not the record ceiling: an unnegotiated Bluetooth link caps
one message at roughly 69 KiB, and the remainder is base64 expansion, the
frame's own JSON, the sealed envelope, and the message header.

A document that does not fit that budget in any form travels the media path
instead, which is the rung above every frame. See
[Documents too large for a frame](#documents-too-large-for-a-frame).

A space whose version list does not fit in one frame costs more than
proportional traffic: every frame of a split offer is answered on its own, so
an offer split into *k* frames draws *k* answers, each itself split when the
answering side holds more than one frame's worth. Both replicas still
converge, and a space of 128 documents or fewer never splits at all.

A receiver MUST NOT trim that cost by scoping its answer to the documents the
inbound frame named. A document the receiver holds that the sender has never
seen is announced only by the receiver's complete list, and an answer scoped
to the names it was sent drops that document silently, leaving a replica that
simply never receives it.

## Attachments

An attachment is a blob a document refers to and does not contain. The
document holds a reference; the bytes travel the media path, which is the
transfer machinery messaging already uses for files.

The failure this shape prevents is a document that has to hold a photo. A
document is bounded by one sealed protocol-state record, so a layer that
inlined blobs would be a layer that could not carry the blobs people actually
send, and it would discover that at commit time with the picture already on
screen.

### The reference

A reference is an ordinary document value. Its canonical form is a map:

```json
{"kind":"attachment","hash":"<64 lowercase hex>","size":<bytes>,"name":"<display name>","mime":"<media type>"}
```

`kind` and `hash` and `size` are required. `name` and `mime` are optional and
MUST be absent rather than null when the writer has none: a reader that has to
tell one spelling from the other has been given two spellings of one value.

`hash` is the SHA-256 of the whole blob, lowercase hex, exactly 64 characters.
It is an address rather than a checksum carried beside one: the same bytes
referenced twice are one attachment, and arriving bytes are accepted only if
they hash to the reference that asked for them.

An implementation MUST reject a reference whose hash is not exactly 64
lowercase hex characters, and MUST NOT case-fold an uppercase one. Two
spellings of one address are two addresses: they would fetch twice, store
twice, and compare unequal while naming identical bytes.

`size` MUST be greater than zero and MUST NOT exceed 9223372036854775807
(2^63 - 1). Zero names no bytes, so a reference carrying it can only ever
produce a fetch that cannot succeed. The upper bound is structural rather than
a policy about how large a blob may be: it is the largest integer this layer
carries without changing its meaning, and a value past it would have to be
clamped, which replicates to the whole space as a number its writer never
wrote.

`name` MUST NOT exceed 256 bytes and `mime` MUST NOT exceed 255 bytes, both
measured on the encoded UTF-8 rather than on characters. They are display
fields that replicate to every member of the space whether or not anybody
fetches the blob, so they are bounded where they are written rather than where
they are shown.

Every bound above is checked in both directions, and the inbound direction is
the one that decides interoperability: a reference arriving inside a peer's
delta is checked before it is read out, and one that fails any check reads as
absent, exactly as an unknown value kind does. A writer that violates a bound
does not hear about it from the other side; its references simply are not
there. Check them where the reference is written.

A reference is one whole value, replaced and never edited in place. That is
what makes concurrent attachment writes safe without any rule anybody has to
remember: two members attaching different blobs to one key resolve the way
every other value resolves, and neither replica ends holding a hash from one
beside a size from another. There is no partial-attachment state to represent,
so this version defines none.

An implementation that does not know this value kind MUST read it as absent.
It MUST NOT render it as text or coerce it to a scalar. This is what lets a
member on an older build sit in a space where attachments are used: they see a
key with nothing in it, rather than a hash presented to a person as content.

`size` describes bytes that are somewhere else, so it bounds nothing locally
and MUST NOT be trusted as an allocation hint. It exists so an application can
decide whether it wants the thing before asking for it over a radio that may
be Bluetooth.

`name` and `mime` are untrusted display text written by whoever wrote the
reference, and they replicate to every member of the space whether or not
anybody fetches the blob. An implementation MUST NOT treat `name` as a path,
and SHOULD render it as inert text: it is written by a peer, and a name
carrying control characters or bidirectional overrides is the cheapest way to
make one thing look like another to a person deciding whether to open it. The
bytes are addressed by `hash` alone, so nothing about carriage depends on
either field.

### How large a blob may be

A blob rides the media path and is bounded by it, not by the document layer:
the transfer layer's own file-size limit is the ceiling. That limit is a local
policy rather than a protocol constant, so an implementation MUST NOT assume
any particular value for a peer's. This one defaults to 100 MB and is
configurable in both directions.

That ceiling is not a recommendation. A fetched blob is delivered to the
application whole, in one event, so its bytes are held in memory on the
receiving side at least once, and on a phone the practical limit is well below
the protocol's. An implementation SHOULD keep attachments to a size it is
willing to materialise in memory, and an application writing references SHOULD
choose that limit deliberately rather than inheriting the transfer maximum.

### The fetch

Fetching is pull. A reference replicates to every member of a space; the bytes
do not move until somebody asks, because a space may reference more bytes than
a phone wants to spend a battery on.

| Inbound | Answer |
|---------|--------|
| `need_blob` | The bytes over the media path, or one `blob_gone`, or nothing |
| `blob_gone` | Nothing |
| Blob bytes over the media path | Nothing |

A holder SHOULD answer `need_blob` with `blob_gone` when it cannot supply the
bytes. A reference outlives the bytes it names, because the reference
replicates and the bytes do not, so a peer holding a reference and no blob is
ordinary rather than broken. Without that answer the asking side cannot tell
that case from a peer that is merely slow.

A requester MUST bound how often it repeats `need_blob` for one hash, and a
holder MUST bound how often it acts on one. Neither frame is idempotent in
cost: each one it acts on spends a whole transfer.

A receiver MUST verify that arriving bytes hash to what it asked for, and MUST
discard them otherwise. Authentication answers who sent bytes, never what they
are, so this check is what makes fetching from an authenticated peer safe
without trusting that peer.

A receiver MUST discard blob bytes it has no outstanding request for, and
SHOULD refuse the transfer as soon as it can identify it rather than at the
end. Discarding only on completion still discards, but the storage and the
battery have already been spent, and the sender paid one frame to spend them.

Where the identifying information rides a particular part of the transfer, a
receiver cannot judge what arrives before it. Such a receiver SHOULD release
what it buffered once it can judge, and MUST NOT let the rest of the transfer
proceed.

An implementation MUST NOT surface a fetched blob as a received file, and MUST
NOT report its transfer to the application from either side. It is document
content moving on a document-layer request: a person who did not start a
download must not be shown one, and a person who did not attach a file must
not be shown one being sent. See
[the data purpose](encryption-envelopes.md#the-data-purpose) for when the rule
takes effect, which is earlier than it looks.

### Scope

Attachment carriage is 1:1 in this version. The media path is a transfer to a
confirmed pairwise session, and two members of a group need not have one with
each other: requiring it would make an attachment depend on a handshake that
may never happen.

References themselves replicate in group spaces like any other value. What a
group member cannot do is fetch the bytes from another member, and an
implementation MUST report that rather than leaving the request outstanding.

A later version that carries blobs in groups needs no new reference format,
only a new capability entry and a way to move bytes under a group key.

## Documents too large for a frame

A document whose catch-up does not fit in 32 KiB in any form travels the media
path as a whole document. This is the rung above `snap`, and the last one.

It is terminal in the same way `snap` is: it provokes no answer, so the
refusals below it can ask freely.

It is gated on `data_versions` entry 3, it is 1:1 only, and the arriving bytes
go through the same import containment every remote blob goes through. The
road the bytes travelled says nothing about what they are: this is still an
import from a peer who is authenticated and may still be wrong.

It differs from an attachment transfer in one rule, and it is the rule a
reader is most likely to carry over by mistake. A snapshot is **unsolicited by
design**: it answers a version exchange rather than a fetch, so there is no
outstanding request to check it against, and the requirement that a receiver
discard bytes it did not ask for does not apply to it. Applying it would
disable this rung entirely, because nothing ever asks for a snapshot by name.

What stands in for that check is the size bound. A sender MUST NOT carry a
document larger than one sealed protocol-state record can hold, and a receiver
MUST refuse one that arrives anyway rather than buffering it to completion.
The receiver could not persist it even if every byte arrived, so the transfer
would otherwise spend a long time to fail at the end, and an unsolicited
transfer that no request bounds is exactly the one whose size has to.

An implementation MUST report a document it cannot replicate rather than
discarding it silently. The two replicas will not converge, both sides keep
accepting edits, and nothing else about that state looks like a problem: a
line in a log is not a report.

## What travels the media path

Blob bytes and oversized documents are carried by the media transfer path
rather than by frames, and are marked as belonging to this layer inside the
sealed chunk-0 plaintext ([encryption envelopes](encryption-envelopes.md)).

Three rules govern the mark.

**It is not application-settable.** An application asking to send a file MUST
NOT be able to produce one. A caller that could would be able to feed bytes of
its choosing to a peer's document engine while that peer's user saw nothing
arrive.

**It ships under an envelope version that covers it.** The plaintext fields
are positional, so a receiver that does not know the field cannot skip it and
would read its length as file content. The version is what makes such a
receiver refuse the chunk cleanly instead.

**It is only ever sent to a peer that advertised entry 3.** A peer without it
routes the transfer to its user, which for a document snapshot means handing
somebody a CRDT encoding as a downloaded file.

The mark names what the bytes are for and, for a document, which document. It
does not name the space, for the reason every frame here does not: the space
is the authenticated sender.

## Conformance vectors

Frozen wire vectors for everything above live at
`crates/offline-protocol/tests/data/data-sync-v1.vectors.json`: one entry per
frame kind, the canonical attachment reference in both its forms, the data
purpose, the hash and digest derivations, and the default-parsing cases.

They are the conformance surface for this chapter. An implementation that
produces those strings from those inputs, and reads those strings back into
those values, interoperates on this layer.

They were computed independently of the implementation they pin, so a mismatch
is evidence about the code rather than two copies of one mistake agreeing with
each other. A vector that fails means the wire format changed: that takes a new
frame version and a new negotiated entry, never an edit to the expected value.
Every shipped install still speaks the old one.

## Acknowledgement

Sync frames ride the ordinary ladder and are acknowledged like any message.
Every data-layer outcome is terminal: a corrupt blob, an unknown version, a
refused import, or a switched-off layer is acknowledged and dropped, never
deferred. Deferral means "this same ciphertext will succeed once the session
is ready" and nothing else, so using it for anything else spends the sender's
whole retry budget on a frame that can never be accepted.

A frame arriving before the session is ready is not a data-layer outcome: it
defers un-acknowledged like every other sealed body, and is handled when the
decryption queue drains.

## Imports are contained

A blob is judged before the document engine sees it, and blobs in flight are
recorded on disk so one that ends the process cannot end it again on every
retry. See
[ADR 0019](../adr/0019-remote-document-imports-are-contained-not-trusted.md).
