# MLS interop harness

Answers one question with a pass or a failure: **does the leaf's MLS stack talk
to the phone's?**

```
cargo run --release
```

## Why this exists

`tools/embedded-footprint` measures whether a leaf node's stack *fits*. Fitting
is not the interesting risk. A device that links 390 KiB of MLS and cannot
decrypt a single frame the phone sends has passed the footprint gate and failed
at the only thing that matters.

The two stacks are different implementations by different authors: OpenMLS on
the phone, mls-rs on the device. Both claim RFC 9420 conformance and both run
the MLS working group's test vectors, which is evidence but not proof about
*this* pairing, with *this* ciphersuite, carrying *this* SDK's credentials. No
published result covers that pair. This harness is that result.

## What it runs

A never-committing member's whole life, in one process:

| Step | Who | What |
|---|---|---|
| 0.1-0.2 | leaf | each corrected default, restored, produces a key package the phone **refuses** |
| 0.3 | leaf | a year-long lifetime is **accepted**, pinning a cap OpenMLS declares and never applies |
| 1-2 | leaf | derive an `off1` address, mint a key package with the corrections |
| 3-4 | phone | parse it, validate it, re-derive the address from the key package it received |
| 5 | phone | create the group, commit the Add, merge |
| 6 | leaf | join from the Welcome, with no out-of-band tree data |
| 7-8 | both | application message each way |
| 9 | leaf | process a commit the phone issued |
| 10 | leaf | decrypt again in the new epoch |

The phone is configured the way `crates/offline-protocol-mls/src/group.rs`
configures it. Both library versions are pinned with `=`: an interop result is a
claim about two specific versions and is worthless if either floats.

## The corrections, and step 0

Getting a leaf's key package accepted took changes neither library signposts.
Both are on the leaf side.

1. **Backdate `not_before`.** OpenMLS tests `not_before < now`, strictly.
   mls-rs's client builder writes `not_before` as exactly the timestamp it is
   handed, without the backdating its own `Lifetime::seconds` helper applies, so
   a package stamped with the current second is refused for being not yet valid.
2. **Pass the timestamp in rather than letting the library read a clock.** This
   is the one with consequences beyond this file. A bare-metal leaf has no
   clock, and mls-rs stamps `not_before = 0` when it cannot read one, with a
   source comment saying the value is there so tests can run. A device that
   ships that way emits a key package whose validity window is in 1970 and is
   refused as expired. **A leaf therefore needs a time source at pairing**, from
   its radio stack, its commissioner, or the pairing exchange itself.

There is also a framing difference, smaller and not a correction to either side:
the SDK puts a bare `KeyPackage` on the wire while mls-rs's convenience API
returns one wrapped in an `MLSMessage`. Both are legal. They have to agree.

**The step 0 lines are negative controls, and they are the most important lines
in the output.** Each restores one default and requires the phone to refuse the
result, one at a time rather than all at once. A correction is a default someone
will eventually tidy back, and a harness that only proves the corrected path
works cannot tell them they broke it. If 0.1 or 0.2 ever *passes*, one of the
two libraries changed its validity rules and whether that correction is still
load-bearing has to be re-derived rather than assumed.

### The correction that wasn't, and what it found

Splitting the control is what produced the most useful result in this harness.

A third item used to be listed above: shorten the key package lifetime, because
"OpenMLS refuses any leaf node whose total lifetime range exceeds one hour plus
three months". **That is not true of OpenMLS 0.7.4.** The constant exists
(`MAX_LEAF_NODE_LIFETIME_RANGE_SECONDS`) and so does the predicate
(`Lifetime::has_acceptable_range`), and nothing in the crate calls the
predicate. `KeyPackageIn::validate` checks only `Lifetime::is_valid`, the
`not_before < now < not_after` window, and reports failure as `InvalidLifetime`,
which is the error name that made a year-long lifetime look like it was refused
for its range when it was really refused for its `not_before`. A control that
broke all three defaults at once could not tell those apart. One that breaks
them individually says so on the first run.

Step 0.3 pins the finding from both directions. The 28-day lifetime this harness
uses stays, because RFC 9420 asks an application to define a maximum and it
bounds how long an unused init key is usable, but it is **leaf-side policy, not
an interop requirement**. If 0.3 ever starts failing, OpenMLS wired up its cap
and that changed.

It also says something about the phone rather than the leaf: because no cap is
applied, the SDK admits a key package from any peer with an arbitrarily long
lifetime, which RFC 9420 asks implementations to reject. That is a gap in
`MlsManager::import_key_package`, recorded in ADR 0021 and not fixed from here.

## What this does not prove

It is one process on a host, so it tests the two libraries against each other
and nothing else. It says nothing about the radio, the fragmentation path, flash
persistence across a power cut, or timing on the part. It also does not exercise
the SDK's envelope layer: it stops at the MLS boundary, and how a sealed frame
is wrapped for the wire is `crates/offline-protocol/src/protocol/send.rs`'s
business, tested separately.

It is deliberately not a member of the SDK workspace. It links two independent
MLS implementations at once, and neither belongs in the SDK's dependency graph.
