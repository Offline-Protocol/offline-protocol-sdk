# Username discovery and the invite payload

This chapter specifies two ways to learn a peer's address without having spoken
to them: a self-certifying **invite payload**, and a non-authoritative
**username directory** published over Nostr.

They differ in what they promise, and the difference is the whole design:

- An **invite** proves the address it carries belongs to the key it carries.
  It is verifiable offline, by anyone, with no network.
- A **discovery record** proves nothing about the *name*. It says only that
  some key asserts a name. The name is arbitrated by a human.

## The invariant that outranks everything else in this chapter

**A username is not an identity, and a resolver MUST NOT treat it as one.**

Anyone may publish any claim to any name. A username therefore resolves to a
*set* of claims, never to an answer, and every conforming implementation MUST
surface the whole set. An implementation that silently selects one claim has
converted a non-authoritative directory into an authoritative-looking one,
which is worse than not implementing this chapter at all: the user believes the
*name* was verified, when only a *key* ever was.

The identity that survives is the address. An implementation SHOULD store the
address a user confirms, not the name they searched for. A name can be
re-claimed by someone else tomorrow; an address is a function of a key.

## Normalization

A username is normalized before it is hashed, signed, or compared:

1. lowercase, using the Unicode **full** lowercase mapping (Default Case
   Conversion, `toLowercase`, the language-insensitive form);
2. then normalize to NFC.

**Full, not simple, and the difference is a wire incompatibility rather than a
detail.** The two mappings disagree wherever one character lowercases to
several: U+0130 LATIN CAPITAL LETTER I WITH DOT ABOVE becomes `i` + U+0307 under
the full mapping and a bare `i` under the simple one. Two implementations that
choose differently derive different tags for the same name and silently never
find each other, which is the failure this whole section exists to prevent. The
mapping must also be language-insensitive: the Turkish tailoring maps `I` to
`ı`, and a directory whose tags depend on the publisher's locale is not a
directory.

The order is normative. Unicode lowercasing can emit a decomposed sequence, so
normalizing first would leave a form that is not NFC, and the operation would
not be idempotent. A non-idempotent normalizer derives one tag when publishing
and a different one when resolving, and the failure is silent: an empty result
for a name that exists.

A conforming implementation MUST refuse a username that, after normalization:

- is empty, or consists only of whitespace,
- exceeds 64 bytes,
- contains a Unicode control (`Cc`) or format (`Cf`) character, or
- has the shape of an address (exactly 44 characters, `off1` prefix, all
  lowercase ASCII alphanumerics).

`Cf` is refused alongside `Cc` because it is the category that actually carries
the rendering attacks: the bidi overrides, the zero-width joiners and the
byte-order mark are all `Cf`, and a name containing one displays as something
other than the bytes that were signed. A screen written against a
"control character" predicate typically tests `Cc` only and lets every one of
them through.

The address-shape refusal is a shape test, not a parse: an address-looking
string with a corrupted checksum is refused too, because it reads exactly as
confusingly in a user interface.

Records arriving on the wire are **parsed, not repaired**. A record whose
username field is not already in normalized form MUST be rejected. Repairing it
would let a record verify against a tag it was never published at.

> Confusable and homograph handling is deliberately out of scope for v1. It is
> the least-defended surface of this chapter. A display layer that warns about
> mixed-script names is a reasonable addition and needs no format change.

## Signing domains

Four domains are live across this protocol, and they MUST be mutually
non-prefixing:

| Domain | Signs |
|--------|-------|
| `offline-ctrl-v1` | Control-plane frames |
| `offline-relay-addr-v1` | The relay address-declaration proof |
| `offline-disc-v1` | Discovery records |
| `offline-invite-v1` | Invite payloads |

Every signature is taken over `domain ‖ Σ(u32be(len) ‖ field_bytes)`. The
**domain itself is not length-prefixed**, so if one domain were a prefix of
another, a signature made under the shorter domain could be replayed as one
made under the longer: an attacker picks a first field whose leading bytes
supply the rest of the longer domain, and the two payloads become identical.
Length-prefixing the fields does not prevent this, because the collision occurs
before the first length prefix.

