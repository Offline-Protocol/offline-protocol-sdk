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

`__DATA_V1__` frames replicate documents. They are specified in their own
chapter, [Document replication](data-sync.md), which covers the frame family,
the exchange and its termination rules, attachment references and the blob
fetch, and what travels over the media path.

The prefix itself is registered above, and is subject to every rule in this
document: it is an internal prefix, it never appears as an outer wire prefix,
and a message body a caller supplies may not begin with it.

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
