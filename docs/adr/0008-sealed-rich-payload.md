# 0008. Rich extras travel inside the MLS plaintext or not at all

**Status:** Accepted
**Shipped in:** 0.16.0 (direct messages and groups)

## Context

Rich message extras are quoted-reply previews, media metadata, and forward
attribution. They were carried as outer message fields, which are visible to
every relay and every forwarding hop.

What that exposed:

- **Quoted-reply previews quote another message's content**, so the outer field
  leaked plaintext from a message that was itself encrypted.
- **Media metadata for cloud media includes the encryption key and the
  initialization vector.** A relay holding those plus the ciphertext URL holds
  the media.
- **Forward attribution names the original sender**, which is exactly the
  relationship the relay should not learn.

## Decision

Wrap the extras and the text in a versioned body **before** encryption, so they
travel inside the AEAD boundary:

```
__RICH_V1__ + {text, reply_context?, media_metadata?, forward_info?, content_type?}
```

Toward a recipient that has not advertised support, the extras are **silently
dropped, never sent in cleartext**.

On receipt the sealed body is **authoritative** and the outer copies are wiped
wholesale.

## Consequences

**Good.** The relay sees ciphertext and nothing else for the fields that matter.

**Good.** Sealing the content-type hint closes a restamping attack. Without it a
relay can rewrite the rendering hint in transit, and rewriting it to the
file-chunk type routes the decrypted message into the file-transfer manager
where it is dropped. A sealed file-chunk claim is therefore refused on restore,
mirroring the send boundary.

**Cost.** A downgrade loses the feature. A recipient without the capability gets
plain text with threading intact but no preview, no rich metadata, and no
attribution. That is the correct trade and it is visible to users.

**Cost.** Forwarded cloud media only keeps its keys toward capable recipients.
The cleartext outer copies remain as the legacy fallback with secrets stripped
at the wire boundary.

## Two rules that are easy to get backwards

**Parsing is unconditional; sealing is gated.** A receiver tries to parse
whatever a peer chose to seal, regardless of what it advertised. A parse failure
surfaces the raw text plus a warning rather than dropping an authenticated
message.

**The size cap is enforced at the API boundary, not at seal time.** A message
queued behind session establishment re-makes the seal decision when it flushes.
A seal-time failure there re-queues the message forever. Bounding at the
boundary means every queued blob is already known to seal.

## The group gate is stricter, deliberately

In a group the body is sealed only when **every** other member is known capable.
One unknown member fails the gate closed.

Capability is established directly, or by **inviter attestation**: the Add commit
carries the added member's capability to existing members and the Welcome carries
a capability map to the joiner, so members added by someone else are still
sealable.

Attestation feeds **only** the group sealing gate. Never 1:1 sealing, never
envelope selection. It is a second-hand claim, adequate for deciding whether to
include optional context in a group message and adequate for nothing else.

Groups formed before attestation existed heal through the drop path, which
key-packages the unknown members once; their automatic reply reopens the gate.

## What would undo this

Writing the extras to the outer message "as a fallback so nothing is lost". The
fallback is the leak.

Letting attestation feed the envelope choice, on the reasoning that it is the
same capability. It is the same field and a different trust level.
