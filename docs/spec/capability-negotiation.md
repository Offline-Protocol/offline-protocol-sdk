# Capability negotiation

## Where capabilities are advertised

Peers advertise what they can parse in the key package payload, the body of a
`__MLS_KEY_PKG__` control frame.

| Field | Layer | Gates | Absent means |
|-------|-------|-------|--------------|
| `wire_versions` | Hop-local | Which frame encodings we may emit to this peer | JSON only |
| `env_versions` | End to end | Which `__MLS_ENC__` payload forms we may emit | Legacy JSON envelope only |
| `rich_versions` | End to end | Whether we may seal a `__RICH_V1__` body, and the v2 media envelope | Plain text only, extras dropped |
| `data_versions` | End to end | Whether we may send `__DATA_V1__` document sync frames, and which document encoding they carry. Entry 1 is 1:1 replication; entry 2 additionally means the peer intercepts these frames inside a *group* ciphertext | No replication with that peer |
| `nostr_pubkey` | End to end | Which key metadata is sealed to on the Nostr path | Seal to the publicly computable key |

The key package payload also carries `user_id`, the MLS key package itself, a
**relative** remaining lifetime in milliseconds, and a `session_reset` flag.

The lifetime is relative rather than absolute so the receiver applies it to
their own clock and clock skew does not expire a valid package.

## The universal rules

Three rules apply to every capability in this protocol. They are what make
mixed-fleet deployments safe.

### 1. Parsing is unconditional; emission is gated

A receiver accepts every historical form of everything, regardless of what it
advertised. Capability governs only what a sender **emits**.

An implementation that gates parsing on its own advertisement drops messages
from peers that legitimately believed it capable, for instance after a partial
upgrade, or after a capability was restored from persistence on one side and not
the other.

### 2. Absence means the floor, never an error

An absent capability list decodes to empty, and empty selects the permanent
floor. A legacy peer that has never heard of a field is served the floor form
automatically.

### 3. Downgrade loses the feature, never the confidentiality

Where a capability gates a security-relevant payload, the non-capable path
**drops** the payload rather than sending it in a weaker form. Rich extras
toward a non-capable recipient are dropped, never sent in cleartext.

## Persistence

`env_versions`, `rich_versions` and `data_versions` are **end to end**: they
describe what a recipient parses after an arbitrary number of relay hops. They
MUST persist across restarts and be restored before any queued send flushes.
Otherwise a restart silently downgrades every established peer until the next
key package exchange, and the queued sends that flush at startup take the
downgrade.

For `data_versions` the consequence is quieter than a downgrade and worse: both
sides keep accepting edits to a document neither is replicating, so the symptom
is not a dropped feature but two replicas that disagree, with nothing anywhere
reporting a problem.

`data_versions` entries are read independently and the list is append-only. A
peer advertising `[1]` is not a peer advertising `[1, 2]` with something
missing: it is an implementation that replicates 1:1 and would render a group
replication frame as literal text. Treating the two as one flag either sends
that peer a frame it cannot read or stops replicating with it altogether.

Entry 2 has a second source, because members of a group never exchange key
packages with each other: a group inviter MAY attest it for a member on the
Add commit and in the Welcome, exactly as it attests `rich_versions`. An
attestation opens the *group* gate only, never 1:1 replication, and any
directly received key package from that peer overrides it in both
directions. Absence of an attestation means "no information" and MUST NOT be
read as a downgrade.

`wire_versions` is **hop-local** and deliberately in-memory only. It describes
what the next hop decodes, and it is re-exchanged on connect.

Per-peer capability state is stored separately from the key package cache. The
key package cache is deleted when a session is created; the capabilities must
outlive it.

## Trust boundary

**The three capability lists are not cryptographically bound to the sender.**

They ride in the plaintext key package envelope *alongside* the signed MLS key
package data, not *inside* the signature. A network attacker positioned on the
pre-session bootstrap can:

- **strip** a list, which downgrades to the floor and is harmless,
- **forge** a list onto a legacy peer, which makes us emit frames that peer
  cannot parse, which is a targeted delivery denial of service.

Neither grants a new capability. Such an attacker already controls key package
delivery and could deny service outright by dropping the packet.

**These are performance and feature negotiations. They are never security
controls.** An implementation MUST NOT derive a security decision from them.

### The exception: `nostr_pubkey`

This field rides in the same plaintext envelope but is consumed as a
**destination key**, not as a feature hint. The distinction matters: a wrong
capability costs a fallback, whereas a wrong key here means envelope metadata is
sealed **to whoever supplied it** and is then readable off a public relay,
passively, for as long as the value stands.

It is therefore honoured **only from a signed key package**. The canonical
signing payload covers the whole key package body, and the gate verifies it
against the key the sender's address derives from, so on this prefix an unsigned
frame does not reach dispatch at all.

Stripping it remains possible and downgrades to the bootstrap key. That is a
privacy downgrade, not a disclosure to the attacker, and one they could equally
achieve by dropping the packet.

## Relay capabilities

The relay advertises its own capability set, which is separate from peer
capabilities and arrives in the relay's authentication answer.

Two ordering requirements:

1. The capability set MUST be injected **before** the internet-available
   transition, so the flush that transition triggers already sees it.
2. It MUST be cleared when internet drops.

The set is bounded (64 entries of 128 bytes in the reference implementation).

Relay capability tokens are opaque strings. The one this protocol defines is
`group_delivery_v3`; see [Group protocol](group-protocol.md) for why the version
in the token is load-bearing.

## Group capability attestation

Rich payload capability in a group is established directly, or by inviter
attestation when a member was added by someone else. See
[Encryption envelopes](encryption-envelopes.md#group-sealing-gate).

Attestation feeds **only** the group sealing gate. It MUST NOT feed 1:1 sealing
and MUST NOT feed envelope selection. It is a second-hand claim, adequate for
deciding whether to include optional context in a group message, not adequate
for anything else.

## Adding a capability

1. Decide the layer. Hop-local capabilities are in-memory and re-exchanged;
   end-to-end capabilities persist.
2. Add the field with a default of empty, so legacy peers decode cleanly.
3. Make the receiving path accept the new form **unconditionally**, and ship
   that release first.
4. Only in a later release, start emitting the new form to peers that advertise
   it.

Step 3 before step 4 is not optional. Reversing them means the first peer to
upgrade emits a form no deployed receiver understands.
