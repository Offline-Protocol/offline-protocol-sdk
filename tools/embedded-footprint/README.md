# Embedded footprint harness

Answers two questions with numbers: **what does `offline-protocol-core` cost on
a Cortex-M33, and what does a whole leaf node cost once MLS is added to it?**

```
rustup target add thumbv8m.main-none-eabihf
rustup component add llvm-tools-preview
./measure.sh              # everything
./measure.sh --core-only  # skip the MLS images
```

## What it measures

Firmware images are linked for `thumbv8m.main-none-eabihf` under the
release profile (`opt-level = "z"`, LTO, `panic = "abort"`), which is the
shape a shipping artifact has:

- `baseline` is the `cortex-m-rt` vector table, the allocator and the panic
  handler, with no protocol.
- `protocol` is the same plus the receiving half of the protocol: decode a
  JSON frame, re-encode it as binary wire v1, decode that, re-encode as JSON,
  parse an address and verify it is canonically spelled, and run the
  identifier policy.
- `leaf` is the same plus a whole device: it drives `offline-protocol-leaf`,
  which provisions an identity, mints a key package, handles an inbound frame,
  and seals an answer. It is built twice, against the feature set a real leaf
  needs and against the full `rfc_compliant` set, because the spread between
  them is the part that is actually a choice.

  It measures the shipping crate rather than a hand-written workload against
  mls-rs, and that changed the number. The earlier version called mls-rs
  directly and so linked neither the envelope codec, nor the control-frame
  signing, nor the address derivation: it priced an image nobody could ship.
  The figure below is about 55 KiB larger for that reason and is the honest
  one.

  There used to be a third image, `leaf-min`, pricing application messages with
  the resilience features off. It is gone: `offline-protocol-leaf` requires all
  four mls-rs features, cargo unifies features, and the variant silently began
  measuring the same bytes as `leaf`. A row reporting a number for a
  configuration nobody can build is worse than no row.

The reported figure is the **delta**, so it excludes the runtime cost that any
firmware pays whether or not it speaks this protocol. The leaf images report two
deltas: against `baseline` it is the whole image, which is what answers "does it
fit"; against `protocol` it is what MLS costs on top of what was already
measured.

The workload is the receiving half on purpose. A constrained leaf node does not
mint messages, it answers them, so it never calls the `std`-gated constructors.
That is the same reason `offline-protocol-core` is linked
`--no-default-features` here: this harness is the leaf configuration, not a
reduced version of the mobile one.

## What the number is not

It is **the protocol layer only**. A door lock also needs, and this does not
include:

- Signature verification and payload unsealing (Ed25519, HPKE, an AEAD)
- A radio driver and the vendor stack it sits on
- An RTOS, if the product uses one
- Key storage, whether Secure Vault or otherwise

It is also not the SDK engine. The engine is `std` and `tokio` bound and does
not build for this target at all; a leaf node is not supposed to run it.

For the `leaf` images specifically, two more things are missing and neither is
small. **Heap is the first.** MLS group state is allocated, not static, so
`.bss` barely moves and the working-set figure is simply not in this
measurement; it has to come from running the thing. **Interoperability is the
second.** These images are linked and never executed, and the frame handed to
the device is an ordinary text message rather than a Welcome or a sealed
envelope, so nothing here says the stack can talk to the phone.
`tools/mls-interop` is what answers that, along with the in-process tests in
the leaf crate, and those are the gates that matter more.

## Where the leaf image stands against the 400 KiB gate

ADR 0021 set a 400 KiB gate to decide whether MLS on a leaf node was viable at
all, and the Stage 0 spike cleared it: 391.3 KiB linked, of which 390.2 KiB sat
above the baseline firmware. The shipping image measured here is larger,
because it links the code a device actually runs rather than the MLS calls
alone. The gate did its job, which was to answer a yes-or-no question before
anything was built; it is not a budget the shipping image is being held to, and
the number that matters now is the one this harness prints.

On a 1536 KiB xG24 the shipping image is a little over a quarter of flash, and
the recovery lever below is worth more than the growth if a product ever needs
it back.

Figures quoted in prose, here and in `CHANGELOG.md`, are a snapshot rather than
a contract: nothing pins rustc for this harness, so a compiler upgrade moves
them with nothing announcing it. The table `measure.sh` prints is the source of
truth. The figures standing here were taken on rustc 1.97.1.

About a quarter of the `leaf` image is dead weight that a better crypto provider
would remove: `mls-rs-crypto-rustcrypto` holds all four curves in one
`EcPrivateKey` enum with no feature gating, so P-384 and P-256 arithmetic link
even when only ciphersuite 3 is enabled. That is roughly 111 KiB, measured by
symbol attribution rather than estimated. Enabling a suite is a runtime filter
in that provider, not a compile-time one.

Static RAM excludes the heap. Every binary provisions 16 KiB so it cancels in
the delta, and the allocator is a bump allocator that never frees, which is
acceptable only because these images are linked and measured, never executed.
A real node brings its own allocator (roughly one to two more kilobytes of
flash) and sizes its own heap from its workload.

## Why it is outside the workspace

It has its own `[workspace]` table and its own lockfile. As a member it would
make every `cargo build --workspace` in the repo demand a Cortex-M toolchain,
and its `.cargo/config.toml` (which pins the target and the linker script)
would fight the SDK's.

## Keeping the sample honest

`SAMPLE_JSON` in `src/bin/protocol.rs` is a real message the SDK produced on
the host, and both codecs were confirmed to round-trip it before it was pasted
in. If the wire format changes such that it no longer parses, the workload
silently shrinks to almost nothing and the footprint looks like an improvement.
The guard against that is `.text` collapsing toward the baseline: if the delta
ever drops by an order of magnitude, the sample stopped parsing, it did not
get cheaper.

The `leaf` images have the same failure mode and a stricter guard, because they
need one: their MLS inputs are *deliberately* not valid, so "it stopped parsing"
is not a signal there at all. `measure.sh` counts `mls_rs` symbols in each image
and fails the run below fifty. A footprint that quietly stopped containing the
thing being measured is worse than no footprint, because it reads as progress.
