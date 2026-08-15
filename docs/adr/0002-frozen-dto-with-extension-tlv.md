# 0002. The binary encoding uses a frozen positional DTO with an extension TLV

**Status:** Accepted
**Shipped in:** 0.14.0

## Context

The binary encoding uses postcard, which is positional and non-self-describing.
That is where the size win comes from, and it is also the hazard: reordering,
removing, retyping, or inserting a field silently corrupts decoding on peers
running the previous layout. There is no error, only wrong values.

Serializing the domain message type directly compounds the problem. That type
carries defaulting rules, skip-when-absent rules, and validation-on-deserialize
behaviour that a non-self-describing format cannot honour field for field. The
skip-when-absent rules in particular mean the field count varies by content,
which a positional format cannot express.

## Decision

Encode through a separate flat DTO with a **frozen** field order, and convert
back to the domain type through the **validating** constructors.

Evolution is constrained to three rules:

1. Existing fields never change, in order or in type.
2. Additive data goes into a trailing `(tag, bytes)` extension list that old
   decoders read and ignore.
3. Anything that cannot be expressed as an extension entry takes a new magic
   byte and is negotiated.

The numeric enum mappings are frozen on the same terms.

## Consequences

**Good.** The security checks the JSON path enforces (identifier caps, logical
clock clamps) apply identically on the binary path, because both go through the
same constructors.

**Good.** Rich, rarely-present structures ride as embedded JSON blobs, so they
keep evolving through their own additive rules without touching the frozen
surface.

**Cost.** A field added to the domain type does not automatically appear on the
binary wire. Someone must decide, per field, whether it warrants an extension
tag.

## The constraint the first extension tag imposes

Extension tag 1 (the base64 content tail) shipped in the **first** release of
wire v1, so advertising the version implies understanding it. A decoder that
ignored it would reconstruct a truncated content field.

That is only safe because no v1 decoder without tag-1 support ever shipped, and
it fixes the rule for every future tag: **a tag whose absence changes meaning
cannot be added to v1.** It needs a new wire version. Tags may be added to v1
only when ignoring them costs efficiency or optional context.

Tag 2 (quoted-reply context) satisfies that test: skipping it delivers the
message without its reply preview, which is exactly what a legacy JSON receiver
does with an unknown field.

## What would undo this

Adding a field to the DTO "at the natural place" rather than at the end, or
adding an extension tag whose absence changes what the message means.