## The invite payload

### Structure

```
InviteV1 {
  v:        u8         // 1
  address:  string     // bech32m off1…
  pubkey:   [u8; 32]   // Ed25519; derive(pubkey) MUST == address
  petname:  string?    // suggested display name, <= 64 bytes
  sig:      [u8; 64]?  // optional; Ed25519 over the canonical payload
}
```

### Encoding

A versioned compact binary struct, rendered **base64url without padding**:

```
[0]       version = 1
[1]       flags: bit0 = petname present, bit1 = signature present
[2..34]   pubkey (32 bytes)
[34]      address length (u8)
[35..]    address bytes (ASCII)
          petname length (u8) + petname bytes   -- if flags bit0
          signature (64 bytes)                  -- if flags bit1
```

Bech32m is deliberately **not** used, despite the address being bech32m:
BIP-173 limits the checksum's error-detection guarantee to about 90 characters,
and this blob exceeds that. Using it here would be out of spec and
misleadingly reassuring.

A decoder MUST reject an unknown version, an unknown flag bit, a truncated
blob, and trailing bytes. Unknown flag bits select trailing sections, so
ignoring one desynchronizes the parse and surfaces as a corrupt petname rather
than as a version error.

An encoder MUST NOT emit a set petname flag with a zero-length petname: an
absent petname and an empty one are the same state and have one encoding.

### Canonical signed payload

```
"offline-invite-v1" ‖ u32be(len)‖bytes over [v, address, pubkey, petname]
```

in that fixed order, with an absent petname encoded as a zero-length field.

### Verification

A verifier MUST, in order:

1. decode the blob and confirm it is structurally complete;
2. confirm `v == 1` and that no unknown flag bits are set;
3. parse the address in canonical form;
4. confirm `derive_address(pubkey) == address`;
5. if a signature is present, confirm it verifies under `pubkey`.

Any failure means **refuse**, not warn.

### What a signature does and does not defend

It does **not** defend against substitution. An attacker who hands you their
own invite, correctly signed by their own key, is indistinguishable from a
legitimate stranger. No payload format can fix that; only out-of-band context
can.

It defends **relabeling**. Without a signature, anyone can mint an invite
pairing a victim's real, public `{address, pubkey}` with an attacker-chosen
petname, so an invite forwarded through a third party can save Alice's key
under the name "Bob". With one, the petname is bound to the key by its owner.

Sign when the invite may travel without its issuer. A QR code shown phone to
phone is already authenticated by the physical channel, and an application that
prompts the user to confirm or edit the name has made the user the authority
over it, which is what a petname properly is.

### What an invite deliberately omits

**No key package.** An MLS key package's init key is consumed by the first peer
who uses it, and a QR code is static, so pairing them guarantees a collision as
soon as two people scan the same code. Session establishment proceeds over
whatever transport connects, by the ordinary exchange.

**No expiry.** A printed QR code that stops working is a bug. Applications
needing revocable invites have the server-mediated group-invite mechanism.

### Container

This specification defines the blob. Applications own the URI scheme. The
recommended form is `<app-scheme>://connect?c=<blob>`: one opaque parameter, so
it composes with any existing scheme and route.

## The username directory

### Two hops

```
username  --hop 1--> {address, pubkey}  --hop 2--> key package --> MLS session
          discovery record              published KP record
          at tag_disc(username)         at tag_kp(address)
```

Hop 2 is the published key-package mechanism, unchanged. Only hop 1 is defined
here. Everything downstream of the address is already authenticated by key
derivation, which is why a discovery record cannot lie about a key and can lie
only about a name.

### Cardinality: a set forms at the tag

**One record per device.** Each install publishes its own record, signed by its
own identity key, authored by its own Nostr key, naming its own address. All of
a username's devices publish to the same tag, and because addressable
replacement is keyed on `(kind, pubkey, d)` and each device has a different
Nostr key, they coexist as separate events. A resolver queries once and
receives the whole set.

This is not a limitation to design around. No device knows the addresses of its
siblings, so a record shaped as `{username, [devices]}` cannot be produced at
all. Aggregating at the tag reaches the same result with zero coordination.

