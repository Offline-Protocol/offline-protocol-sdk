# Embedded footprint harness

Answers one question with a number: **what does `offline-protocol-core` cost
on a Cortex-M33?**

```
rustup target add thumbv8m.main-none-eabihf
rustup component add llvm-tools-preview
./measure.sh
```

## What it measures

Two firmware images are linked for `thumbv8m.main-none-eabihf` under the
release profile (`opt-level = "z"`, LTO, `panic = "abort"`), which is the
shape a shipping artifact has:

- `baseline` is the `cortex-m-rt` vector table, the allocator and the panic
  handler, with no protocol.
- `protocol` is the same plus the receiving half of the protocol: decode a
  JSON frame, re-encode it as binary wire v1, decode that, re-encode as JSON,
  parse an address and verify it is canonically spelled, and run the
  identifier policy.

The reported figure is the **delta**, so it excludes the runtime cost that any
firmware pays whether or not it speaks this protocol.

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

Static RAM excludes the heap. Both binaries provision 16 KiB so it cancels in
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
