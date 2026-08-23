# 0023. A control frame states when it was made

**Status:** Accepted

## Context

Every security-gated control frame carries an Ed25519 signature over a
domain-separated canonical payload. Under `offline-ctrl-v1` that payload binds
four fields: the sender, the message id, the recipient and the content. It
states who, to whom, and what.

It states nothing about *when*, and a signature that says nothing about when is
a bearer capability that never expires. A frame recorded off the air verifies
exactly as well on its tenth delivery as on its first, and the verifier has no
way to tell the two apart.

The destructive case is a `__MLS_KEY_PKG__` frame carrying `session_reset`,
which tears down a live session. Anyone who records one holds a repeatable way
to break a pair. Two things bounded it and neither closed it:

- The receive deduplicator remembers a message id for an hour, so a replay is
  refused inside that window and admitted outside it.
- A leaf node remembered the ids of the last few reset frames it acted on, a
  bounded list on a device with a few hundred kilobytes of flash. That denied a
  repeat of a frame it still remembered and left an older capture spendable.

The phone had no equivalent memory at all, and the Nostr resolution path does
not pass the receive loop's deduplicator, so a published record fetched from a
relay reached dispatch with nothing having asked whether it had been seen.

Recorded as [issue 403](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/403).

### What made this awkward to fix

The obvious answers are a monotonic counter or a challenge-response nonce, and
both are incompatible with how this protocol delivers control frames.

A control frame is retransmitted as **frozen signed bytes**. The outbox holds
an entry for `outbox_max_lifetime_ms` (7 days by default) and each probe
refreshes its last-send stamp, so terminal failure moves out to an absolute cap
of four lifetimes, about 28 days. A connection request riding that ladder is
signed once, at the start, and re-sent unchanged for a month. A published key
package is not delivered at all: it is left on a relay to be *found*, for as
long as the package is valid, which is 30 days for one this install minted and
up to the 90 days `MAX_ACCEPTED_KEY_PACKAGE_LIFETIME` admits for one that
arrived from elsewhere.

So a receiver that demands monotonicity refuses frames that are late by design,
and it refuses them at the worst moment: the recovery frame for a broken pair
is itself a reset, so a counter that regresses after a restore-from-backup
deadlocks on exactly the message that would have healed it. A challenge-response
nonce fails harder still, because the verifier of a frame that waited a week was
not reachable when that frame was minted and cannot have issued anything.

## Decision

**The signed payload states when the frame was made, and three separate
mechanisms turn that statement into a closure.** None of them is sufficient
alone, which is why all three are here.

### 1. The payload

`offline-ctrl-v2` covers the four v1 fields plus the frame's timestamp, as
eight big-endian bytes so that an instant has exactly one encoding. The
timestamp already crosses both wire codecs and is not rewritten in flight, so
nothing on the wire grows.

It is a **new domain rather than a new field under the old one**. A signature
does not say which byte string it was made over, so a verifier that appended a
field under the same domain would report a version gap as a signature failure,
which is indistinguishable from a forgery. Separating the domains states the
difference instead of relying on it being detected. The two are mutually
non-prefixing, which the engine pins across every domain in the protocol.

### 2. The window

A v2 signature is checked against the verifier's clock, and the two ends allow
different ages:

| | Past | Future |
|---|--:|--:|
| Phone | 30 days | 48 hours |
| Leaf | 48 hours | 48 hours |

Thirty days is not a comfort margin. It is the smallest number that clears the
path on which this protocol delivers a legitimately old frame under this
payload: the outbox's 28-day absolute cap. A shorter window refuses frames that
are late by design.

A published key package is the other genuinely old frame, and it is not what
sets this number, because such a record is signed under the older payload and
never judged for its age (see below). Were that to change, the window it would
have to clear is not 30 days but the 90 that
`MAX_ACCEPTED_KEY_PACKAGE_LIFETIME` admits from a peer, which is worth knowing
before anyone reads the two numbers as one.

A leaf allows two days because none of that reaches a device. A leaf is paired
with one phone over a direct radio link, is not addressed through a relay, and
has no outbox ladder measured in weeks. What it sees late is a retransmission
over a link that dropped.

The future window is generous on purpose, and the asymmetry is deliberate: a
window too small refuses a real peer whose clock runs fast, and that refusal
looks exactly like an attack, while a window too large only lets an attacker
who already holds a valid signature choose when within two days to spend it,
which mechanism 3 denies separately.

### 3. The ratchet

A window alone is worth nothing, because an attacker replays a capture from
*before* the peer upgraded: the older payload verifies as it always did, and
the entire check is side-stepped by choosing an old enough recording.

So a peer that has once presented a v2 signature is held to it, and its v1
control frames are refused from then on. The record is durable and lives in the
peer's `EncryptionCapableEntry`, **not** in `PeerCapabilities`, which every
fresh key package overwrites wholesale: a ratchet that an ordinary frame can
clear is not a ratchet.

### 4. The spent-reset mark

Inside the window a captured reset is still spendable, once per deduplicator
window, for thirty days. So a reset is acted on only when its signed timestamp
is strictly newer than the last reset acted on from that peer.

A high-water mark rather than a set of spent frame ids: one integer per peer
instead of a list that grows with traffic, and strictly stronger, because it
refuses *every* older reset including ones the verifier has no memory of. It is
recorded **before** the teardown, since the teardown is followed by a fresh
session and a crash in between would leave the frame able to destroy the
replacement too.

