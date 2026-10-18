# 0029. The mobile libraries are optimised for size and still unwind

**Status:** Accepted

## Context

The workspace defined two release-grade profiles: `release` (opt-level 3,
link-time optimisation, stripped) and `minisize` (the same at opt-level "z",
with `panic = "abort"`). ADR 0018, ADR 0019, threat model R11, the C guide's
K4, the telemetry sink's contract and CLAUDE.md all said the mobile library
shipped under `minisize`, and reasoned from its `panic = "abort"`.

No build ever used it. Every iOS, Android, desktop and wheel build in the
release workflow, the React Native scripts and CI ran `cargo build
--release`. The mobile libraries shipped at opt-level 3 and unwound, and
until 0.30.0 none of them, on any platform, had link-time optimisation
either: the FFI crate's `lib` crate type turned it off for the whole crate.
The documents were wrong in both directions at once.

That left a decision rather than a correction, because the levers do
different things. Size, measured on the macOS arm64 build of the FFI library
at one commit, every row with link-time optimisation:

| opt-level | panic | Shared library | Static library |
|---|---|---|---|
| 3 (`release`) | unwind | 10,932,208 | 23,393,288 |
| "s" (`mobile`) | unwind | 8,538,400 | 20,469,440 |
| "z" | unwind | 6,347,712 | 20,988,624 |
| "z" (`minisize`) | abort | 5,120,848 | 18,613,512 |

Speed, from the Criterion benches on the same machine, median time at
opt-level 3 and the change at "s" and "z":

| Bench | opt-level 3 | "s" | "z" |
|---|---|---|---|
| `mls_seal_dm` | 88.6 µs | +43% | +145% |
| `mls_open_dm` | 101.7 µs | +37% | +115% |
| `send_message` | 4.11 µs | -1% | +46% |
| `process_loop` | 1.43 µs | +8% | +58% |
| `message_serialization/1000` | 0.70 µs | +30% | +96% |
| `ble_fragmentation/1000` | 0.99 µs | +33% | +115% |
| `ble_reassembly/1000` | 3.26 µs | +1% | +83% |

Keeping the crypto and codec crates at opt-level 3 under "z" (30 crates
from `aes-gcm` to `serde_json`) gave back a third of the MLS cost for 3.6%
of size and moved nothing else: under fat link-time optimisation the final
pass runs at the root's level, and the rest of the cost is in the SDK's own
crates.

`panic = "abort"` is the only lever that changes behaviour, and what it
changes is every panic seam the SDK has. Event handlers run under
`catch_unwind`, so a panicking application handler is counted and the
engine's shared state is not poisoned. The telemetry sink dispatch does the
same. A document import that panics inside the CRDT engine is caught and the
document marked unusable instead of the process ending. UniFFI wraps every
foreign call in `catch_unwind` and hands a panic to Swift and Kotlin as an
exception the application can catch and report. Under `panic = "abort"`
each of these becomes a SIGABRT attributed to the host application.

## Decision

1. **The iOS and Android libraries are built with a `mobile` profile:**
   `release` at opt-level "s", unwinding. The release workflow, both React
   Native build scripts and CI's iOS build use `--profile mobile`.
2. **"s", not "z".** "z" takes another quarter off the shared library but
   doubles the cost of MLS sealing and opening and triples small-message
   serialization; the data layer, the heaviest CPU path, has no bench, so its
   cost at "z" would ship unmeasured. The static library the iOS framework
   and the npm package carry is no larger at "s" than at "z".
3. **The desktop libraries and the Python wheels stay on `release`.** The C
   binding's K4 needs unwinding, and a service on a desktop or a Linux box
   prefers speed to a few megabytes.
4. **`minisize` is deleted.** It was never built, and its `panic = "abort"`
   is what the documents wrongly said shipped. A firmware build keeps its own
   size profile in `tools/embedded-footprint`, where aborting is right: a
   bare-metal target has no unwinding.
5. `the_mobile_libraries_are_built_with_the_mobile_profile` and
   `the_mobile_profile_optimises_for_size_and_unwinds` hold the four build
   sites, the desktop builds' `--release`, the profile's opt-level and the
   absence of `panic = "abort"`.

## Consequences

- The containment arguments in ADR 0018 and ADR 0019 still hold, and for the
  reason they gave first: every blob the engine imports came out of a sealed
  record. What changes is the residual. A crafted blob from a space member
  now leaves one document unusable for the process instead of ending it.
- The size `panic = "abort"` would save is left on the table on purpose. A
  host that wants it builds the library with `panic = "abort"` itself, and
  loses each seam above in exchange.
- The numbers in ADR 0024 and the 0.23 changelog labelled `minisize` were
  measured under a profile no build used, without link-time optimisation.
  They are correct as measurements and do not describe a shipped artifact.

## What would undo this

Moving `mobile` to "z" is one word, and worth it if the application size
matters more than a doubled MLS cost: measure the data layer first. Adding
`panic = "abort"` to `mobile`, or pointing a mobile build back at
`--release`, fails the guards above. Reversing the unwinding decision itself
means re-reading every `catch_unwind` in the workspace and deciding, for
each, what the application sees instead.