A username is therefore **1:N always**, even for a single-device user, who is a
set of one.

> Consequence, not solved here: a sender addresses one device. An application
> whose user has three devices must decide whether to fan out or pick one. That
> is an application decision with real cost. The discovery layer's job is to
> stop pretending the mapping is 1:1.

### Tag derivation

```
tag_disc(username) = x-only-secp256k1-pubkey(SHA-256("offline-disc-v1:" ‖ username))
```

The scalar-to-pubkey step mirrors the address routing tag, so the published
value is shaped like any other `#p` pubkey.

The domain separator is what stops the username and address namespaces sharing
a preimage space, so no future third derivation can be aimed across them.

### Record structure

```
DiscoveryRecordV1 {
  v:            u8        // 1
  username:     string    // normalized
  address:      string    // bech32m off1…
  pubkey:       [u8; 32]  // Ed25519; derive(pubkey) MUST == address
  nostr_author: [u8; 32]  // x-only key this record is valid when published under
  issued_at_ms: i64       // signing time
  sig:          [u8; 64]  // Ed25519 by pubkey over the canonical payload
}
```

Canonical signed payload:

```
"offline-disc-v1" ‖ u32be(len)‖bytes over
  [v, username, address, pubkey, nostr_author, issued_at_ms]
```

in that fixed order. `issued_at_ms` is encoded as its 8-byte big-endian two's
complement form, not as a decimal string, so two implementations cannot
disagree about leading zeroes or a sign.

### Publication

- Event kind **30777**, addressable.
- `d` tag = the discovery tag. Deterministic, **not** a random slot id: a
  directory entry is a statement that should be *replaced*, and a deterministic
  `d` is what makes NIP-01 addressable replacement do that work. It is also
  what makes retraction possible at all.
- `p` tag = the discovery tag.
- Content is NIP-44-sealed to `discovery_seal_keypair_for_username(username)`:
  HKDF-SHA256, salt none, IKM the normalized username bytes, info
  `"offline-protocol/nostr/v1/discovery-seal-key/" ‖ counter`.
- `created_at` is the true current time and MUST NOT be jittered into the past.
  Relays keep the newest event per `(kind, pubkey, d)`, so a backdated
  republication is silently dropped and would strand a stale claim.

> Kind 30777 is **unregistered**. Nothing in the NIPs kind registry is assigned
> anywhere in the 30700 to 30800 range as of 2026-08-17. An implementation
> publishing this format should be aware it may one day collide.

### The seal key is public by construction

Anyone who knows the username can derive the seal key and open the record. That
is the design. The record contains only what is public to someone who already
knows the name.

**This key MUST NOT be load-bearing.** It must never back encryption of
anything secret, never back relay authentication, and never inform any
authentication decision. Every authenticity property of a record comes from its
Ed25519 signature and from `derive_address(pubkey) == address`.

What sealing buys, given that anyone entitled to fetch can unseal, is
resistance to *bulk collection*. Publishing in the clear would let a single
`{"kinds":[30777]}` request return a directory of every username on the relay
paired with its address. Sealing costs nothing in reach: fetching requires the
tag, the tag requires the username, and the username reconstructs the key.

### Verification

A resolver MUST check, in order:

1. `v == 1`, and every fixed-length field is the right size;
2. the username matches the queried name exactly;
3. `derive_address(pubkey) == address`;
4. the Ed25519 signature verifies under `pubkey`;
5. `nostr_author` equals the publishing event's author key.

Step 2 is what catches a genuine record for one name copied onto another's tag.

Step 5 is what stops a **re-authored copy**. Because the seal key is publicly
derivable, a third party can unseal a record, re-seal the untouched and
genuinely signed payload under their own Nostr key, and republish it. For a key
package the cost is a dead session. For a directory entry it defeats
retraction: addressable replacement is per-author, so the owner's tombstone
replaces only the owner's own event and never a copy standing under someone
else's key. That would keep a rotated-away or compromised address in the
directory indefinitely.

