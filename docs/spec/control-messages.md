# Control messages

## The prefix convention

Protocol-internal messages are ordinary messages whose `content` begins with a
reserved prefix. The prefix names the frame type; the rest of `content` is the
frame body, usually JSON.

**Where base64 appears in a frame body.** It carries MLS bytes, and within the
body only in these places (message metadata is separate; see the reserved keys
in [Message model and wire format](wire-format.md#reserved-metadata-keys)):

| Carrier | Shape |
|---------|-------|
| `__MLS_ENC__` body | base64 of the compact binary envelope, **only** when the compact envelope is negotiated for that recipient; the JSON floor carries `ciphertext` as a JSON byte array instead |
| `__GRP_MLS_MSG__` body | JSON payload whose `ciphertext` field is base64 |
| `__GRP_MLS_COMMIT__` body | JSON payload whose `ciphertext` field is base64 |
| `__GRP_RELAY_BCAST__` body | JSON payload whose `ciphertext` field is base64 |
| `__GRP_MLS_WELCOME__` body | JSON payload whose `welcome_data` field is base64 |
| `__GROUP_MSG__` body | base64 of the MLS ciphertext directly, with no JSON wrapper; a decode failure is treated as legacy plaintext, which is then refused for any group the receiver secures with MLS |
| `__DATA_V1__` body | JSON payload whose `blob` field is base64, on the frame kinds that carry document bytes |

Two consequences are worth stating, because both have been read the wrong way:

- The 1:1 `__MLS_WELCOME__` body is **not** base64. It is a JSON
  `WelcomeMessage` whose MLS bytes are a JSON byte array.
- `__MLS_ENC__` is the only prefix whose body shape depends on negotiation.
  Receivers sniff the byte after the prefix (`{` means the JSON floor), so no
  per-message signalling is needed.

A frame body outside this table is not base64.

This is a deliberately low-tech multiplexing scheme. It costs prefix bytes on
every control frame, and it buys the ability to carry control traffic over any
transport that can carry a message, with no separate channel, no separate
framing, and no separate delivery machinery.

## Reserved prefix registry

Every prefix in this table is reserved. An implementation MUST reject an
application `send` whose content begins with any of them, on every public send
surface. Without that check, application text is a control-frame injection
vector.

### Session establishment

| Prefix | Direction | Body |
|--------|-----------|------|
| `__MLS_KEY_PKG__` | peer to peer | Key package payload, JSON |
| `__MLS_WELCOME__` | peer to peer | JSON `WelcomeMessage`; the MLS Welcome bytes are a JSON byte array inside it, not base64 |
| `__MLS_ENC__` | peer to peer | Encrypted envelope, see [Encryption envelopes](encryption-envelopes.md) |
| `__MLS_CONFIRM_PROBE__` | peer to peer | Session confirmation probe |
| `__MLS_CONFIRM_ACK__` | peer to peer | Session confirmation acknowledgement |

### Prefixes that never appear on the wire

Two prefixes are reserved but travel only **inside** an encrypted envelope. They
are listed in the registry so application content can never impersonate them.

| Prefix | Where it lives | Purpose |
|--------|----------------|---------|
| `__MLS_ENC_CONFIRM__` | Inside an `__MLS_ENC__` envelope | A group-aware decrypt that lets the both-create session owner converge. Consumed on receipt, never surfaced to the application |
| `__RICH_V1__` | Inside the decrypted MLS plaintext | The sealed rich payload body |
| `__DATA_V1__` | Inside the decrypted MLS plaintext | Replicated-document sync frames, negotiated as `data_versions`. See [Document sync frames](#document-sync-frames) |

### Connection lifecycle

| Prefix | Meaning |
|--------|---------|
| `__CONN_REQ__` | Connection request |
| `__CONN_ACC__` | Connection accepted |
| `__CONN_REJ__` | Connection rejected |
| `__CONN_CAN__` | Connection cancelled |

### Group frames originated by peers

| Prefix | Meaning |
|--------|---------|
| `__GRP_MLS_MSG__` | MLS-encrypted group application message |
| `__GRP_MLS_WELCOME__` | Group invite carrying an MLS Welcome |
| `__GRP_MLS_COMMIT__` | Membership change commit |
| `__GRP_MLS_LEAVE__` | Leave notification |
| `__GRP_ROLE_CHG__` | Role change notification |
| `__GRP_RENAME__` | Group rename notification |

### Group frames originated by the relay

| Prefix | Meaning |
|--------|---------|
| `__GROUP_CREATED__` | Group registration confirmed |
| `__GROUP_MSG__` | Relay group fan-out |
| `__GROUP_MEMBER_ADDED__` | Relay-side membership add |
| `__GROUP_MEMBER_REMOVED__` | Relay-side membership remove |
| `__GROUP_INFO__` | Group metadata answer |
| `__USER_GROUPS__` | The user's group list |
| `__GROUP_ERROR__` | Relay-side error report |

### Relay hint frames

| Prefix | Meaning |
|--------|---------|
| `__GRP_RELAY_REG__` | Register the group roster with the relay |
| `__GRP_RELAY_BCAST__` | Ask the relay to fan a group message out |

These two are **self-addressed** frames that the local bridge intercepts and
replaces with relay-native frames. They never reach a peer. Their handling has
two mandatory properties, and both are load-bearing:

1. They MUST be sent with acknowledgement disabled. Because the frame is
   replaced rather than transmitted, no acknowledgement can ever come back. On
   the ordinary acknowledgement ladder an unacknowledgeable frame is
   retransmitted for the full retry budget, each resend costing another full
   relay fan-out, ending in a delivery failure for an identifier the
   application never saw plus a transport-selector penalty for a transport that
   did nothing wrong.
2. They MUST be pinned to the internet transport rather than routed by the
   transport selector. The selector demotes the internet transport by design,
   and some mesh transports swallow a self-addressed frame: Wi-Fi Direct and
   Reticulum enqueue it unconditionally and report success. Bluetooth LE fails
   closed, because self is never a connected peer, but that only helps on a
   BLE-only device: a synchronous refusal is a fallback trigger, so the frame
   reaches one of the others anyway. The caller then believes the broadcast
   succeeded and skips its per-member fallback, delivering to nobody. One such
   transport is enough to lose the frame, which is why the rule is a pin rather
   than a preference.

Retry policy for these frames lives at the application layer instead, with
explicit trackers and bounded attempts.

### Presence and indicators

| Prefix | Meaning |
|--------|---------|
| `__PRESENCE__` | Presence update |
| `__TYPING__` | Typing indicator |
| `__READ_RECEIPT__` | Read receipt |

### Service discovery

Service messages use their own prefix family: `__SVC_DISC_Q__`,
`__SVC_DISC_R__`, `__SVC_REQ__`, `__SVC_RESP__`, plus a generic service message
prefix. They are reserved on the same terms.

They are signature-gated like any control frame, but they are **exempt from the
encryption requirement**, so discovery gossip and the application-supplied
request and response bodies are sent in cleartext. See
[residual risk R9](../security/threat-model.md#r9-service-discovery-and-service-bodies-are-signed-not-encrypted).

## Document sync frames

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

### Shape

The body is a JSON object. Its first field is the schema version and its `k`
field names the kind:

```
__DATA_V1__{"v":1,"k":"vv","reply":false,"partial":false,"docs":{"<doc>":"<base64 version>"}}
__DATA_V1__{"v":1,"k":"delta","doc":"<name>","blob":"<base64>"}
__DATA_V1__{"v":1,"k":"snap","doc":"<name>","blob":"<base64>"}
__DATA_V1__{"v":1,"k":"need_snap","doc":"<name>"}
```

A receiver MUST read `v` before attempting to parse the body, and MUST consume
a frame whose version it does not know without surfacing it. Reversing that
order makes every future frame format indistinguishable from corruption.

`v` covers the document encoding as well as the frame schema. The engine
guarantees only that new code reads old encodings, so an encoding change is a
new version here, and a peer that has not advertised it is not sent one.

### The space is never on the wire

Neither the space nor the peer appears in any frame. On a 1:1 session a
receiver derives the space from the authenticated wire sender, and a sender
from the recipient; the two replicas therefore name the same space
differently, each by the other's address. In a group the space is the group
whose key opened the ciphertext, so both replicas name it identically.

In both cases the space is derived, never declared, and that is what bounds
reach: a peer cannot name a space, so it cannot reach a document shared with
anybody else. Reaching a group's documents requires being able to encrypt
under that group's key, which is membership.

### Group scope

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

Directed frames are still encrypted under the group key rather than a
pairwise session: two members of a group need not have a 1:1 session with
each other, and requiring one would make replication depend on a handshake
that may never happen. Every member is entitled to the contents, so
addressing is a traffic decision, not a confidentiality one.

### The exchange

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

### Every leg ends

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

### Sizes

A space accepts at most 1024 documents on a peer's say-so. Every unfamiliar
name in an offer, and every blob naming an unfamiliar document, becomes
stored state, and nothing else bounds how many names one exchange can carry.
The ceiling applies only to documents a peer names; an application creating
its own is not subject to it.

A frame carries at most 32 KiB of document bytes before base64. The figure is
the mesh ceiling, not the record ceiling: an unnegotiated Bluetooth link caps
one message at roughly 69 KiB, and the remainder is base64 expansion, the
frame's own JSON, the sealed envelope, and the message header.

A document that cannot be caught up inside that budget in any form is reported
rather than sent. Carrying one over the media transfer path is not yet
specified.

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

### Acknowledgement

Sync frames ride the ordinary ladder and are acknowledged like any message.
Every data-layer outcome is terminal: a corrupt blob, an unknown version, a
refused import, or a switched-off layer is acknowledged and dropped, never
deferred. Deferral means "this same ciphertext will succeed once the session
is ready" and nothing else, so using it for anything else spends the sender's
whole retry budget on a frame that can never be accepted.

A frame arriving before the session is ready is not a data-layer outcome: it
defers un-acknowledged like every other sealed body, and is handled when the
decryption queue drains.

### Imports are contained

A blob is judged before the document engine sees it, and blobs in flight are
recorded on disk so one that ends the process cannot end it again on every
retry. See
[ADR 0019](../adr/0019-remote-document-imports-are-contained-not-trusted.md).

## The control-plane signature gate

Control frames are authenticated with an Ed25519 signature over a canonical
payload, verified against the key the claimed sender's address derives from.

### Canonical payload

```
CTRL_SIGN_DOMAIN ||
  u32be(len(sender))    || sender    ||
  u32be(len(id))        || id        ||
  u32be(len(recipient)) || recipient ||
  u32be(len(content))   || content
```

Three properties matter:

- **Domain separation.** The payload opens with a fixed domain constant, so a
  signature produced for this purpose cannot be replayed as a signature for any
  other purpose that shares the identity key.
- **Length prefixing.** Every field is length-prefixed, so no two distinct field
  tuples produce the same byte string. Concatenation without length prefixes is
  forgeable by shifting a delimiter.
- **Big-endian lengths.** Fixed so implementations agree.

The signature and the signer's public key ride in the message metadata.

### Verification

A verifier MUST:

1. Recompute the canonical payload from the received frame.
2. Verify the signature against the public key carried in the metadata.
3. **Derive an address from that public key and check it equals the claimed
   `sender`.**

Step 3 is the step that makes the gate meaningful. Steps 1 and 2 alone prove
only that whoever supplied the public key also supplied a matching signature,
which any party can do for any name. Deriving the address from the presented key
is what binds the signature to the claimed identity, and it is why this protocol
needs no trust-on-first-use store.

### Exemption class 1: the data plane

Two prefixes are exempt because they are authenticated later, by MLS, rather
than by an Ed25519 signature:

| Prefix | Why exempt | What authenticates it instead |
|--------|-----------|-------------------------------|
| `__MLS_ENC__` | 1:1 envelopes are sent through the ordinary send path and never signed outbound | MLS decryption, plus the credential-to-wire-sender comparison |
| `__GROUP_MSG__` | The relay re-emits it per member from only `{group_id, sender, content}`, so the rebuilt frame is structurally unsigned | MLS decryption, plus the credential-to-wire-sender comparison; plaintext naming an MLS-secured group is dropped as spoofing |

The list is maintained as an **exclusion** list rather than an inclusion list, so
a newly added prefix is security-gated by default. An implementation MUST NOT
invert that: an inclusion list means every forgotten prefix is silently
ungated.

Residual, stated plainly: a group with no MLS state accepts unauthenticated
plaintext on `__GROUP_MSG__`. That is identical to the pre-gate behaviour and
unreachable in deployments where every group is MLS-secured.

### Exemption class 2: relay answers

Six prefixes are exempt for a different reason, and the two reasons MUST NOT be
conflated. A data-plane frame is authenticated later. A relay answer is not
authenticated by this protocol at all.

`__GROUP_CREATED__`, `__GROUP_MEMBER_ADDED__`, `__GROUP_MEMBER_REMOVED__`,
`__GROUP_INFO__`, `__USER_GROUPS__`, `__GROUP_ERROR__`.

These are not frames any peer transmitted. The relay answers over its own
channel and the local bridge synthesizes a message from that answer. There is no
private key anywhere in that path, so requiring a signature would drop every one
of them, taking group registration with it, and with that the sync gate that
group broadcast depends on.

Two things protect them, neither of them a signature:

1. The bridge restricts these prefixes to the relay channel, so a mesh peer
   cannot deliver a crafted relay answer through the ordinary message path.
2. The exemption is **narrower than the prefix**: it applies only to a frame
   that arrived on the internet transport carrying no transport peer identity,
   which is the shape a locally synthesized answer has. A peer frame on a mesh
   transport, or one carrying a carrier identity, is still required to be
   signed.

**Residual, stated plainly:** anything able to inject on the relay ingest path
can forge these frames. That is the pre-existing relay-trust surface. Closing it
means moving relay answers off the message plane onto dedicated entry points, the
way the group delivery report already works. See
[Threat model](../security/threat-model.md).

### The two exemption lists must stay disjoint

A prefix in both lists would make the narrow relay conditions unreachable for
it, because the data-plane exclusion is consulted first. Implementations SHOULD
assert disjointness in a test.

### Hand-mirrored lists

The relay-answer exemption list exists in three places that no single compiler
sees together: the protocol core and each native bridge. A prefix present in one
copy and absent from another fails **silently**: the bridge injects the answer
unattributed, the gate declines to exempt it, and the frame is dropped as
unsigned with no peer at fault.

Each copy MUST be pinned against literals in its own language's test suite. A
test that recomputes the list from the constant it is checking agrees with any
edit, which is precisely the failure mode. See
[Bridge contracts](../bridges/README.md).
