# 0004. Control frames are signature-gated with two documented exemption classes

**Status:** Accepted

## Context

Control frames carry session establishment, connection lifecycle, group
membership, presence, and relay coordination. A forged control frame is worth
far more to an attacker than a forged application message.

Not every control frame can be signed, and the reasons differ.

## Decision

Require an Ed25519 signature over a domain-separated, length-prefixed canonical
payload on every internal prefix, verified against **the key the claimed
sender's address derives from**.

Maintain the exceptions as an **exclusion** list, so a newly added prefix is
gated by default.

Recognize exactly two exemption classes, and keep them separate:

| Class | Members | What authenticates them instead |
|-------|---------|--------------------------------|
| Data plane | `__MLS_ENC__`, `__GROUP_MSG__` | MLS decryption plus the credential-to-wire-sender comparison |
| Relay answers | 6 relay-originated prefixes | **Nothing in this protocol** |

## Consequences

**Good.** Address derivation from the presented key is what makes the gate
meaningful. Signature verification alone proves only that whoever supplied the
public key also supplied a matching signature, which any party can do for any
name.

**Good.** An exclusion list means forgetting to gate a new prefix is a
compile-time absence rather than a silent hole.

**Cost.** The relay-answer exemption is a real hole. Anything able to inject on
the relay ingest path can forge group registration, membership answers, group
info, the group list, and error reports.

## Why the two exemption classes must not be merged

A data-plane frame is authenticated **later**. A relay answer is **not
authenticated at all**. Listing a prefix in both makes the narrow relay
conditions unreachable for it, because the data-plane exclusion is consulted
first.

The relay exemption is narrower than the prefix: it applies only to a frame that
arrived on the internet transport carrying no transport peer identity, which is
the shape a locally synthesized answer has. A peer frame on a mesh transport is
still required to be signed. Merging the lists silently discards those
conditions.

## Why the relay-answer exemption exists at all

The relay answers over its own channel and the bridge synthesizes a message from
that answer. There is no private key anywhere in that path. Requiring a
signature drops every one of them, taking group registration with it, and with
that the sync gate group broadcast rides on.

## What would close it

Moving relay answers off the message plane onto dedicated entry points, the way
the group delivery report already works. See
[ADR 0014](0014-dedicated-ffi-entry-points.md).

## Maintenance hazard

The relay-answer list exists in three places no single compiler sees together:
the core and each native bridge. A prefix present in one copy and absent from
another fails **silently**: the bridge injects the answer unattributed, the gate
declines to exempt it, and the frame is dropped as unsigned with no peer at
fault.

Each copy is pinned against literals in its own language's tests. A test that
recomputes the list from the constant it checks agrees with any edit, which is
exactly the failure mode.
