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
| 0 | leaf | mls-rs defaults produce a key package the phone **refuses** |
| 1-2 | leaf | derive an `off1` address, mint a key package with the corrections |
| 3-4 | phone | parse it, validate it, check the SDK's leaf identity binding |
| 5 | phone | create the group, commit the Add, merge |
| 6 | leaf | join from the Welcome, with no out-of-band tree data |
| 7-8 | both | application message each way |
| 9 | leaf | process a commit the phone issued |
| 10 | leaf | decrypt again in the new epoch |

The phone is configured the way `crates/offline-protocol-mls/src/group.rs`
configures it. Both library versions are pinned with `=`: an interop result is a
claim about two specific versions and is worthless if either floats.

## The three corrections, and step 0

Getting a leaf's key package accepted took three changes, none of them
signposted by either library. All are on the leaf side.

1. **Shorten the key package lifetime.** mls-rs defaults to a year. OpenMLS
   refuses any leaf node whose total lifetime range exceeds one hour plus three
   months. The default is refused on arrival.
2. **Backdate `not_before`.** OpenMLS tests `not_before < now`, strictly.
   mls-rs's client builder writes `not_before` as exactly the timestamp it is
   handed, without the backdating its own `Lifetime::seconds` helper applies, so
   a package stamped with the current second is refused for being not yet valid.
3. **Pass the timestamp in rather than letting the library read a clock.** This
   is the one with consequences beyond this file. A bare-metal leaf has no
   clock, and mls-rs stamps `not_before = 0` when it cannot read one, with a
   source comment saying the value is there so tests can run. A device that
   ships that way emits a key package whose validity window is in 1970 and is
   refused as expired. **A leaf therefore needs a time source at pairing**, from
   its radio stack, its commissioner, or the pairing exchange itself.

There is also a framing difference, smaller and not a correction to either side:
the SDK puts a bare `KeyPackage` on the wire while mls-rs's convenience API
returns one wrapped in an `MLSMessage`. Both are legal. They have to agree.

**Step 0 is a negative control and is the most important line in the output.**
It builds a leaf with the uncorrected defaults and requires the phone to reject
it. Every one of the three corrections is a default that someone will eventually
tidy back, and a harness that only proves the corrected path works cannot tell
them they broke it. If step 0 ever *passes*, one of the two libraries changed
its lifetime rules, and whether these corrections are still load-bearing has to
be re-derived rather than assumed.

## What this does not prove

It is one process on a host, so it tests the two libraries against each other
and nothing else. It says nothing about the radio, the fragmentation path, flash
persistence across a power cut, or timing on the part. It also does not exercise
the SDK's envelope layer: it stops at the MLS boundary, and how a sealed frame
is wrapped for the wire is `crates/offline-protocol/src/protocol/send.rs`'s
business, tested separately.

It is deliberately not a member of the SDK workspace. It links two independent
MLS implementations at once, and neither belongs in the SDK's dependency graph.
