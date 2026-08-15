# Identity and addressing

## The identity key

Every install holds one long-term Ed25519 keypair. It is the root of every
identity claim the protocol makes:

- it signs control-plane frames,
- its public key derives the install's address,
- it is the signature key inside the install's MLS credentials.

Nothing in this protocol rotates the leaf signature key or the MLS credential
independently of the identity key. An implementation that adds such a rotation
breaks the leaf identity binding described in
[Group protocol](group-protocol.md) and MUST re-derive the binding rules first.

## Address derivation

An address is a self-certifying name: it is a function of the identity public
key, so a peer can check a claimed address against a presented key with no
directory, no registry, and no trust-on-first-use store.

```
payload  = 0x01 || SHA-256(ed25519_public_key)[0..20]
address  = bech32m(hrp = "off", payload)
```

- The version byte is `0x01`. It is the only version this specification
  defines.
- The hash is truncated to 20 bytes (160 bits).
- The encoding is bech32m (BIP-350), not the original bech32 constant.
- The canonical rendering is lowercase and exactly 44 characters:
  `off` (3) + separator (1) + data (34) + checksum (6).

Example rendering: `off1…` where the elided part is the 34-character data
section plus checksum.

### Parsing rules

A conforming parser MUST reject:

- uppercase input, even though BIP-173 permits an all-uppercase spelling,
- a string whose checksum validates under the original bech32 constant rather
  than bech32m,
- a human-readable part other than `off`,
- a payload whose version byte is not `0x01`,
- a payload whose length is not 21 bytes.

Canonicality is not a property the bech32 libraries hand you. A decoder that
accepts a string and returns a payload has not proved that re-encoding the
payload yields the same string. Implementations MUST verify canonicality by
re-encoding the decoded payload and comparing, or by refusing every
non-canonical form explicitly. Two distinct strings decoding to one address is a
security bug, not a cosmetic one: it splits any set, map, or dedup keyed by the
rendered form.

### Security margins

Truncation to 160 bits buys two different things, and only the first is what the
impersonation resistance rests on:

| Attack | Cost | What it buys the attacker |
|--------|------|---------------------------|
| Second preimage: produce a key deriving to a *specific existing* address | ~2^160 | Impersonation of a chosen peer |
| Collision: produce *two* keys sharing one address, neither fixed in advance | ~2^80 | One entity holding two signing keys indistinguishable at the address layer |

The ~2^80 collision margin is below the ~2^128 a greenfield design would target.
It is a deliberate trade: every mesh frame carries a sender and a recipient
address, and the Bluetooth LE budget is the binding constraint. Widening the
hash is a version bump and a migration, not a patch.

The consequence of the collision margin is worth stating plainly, because it is
the one place the address layer is weaker than the MLS layer above it: an
attacker who finds a collision can equivocate, holding two signing keys that
present as one address, which defeats the "one identity cannot hold two leaves"
property that the group leaf binding otherwise inherits from MLS signature-key
uniqueness.

## Ordering

**Two orderings exist and they are different orders.** The bech32 charset
`qpzry9x8gf2tvdw0s3jn54khce6mua7l` is not monotonic in ASCII: value 4 renders as
`y` (0x79) and value 5 as `9` (0x39). A string comparison would also weigh the
checksum characters, which carry no identity information at all. Hash-byte order
is the identity-bearing comparison; rendered-string order is a comparison of an
encoding.

**Which order applies is fixed per tiebreaker, and the tiebreakers disagree.**

| Tiebreaker | Compares |
|------------|----------|
| Session slot ownership (both-create) | hash bytes, falling back to string order when either identifier does not parse as an address |
| Group leave election | rendered address strings |
| Admin auto-promotion | rendered address strings |
| Fork leader election | rendered address strings |

Both orders are deterministic and total, so each of these converges on its own:
every peer running a given tiebreaker sorts the same way and reaches the same
winner. An implementation MUST use, for each tiebreaker, the order named in this
table.

A change MUST NOT "harmonize" one site onto the other order. That is the move
that breaks convergence: peers that changed and peers that did not would elect
different winners from identical input, with no way to detect the disagreement
locally. Prefer hash-byte order for anything new, because it compares identity
rather than encoding. See
[ADR 0003](../adr/0003-self-certifying-addresses.md#two-orderings-exist-and-a-tiebreaker-must-not-mix-them).

## Session identifiers

A 1:1 MLS session between two parties is named by a deterministic slot
identifier derived from the two addresses:

```
session_id = "session:" || lower || ":" || higher
```

where `lower` and `higher` are the two canonical address renderings ordered by
hash bytes as described above.

Three properties follow, and all three are load-bearing:

1. **Symmetric.** Both parties compute the same identifier without exchanging
   it, so a session can be addressed before either side has state for it.
2. **Public.** The identifier is a function of two public addresses. Anyone can
   compute the slot identifier for any pair. It is therefore not a secret and
   MUST NOT be treated as one.
3. **Bindable.** A receiver can check that an inbound envelope names the slot it
   shares with the claimed sender. An envelope naming any other slot is
   refused before decryption is attempted.

Property 2 is the reason the desync recovery trigger described in
[Session lifecycle](../state-machines/session-lifecycle.md) is unauthenticated,
and the reason acting on it must be harmless rather than trusted.

Implementations that carry non-address identifiers for legacy reasons fall back
to string ordering for those. That fallback exists for compatibility and is not
part of the specification for new deployments.

## Group identifiers

A group is named by an opaque identifier carried in the MLS group context. It is
chosen by the group creator.

The `session:` prefix is a **reserved namespace**. A group Welcome naming a
`session:`-prefixed identifier MUST be refused, and a session Welcome naming a
group identifier MUST be refused. Without that reservation, a group invite could
be aimed at a 1:1 session slot and displace it.

## What an address does not tell you

An address is a name for an identity key. It is not:

- a device identifier (one identity may run on several installs only if the
  identity key is shared, which this protocol neither prevents nor supports as a
  designed feature),
- a username (the mapping from a human-readable name to an address is a
  directory concern, handled outside this specification),
- a routing hint (the mesh discovers routes; the address carries no topology).

The relay and directory layers may key their own state by username. When they
do, an implementation MUST NOT assume the relay's identifier space intersects
the protocol's address space. The group delivery report in
[Group protocol](group-protocol.md) documents the concrete failure that
assumption caused.