Every verification failure is **ordinary**, not exceptional. The tag is public,
anyone may publish to it, and a query returns whatever the relay holds. A
resolver drops the record and continues.

### Staleness is advisory

`issued_at_ms` MUST NOT be used to reject a record. A record is not a liveness
signal; the key-package fetch that follows it is. A stale record whose key
packages are gone fails at that fetch, which is the honest place to fail.
Rejecting on age would instead make a peer who has been offline for a month
unreachable *by name* while their key packages sit valid on a relay.

Surface the age. Let the application sort. Let hop 2 arbitrate.

### Retraction

A retraction republishes the same `(kind, pubkey, d)` with a tombstone body and
a fresh `created_at`:

```json
{"v": 1, "retracted": true}
```

and SHOULD additionally emit a NIP-09 deletion request naming the record's
`kind:pubkey:d` coordinate.

Retraction is **best effort**: a relay may honour neither. The tombstone is the
half that works through the replacement rule rather than through a relay's
cooperation, so an implementation MUST publish the tombstone and MAY treat the
deletion as optional.

A resolver MUST treat a tombstone, or an undecodable body, as "no claim from
this author".

### Resolution

Query:

```json
{"#p": ["<tag_disc>"], "kinds": [30777], "limit": 16}
```

Carrying no `since`: a claim is republished only when it changes, so a settled
claimant's record may be arbitrarily old while remaining entirely current.

A resolver:

- accumulates verified claims keyed by the publishing Nostr key, since that is
  what a device is here;
- keeps the newest `issued_at_ms` per author, because a repeat from one author
  is that device republishing;
- MUST return the whole set, in no meaningful order;
- SHOULD report how many records were seen and refused, so "nobody claims this
  name" can be distinguished from "everything claiming it was junk".

An implementation MUST bound both the number of concurrent resolutions and the
number of claims accumulated per resolution. The tag is public and a squatter
can flood it.

### Gating

Publication and resolution are gated together by one switch, which SHOULD
default to **off**. Publishing binds a human-readable name to an address in a
public place, which is materially more disclosure than a key-package record's
"an install with this tag exists": here the mapping *is* the payload.

Publication additionally REQUIRES published key packages. A discovery record
pointing at an address whose key packages are absent resolves and then dead-ends
one hop later, so the two are hard-coupled rather than merely documented.

## Threat model and residuals

**Squatting is possible by construction and is the design, not a defect.**
First-publisher-wins does not exist on a Nostr relay. Every claim is a claim.
The resolver surfaces the set and a human confirms out of band. This is NIP-05's
model verbatim: identify, never verify; follow keys, not names.

This is also why the invite path is permanent rather than transitional: the
out-of-band confirmation is this layer's only trust anchor, so removing the
invite path would remove the directory's security model.

Residuals, stated plainly:

- **Enumeration by guessing.** Anyone who guesses a username can compute its
  tag and learn whether a claim exists, plus its refresh timing. The mitigation
  is that the preimage is a name the guesser already knew, and the payload is
  sealed so a scrape by kind returns nothing.
- **Retraction is best effort.** A relay may ignore both halves. The
  `nostr_author` binding stops a third party keeping a retracted claim alive;
  the owner's own stale copies on unreachable relays persist. Hop 2 arbitrates.
- **Liveness signal.** Publishing is unprompted traffic. Record existence and
  refresh timing are visible to every relay. Default-off is the answer.
- **No revocation of a compromised device's claim by anyone but that device.**
  A compromised key can re-sign its own claim indefinitely. This is inherent to
  a non-authoritative directory.

## The server-backed alternative

An application that already operates unique usernames, a search index and
authentication should use *that* as its directory. It is authoritative and
unsquattable, which this chapter's directory by design is not.

Binding needs no new protocol surface. The client signs a server-issued nonce
under a **domain-separated** payload, and the server verifies the signature and
**re-derives** `derive_address(pubkey) == address` rather than trusting the
presented address. The domain MUST NOT collide with `offline-ctrl-v1`, or a
hostile server harvests a replayable control-frame signature from every client
that ever authenticated.

Cardinality must be 1:N there too.