It is moved only by a stamp that is inside a signature. On the older payload the
timestamp is attacker-rewritable, and honouring one there would let anyone able
to inject park a peer's mark at `i64::MAX` and permanently deny that peer the
ability to heal a forked session, which is a worse failure than the replay.

## Consequences

### A peer that has proved nothing keeps everything it had

A driven rekey *arrives as* a reset. So a peer whose resets are ignored is a
peer this node can no longer heal a forked session with: post-compromise
security stops arriving and the pair silently stops converging.

Holding legacy peers to a payload they have never shown they can produce would
do exactly that to every install that has not upgraded. The bar is therefore the
ratchet and not the frame. A peer that has proved it signs v2 must use it; a
peer that has not is where it always was, and its exposure closes by itself on
its first freshness-bound frame.

This was found by a pre-existing end-to-end healing test, not by reasoning about
it, which is worth recording: the reasoning that produced the first version of
this rule was clean and wrong.

### A key package escapes the ratchet, with its reset ignored

Without an escape the ratchet is a trap with no exit. A peer whose record of us
was lost signs v1, because it no longer knows we accept v2, and is refused on the
very frame that would have told it, and since a key package is the only frame
that re-teaches capabilities, the state is permanent.

The case is **capability-record loss, not a reinstall.** An address is the hash
of an identity key, so a peer that reinstalls arrives under a different address
and is simply a different peer. What produces a peer at a known address whose
record we no longer hold is eviction under `MAX_PENDING_KEY_PACKAGES` pressure,
a restore that lost the capability category, or a storage failure on it. Getting
this backwards would have made the escape look unnecessary.

So a v1 key package from a held peer is admitted and its `session_reset` is
ignored. The escape also clears `key_package_sent_to`, because the reciprocal
advertisement that does the re-teaching is skipped for a peer we have already
sent to.

Admitting it costs nothing an attacker wants: what survives is capability
advertisement, which this protocol treats as unauthenticated hint data
everywhere else, and the destructive half stays shut.

### First contact necessarily signs v1

A stranger's capabilities arrive in their reply, so the first key package to a
peer never met is signed under the older payload. That is inherent rather than a
gap to close, it converges in one round trip, and the ratchet closes behind it.

### Published Nostr key packages stay on v1

A published record has no known verifier: it is left on a relay for any stranger
to fetch, so there is no advertised capability to pick a payload from. Signing
v2 would make it unverifiable to every install that has not upgraded, and for
cold contact that means the record does not work at all: being readable by
someone never met is its entire purpose.

It is not the hole it looks like. The record carries `session_reset: false`, so
it holds no directive worth replaying, and re-delivering it can do nothing but
re-offer a key package still inside its own validity. Bounding *that* was
[issue 396](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/396)'s
subject rather than this one's, and it is now bounded: `verify_lifetime_bound`
refuses a window wider than `MAX_ACCEPTED_KEY_PACKAGE_LIFETIME` at import and on
every read of the cache, so the replay of a published record expires with the
package instead of never.

### The failure mode is a clock, and it needs a lever

Both checks read the verifier's own clock. A device that comes up with an unset
one reads every honest peer as decades in the future and refuses all of them,
taking out its whole control plane (key package exchange included) for as long
as the clock is wrong.

That is a self-inflicted outage a shipped app cannot wait for a new binary to
fix, so `security.control_freshness_enforced` (default `true`) returns a node to
pre-403 behaviour exactly: signatures still verified, nothing refused for its
age, resets honoured on any verified frame. It reaches the bindings for the same
reason. It is a diagnosis and recovery tool, not a setting to deploy on.

**With the switch off a node records nothing, and that half is load-bearing.**
The gate reports the frame's age as *unestablished* rather than as established,
so no timestamp travels to the handler and the high-water mark cannot move.
Reporting the opposite would be the easier reading of "the operator asked us not
to check", and it is the failure this decision calls worse than the replay
itself: with the check off, one captured frame signed under the older payload
carries an attacker-written timestamp, and honouring it parks a peer's mark at
`i64::MAX`. That survives a restart and the switch being turned back on, and it
denies that peer every future reset, which is to say every future chance to heal
a forked session. What keeps resets working meanwhile is the dispatch site
consulting the switch itself, not the gate pretending an age was checked.

The refusal is also reported under its own `STALE_CONTROL_FRAME` code rather
than folded into the signature failures, because an integrator who cannot tell a
clock fault from a forgery cannot diagnose either. Many peers in a short window
is a clock; one peer while others are fine is that peer.

### What is left

Replay of a frame **inside** the window, other than a session reset, is bounded
to 30 days rather than closed. Those frames carry no directive that destroys
state, which is why the spent mark covers the reset alone and not every
control frame: a mark per directive class is a per-peer record per class, and
the cost is not worth paying for a frame whose replay re-states something
already true.

A control frame already sitting in the outbox when a peer's ratchet closes is
refused for the rest of its retry ladder. Outbox entries are frozen signed
bytes, so a frame minted under the older payload before that peer's first v2
signature arrived cannot be re-signed, and it is refused on every attempt until
terminal failure, up to the absolute cap of about 28 days. This is inherent to
any ratchet rather than a gap in this one, it is bounded, and it heals: a key
package still escapes, so the pair re-teaches itself and later frames are minted
under the newer payload. It is written down here so the next reader meets it as
a known cost rather than as a bug report.

Nothing here changes the relay-answer forgery residual (threat model R1). A
frame with no signer cannot be given a freshness statement either.
