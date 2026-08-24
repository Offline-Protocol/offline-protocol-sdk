# Changelog

All notable changes to the Offline Protocol SDK are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). This changelog covers
everything after the **v0.7.1** release.

This file holds unreleased changes and the current release. Older releases are
archived by series under [docs/changelog/](docs/changelog/); see the
[archive index](docs/changelog/README.md).

## [0.24.0] — 2026-08-24

> **A door lock speaks this protocol now, and not a smaller version of it.**
> `offline-protocol-leaf` is a constrained device as a real peer: RFC 9420 MLS
> through mls-rs, the same frames, the same envelope, the same trust gates, no
> second sealing path and no reduced properties. It runs the never-committing
> member profile, so the phone creates the group and issues every commit while
> the device joins, opens, answers and persists. The whole image measures
> 449.5 KiB of flash on a Cortex-M33, a little over a quarter of a 1536 KiB
> xG24, and `tools/embedded-footprint/measure.sh` prints the table rather than
> this note pinning it. Nothing an engine-hosting application links changes: the leaf
> crate sits on `offline-protocol-core` and `offline-protocol-sealed` and never
> on the engine.
>
> **Four obligations come with that crate and none of them can be met by a
> passing build.** A leaf takes its time as a parameter (a device that lets an
> MLS library read a clock it does not have stamps 1970 and is refused as
> expired, so it never pairs at all), registers no `getrandom` backend
> (firmware wires the part's hardware entropy, and key generation is exactly as
> strong as what it returns), needs a `LeafStore` that is atomic per entry, and
> owns **authorization** itself: a session proves who a peer is and never that
> the owner meant them, so a lock that opens for whatever arrives on an
> established session opens for anyone patient enough to pair with it. See
> [docs/spec/leaf-provisioning.md](./docs/spec/leaf-provisioning.md) and
> [ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md).
>
> **A key package's validity window is bounded now, in both directions, and one
> class of peer stops being accepted.** RFC 9420 puts the maximum total lifetime
> on the application and nothing here defined one, so a package claiming a
> century was admitted and usable until the century ran out. All three admission
> routes now refuse a window wider than 90 days. That admits every package any
> released version of this SDK has put on the wire, but **a peer running an MLS
> stack that defaults to a year, which is what mls-rs hands out, is refused at
> import** with an `InvalidKeyPackage` naming both widths. Closing it also
> revealed that this SDK was minting 84-day windows while documenting 30; both
> now come from one number.
>
> **A control frame states when it was made, and a recorded one stops
> verifying.** Nothing about time was inside the signature, so a frame captured
> off the air verified as well on its tenth delivery as on its first, and the
> destructive case was a key package carrying `session_reset`, which tears down
> a live session on demand. `offline-ctrl-v2` binds the timestamp under its own
> domain, adds no wire bytes, and negotiates through `ctrl_versions` in the key
> package, so **nothing an application does changes.** Two things to know for a
> deployment: a new `STALE_CONTROL_FRAME` warning reports the refusal, and when
> it appears across many peers **the first thing to suspect is the device's own
> clock**, since the timestamp is judged against it. `security.control_freshness_enforced`
> turns enforcement off for a fleet whose clocks are wrong; it gives back the
> replay this closed, so it is a recovery tool rather than a setting to deploy
> on. See [ADR 0023](./docs/adr/0023-a-control-frame-states-when-it-was-made.md)
> and [docs/UPGRADING.md §17](./docs/UPGRADING.md#17-two-refusals-that-are-new-and-one-rotation-you-now-owe-v0240).
>
> **Post-compromise security is now something an application schedules.**
> Healing arrives when a commit rotates a member's leaf in the ratchet tree, and
> nothing here ever scheduled one: a re-key fired on an epoch desync, which is a
> fault rather than a cadence, so a pair that never forked never rotated.
> `rekey_session(peer_id)` drives it deliberately, in every binding. The cadence
> is the application's, and it matters most against a leaf node, which never
> commits at all, so every rotation in such a pair is the phone's to originate.
>
> **Two new crates publish alongside the rest**, at the same version:
> `offline-protocol-sealed`, which holds the one copy of everything both ends of
> a sealed conversation must agree on, and `offline-protocol-leaf`. Rust code
> that matches on the error type of `EncryptedMessage::from_bytes`,
> `EncryptedMessage::from_base64` or `GroupId::new` now sees `SealedError`;
> every rendered string, FFI code and wire byte is unchanged, and no binding
> surface breaks in this release.

### Security

- **A key package's validity window is now bounded, in both directions.** RFC
  9420 puts this on the application: define a maximum total lifetime and reject
  any key package claiming more. Nothing here did. OpenMLS looks like it does
  the job and does not, declaring the constant and shipping the predicate while
  `KeyPackageIn::validate` calls neither, so it checked only that *now* fell
  inside the window. A package claiming a century was admitted, cached, and
  usable for establishing new sessions until the century ran out
  ([#396](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/396)).

  All three routes by which a key package this install did not mint is admitted
  now refuse a window wider than 90 days: the 1:1 import, every read of the
  contact cache, and a group invite. The cache read matters as much as the
  import, because an entry written to the protocol-state store out of band never
  passed the import gate, which is the same reason the address-binding check
  runs there too; the group invite matters because it takes its bytes straight
  off the wire, and a bound applied on two routes of three is not a bound.

  Closing it turned up the other half. **This SDK was minting 84-day windows
  while documenting 30.** The key package builder was never told a lifetime, so
  OpenMLS applied its own default of three months plus an hour, and the constant
  named for a 30-day lifetime governed only when the local record stopped being
  offered, not the window every other install actually judges. Both now come
  from one number, and a package this install mints says 30 days because it is
  30 days.

  That is also why the cap is 90 days rather than the bound OpenMLS declares:
  three months plus an hour is exactly what an unconfigured build emits, so a
  cap set there would admit every package this SDK has ever minted with no
  margin at all and refuse any peer whose skew allowance is a second wider.

  **What to expect on upgrade.** Nothing for ordinary peers: 90 days admits
  every package any released version of this SDK has put on the wire, and a leaf
  node's 28 days clears it three times over. A peer running an MLS stack that
  defaults to a year, which is what mls-rs hands out, is refused at import with
  an `InvalidKeyPackage` naming both widths.

- **A control frame now states when it was made, and a captured one stops
  verifying.** The canonical signing payload bound the sender, the message id,
  the recipient and the content, and nothing about time, so a frame recorded off
  the air verified as well on its tenth delivery as on its first. The
  destructive case was a key package carrying `session_reset`, which tears down
  a live session: anyone who recorded one held a repeatable way to break a pair
  ([#403](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/403)).

  `offline-ctrl-v2` adds the frame's timestamp to the signed payload, under its
  own domain so the two can never be confused. Nothing on the wire grows: the
  timestamp already crosses both codecs and is not rewritten in flight. A
  verifier refuses a frame outside its window (30 days past and 48 hours future
  on an install running the engine, 48 hours on a leaf node), refuses the older
  payload from a peer that has once proved it can produce the newer, and admits
  a `session_reset` only above a per-peer high-water mark, so one recording is
  worth one teardown rather than one an hour for a month.

  **Nothing an application does changes.** Peers negotiate the payload through
  `ctrl_versions` in the key package, first contact converges in a single round
  trip, and a peer that has never produced the newer payload keeps every
  behaviour it had, including resets, which is what a driven rekey arrives as.
  See [ADR 0023](./docs/adr/0023-a-control-frame-states-when-it-was-made.md).

  Two things to know for a deployment. A new `STALE_CONTROL_FRAME` security
  warning reports the refusal, and **the first thing to suspect when it appears
  across many peers is the device's own clock**, since the timestamp is judged
  against it. And `security.control_freshness_enforced` (default `true`,
  `controlFreshnessEnforced` in the bindings) turns enforcement off without a
  new binary, for a fleet whose clocks are wrong; it gives back what #403
  describes, so it is a recovery tool rather than a setting to deploy on.

  Raising `outbox_max_lifetime_ms` above 7.5 days now has a consequence worth
  knowing: this device's own late retransmissions of signed control frames can
  be refused as stale by the peer they finally reach. Ordinary messages are
  unaffected. The default of 7 days sits well inside the window.

- **The npm package publishes over trusted publishing, so no long-lived npm
  credential exists anywhere.** `release.yml` authenticated with an `NPM_TOKEN`
  repository secret until now; it exchanges the workflow's own OIDC identity for
  a short-lived, workflow-scoped registry token instead, which cannot be
  exfiltrated from a build log or replayed from anywhere else. The trust is
  registered at the registry against this repository and the workflow
  *filename*, so renaming `release.yml`, or moving the publish into a reusable
  workflow, revokes the ability to publish with nothing in the diff that says
  so.
- **Provenance is no longer conditional on a flag this repository computes.**
  `--provenance` was resolved from `github.event.repository.visibility`, because
  the registry answers HTTP 422 to a provenance publish from a private source
  repository, and that failure took down the v0.20.1 release after the GitHub
  release had already been created. npm applies the same rule itself now, by
  declining before publishing rather than by rejecting, so the workaround is
  deleted rather than carried forward. What actually shipped is read back from
  the registry after the upload: a published version without an attestation
  fails the run, because a green publish step proves the tarball uploaded and
  never that it carried provenance.

### Added

- **An application can now ask for a session rotation, which is the only way
  post-compromise security arrives on a pair that never desyncs.** Healing
  happens when a commit rotates a member's leaf in the ratchet tree, and this
  SDK originates one on a re-key. Nothing scheduled a re-key: one fired on an
  epoch desync, which is a fault rather than a cadence, and the rekey interval
  is a floor on how often that may happen rather than a timer that fires
  anything. So a pair that never forked never rotated, and the window a stolen
  key stays useful for was bounded by nothing.

  `rekey_session(peer_id)` drives the existing path deliberately, over the FFI
  and in every binding (`rekeySession` in Swift, Kotlin and TypeScript). A peer
  cannot tell it from a desync-driven re-key, because the teardown and the reset
  advertisement are one shared step. Queued messages survive, sealed at flush
  time against whatever session is current then.

  It returns a boolean rather than nothing. `false` means the per-peer
  rate-limit window has not lapsed, which is not a failure: a caller on a fixed
  schedule that briefly outruns the floor is behaving correctly and a later call
  succeeds. The floor is shared with the desync path, so a caller looping on
  this cannot do to a pair what that bound exists to stop an attacker doing, and
  the `SESSION_REKEY_TRIGGERED` warning is deliberately **not** raised here,
  because it names an epoch desync and exists so a sustained rate of them reads
  as an attack signature.

  **A rotation that fails changes nothing.** The reset is advertised before the
  local session is torn down, and a transport reports an error only once nothing
  has accepted the frame, so a failure leaves the session intact, still usable,
  and the window unspent. Rotate while the peer is reachable and treat a failure
  as "try again later". The order matters most against a leaf node, which speaks
  only when spoken to: discarding first and then failing to send would leave the
  phone with no session, the device holding one it will never be told to drop,
  and nothing on either side that re-opens the exchange. The desync path makes
  the opposite choice on the same failure, because what reaches it is a fork
  rather than a healthy session.

  **The cadence is the application's, and this is an obligation rather than a
  setting.** A rotation costs a teardown, a key-package exchange and a
  re-establish, and what that is worth to a mains-powered lock and to a phone on
  a metered link are different answers that nothing on the wire distinguishes.
  It matters most against a leaf node, which never commits at all, so every
  rotation in such a pair is the phone's to originate. The documentation
  previously said healing arrived "on a cadence the phone sets", which read as a
  schedule and described a rate limit; ADR 0021, the leaf provisioning chapter
  and the session-lifecycle machine now say what actually drives it.

- **`offline-protocol-core` compiles without `std`.** The crate now builds for
  bare-metal targets with `--no-default-features`, which is what a constrained
  leaf node links: a Cortex-M door lock, sensor or mains-powered relay that
  speaks the protocol but cannot host the engine. `std` is a feature and it is
  on by default, so the published API and every existing consumer are
  unchanged.

  What `std` gates is exactly what the platform has to supply and a bare-metal
  target does not: a wall clock (`Timestamp::now`, `WallClockTimestamp::now`),
  a monotonic clock (`LocalInstant`), entropy (`MessageId::new`, and
  `Message::new` and `MessageBuilder` with it), and threads (the `sync`
  module). Everything that parses, validates, re-encodes or compares is present
  in both configurations, which is the half a leaf node that only forwards
  uses. A node that answers also mints, and `Message::from_parts` in this same
  release is what lets it: build messages from wire-supplied parts, via
  `MessageId::from_bytes` and `Timestamp::from_millis` on that path.

  Two field types changed spelling to `MetadataMap`, which under `std` **is**
  `HashMap<String, String>` exactly as before, and is a `BTreeMap` without it.
  Nothing moves on the wire: JSON objects are unordered by definition and the
  binary v1 codec carries metadata as an ordered `Vec<(String, String)>`.
  `chrono` is no longer a dependency of this crate. See
  [ADR 0020](./docs/adr/0020-core-compiles-without-std.md).

- **The protocol layer's cost on a Cortex-M33 is measured, not estimated.**
  `tools/embedded-footprint` links two firmware images for
  `thumbv8m.main-none-eabihf` and reports the difference: **about 95 KiB of
  flash (97,152 bytes) and no static RAM** beyond the heap a node provisions
  for itself. That is roughly 6% of an xG24's 1536 KB. CI prints the table on
  every run.

  The figure is the protocol layer alone. It excludes signature verification
  and payload unsealing, the radio driver, an RTOS and key storage, and it is
  not the engine, which is `std` and `tokio` bound and does not build for this
  target at all.

- **`offline-protocol-sealed`, a new crate holding what both ends of a sealed
  conversation must agree on.** A phone and a leaf node run different MLS
  implementations ([ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md)) and
  are not allowed to disagree about anything outside them: the
  `EncryptedMessage` envelope and its compact codec, `derive_address`, the
  domain-separated canonical signing payload every signature is taken over, and
  the sender-ratchet bounds a session is configured with. All four were in
  crates that need `std`. They now live in one crate that builds for bare metal,
  and everything else re-exports or delegates to it, so there is exactly one
  implementation of each in the workspace.

  `tools/mls-interop` previously carried its own copies of the derivation rule
  and the ratchet constants, with nothing pinning them to the SDK's. It now
  imports them, and a guard test reads the harness source and fails if the
  copies come back. See
  [ADR 0022](./docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md).

- **Two more pieces join that layer: the 1:1 control-frame prefixes and
  `KeyPackagePayload`.** Both were private to the engine, and both are things
  the two ends of a sealed conversation have to agree on while building them
  with different MLS implementations. A frame's type is the prefix its content
  begins with, so two ends that disagree about one do not have a conversation;
  and the key package payload is the only channel in this protocol by which
  capabilities are advertised, which is why a device that cannot mint one is
  served the floor forever.

  `offline-protocol-sealed` gains `prefixes` (the six a pair speaks: key
  package, Welcome, encrypted, the two confirmation frames, and the encrypted
  confirmation that only ever travels inside an envelope), plus
  `KeyPackagePayload` and `MLS_ENVELOPE_COMPACT_V1`. **Reservation stays in the
  engine**: `INTERNAL_PREFIXES` is what refuses application content beginning
  with a reserved prefix, and it is still generated from the one macro
  invocation that names them, so adding a prefix is still a single-line change
  in a single place. Group, connection, relay, presence and sealed-body
  prefixes stay in the engine too, because none of them reach a device and the
  alternative puts relay vocabulary in an image budgeted in kilobytes.

  Pure relocation: no wire byte, no JSON field, no error string and no FFI
  signature changes, and every engine use site is untouched because the
  constants and the type are re-exported under their existing names. Two new
  guard tests pin the prefix literals byte for byte and prove none of them is a
  prefix of another, which is a live near miss between `__MLS_ENC__` and
  `__MLS_ENC_CONFIRM__`.

- **What a leaf node owes at pairing is specified** in
  [docs/spec/leaf-provisioning.md](./docs/spec/leaf-provisioning.md). A leaf is
  a peer rather than a class of peer: same frames, same envelope, same trust
  gates, no second sealing path. What is genuinely different is four
  obligations that a passing build does not reveal, and each is stated with the
  failure it prevents. A static artifact carries `{address, pubkey}` and never
  a key package, because an init key is single use and a sticker is not. A key
  package is minted from a **supplied** time, because an implementation that
  cannot read a clock stamps a validity window at the Unix epoch and the peer
  refuses it as expired, so a device that ships that way never pairs at all.
  State is persisted **before** a frame that advanced it is emitted, because a
  device that answers and then loses power comes back and reuses an AEAD nonce.
  Entropy comes from real hardware, because MLS key generation is exactly as
  strong as what that source returns.

  The chapter also specifies the never-committing profile (what a leaf emits,
  what it accepts, and what it must never emit), and states that a phone-driven
  rekey reaches a device as a key package with `session_reset` set rather than
  as an unsolicited Welcome, which is the sequence a device has to survive for
  post-compromise security to arrive at all.

- **The leaf profile in [capability
  negotiation](./docs/spec/capability-negotiation.md)**: what a minimal device
  advertises, and the two rules that are easy to get backwards on a part where
  every kilobyte is argued over. A device that advertises nothing still
  interoperates, because empty lists select the floor and the floor is a
  complete conversation. And parsing stays unconditional on a device too: a
  leaf that decodes only the envelope form it advertised drops frames from a
  peer that legitimately believed it capable.

- **Two threat-model entries for the device class**:
  [A8, the provisioning-time adversary](./docs/security/threat-model.md), who
  handles a device or its label before its owner does, and has no
  cryptographic answer because every cryptographic check passes (the key on the
  swapped label does derive to the address on that label); and R12, which says
  plainly that boundary 3's "platform secure storage holds" assumption means an
  OS keystore on a phone and whatever the part provides on a microcontroller.
  One device, one key: a fleet sharing an identity key turns one extraction in
  a laboratory into every unit's identity.

- **`offline-protocol-leaf`: a constrained device that speaks this protocol as
  a real peer.** A door lock or a sensor with a few hundred kilobytes of flash
  and no operating system now runs RFC 9420 MLS through mls-rs and holds an
  end-to-end encrypted conversation with a phone under the same guarantees a
  phone gets. Same frames, same envelope, same trust gates, no second sealing
  path and no reduced properties, which is the decision
  [ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md) took and this crate
  implements.

  `LeafDevice` is a frame-level state machine rather than a bag of primitives:
  an inbound message goes in, the frames to send and what happened come out. It
  runs the **never-committing member** profile, so the phone creates the group
  and issues every commit while the device joins, opens, answers and persists.
  Per-commit cost on the device is two elliptic-curve operations and
  per-message cost is symmetric only.

  What it refuses is the point. Every control frame must carry a signature
  whose key **derives to the address the frame claims**, and an identifier that
  is not an address is the same refusal rather than a skip, because a claim
  with no derivation to check is the bypass. A Welcome must name the peer that
  signed it, name the group this pair would build, spend **the key package this
  device minted for that peer**, and then actually join the group its body
  claimed. That third one is what separates a peer from anyone who overheard
  it: a key package rides in a frame that is signed but not encrypted, so a
  copy taken off the air is as spendable as the original and satisfies every
  other gate honestly. Checked before the join, because joining spends the init
  key, and a package burned by a listener leaves the peer it was minted for
  holding a Welcome that no longer opens. The group must still be a pair every
  time the device uses it, re-read from the roster and derived rather than
  read, because a commit changes the membership without changing the group id
  and every commit here is the peer's to make. Checking only on the commit
  would check at the one moment whose answer cannot be kept: it is applied and
  durable by the time there is a roster to read, so the refusal is a returned
  value that no reboot survives, and the device would seal its next message
  into the widened room and call it an ordinary session. A sealed frame's MLS
  sender must be the peer the frame came from, commits included. A confirmation
  probe is answered only by a device that still holds a session it can load and
  that is still this pair, because a peer confirms on that answer and
  flushes into it, and an inbound acknowledgement is never acted on at all,
  because a leaf emits those and never probes, so every one that arrives is
  unsolicited and treating it as proof of a session would let any keypair
  holder assert one. A reset frame is acted on once, so a captured one is not a
  repeatable session teardown. Underneath all of them, a frame addressed to
  another node is ignored before a prefix is read: a signature covers the
  recipient rather than checking it, and a sealed frame carries none at all, so
  without that gate an overheard key package mints a private init key nobody
  asked for and a captured frame with its recipient rewritten is still acted
  on. Every one of these is covered against a real OpenMLS phone in the same
  process, which is the only kind of test that catches a default in one library
  the other refuses.

  **What it keeps is bounded.** Prior-epoch records are trimmed to a window
  rather than kept forever, which bounds both the flash they occupy and how far
  back a stolen device reads; unpairing erases them along with the session and
  with the key package minted for that peer, so neither epoch secrets nor an
  unspent init key outlive the erasure an owner asked for. Peer records and
  unspent key packages are bounded too, and a full peer table refuses a
  stranger rather than evicting somebody the owner paired with, because
  producing a frame that derives to its own address costs an attacker nothing.
  Every operation that advances state takes `&mut self`, so two seals racing
  into one AEAD nonce is a compile error rather than a rare one.

  **Persist-before-emit is structural, not documented.** Every operation that
  advances ratchet state writes through `LeafStore` and only then returns the
  frame, so a store that fails produces an error and no frame at all. A device
  that emitted first would come back from a power cut and reuse an AEAD nonce,
  which is a confidentiality failure rather than a lost message. A test arms a
  failing store and asserts both that nothing is emitted and that the write was
  actually attempted, so it cannot pass by short-circuiting earlier.

  The seam is atomic per entry rather than across a set, so what a cut lands
  between is chosen rather than left to chance. Prior-epoch records go down
  before the state, because a state with records it does not yet reference
  costs nothing while the reverse loses the out-of-order tolerance a lossy
  radio needs. The **epoch marker mls-rs sequences against travels inside the
  state entry**, because ordering cannot make those two safe in either
  direction: a marker that reached flash without its state refuses every commit
  that follows, permanently, and the device goes deaf to its peer until that
  peer drives a full reset. A separate high-water record survives the state and
  bounds the erasure sweep on unpair, which is the one job the in-state marker
  cannot do.

  Four obligations stay with the integrator, and the API is shaped so none can
  be forgotten silently: every entry point needing a clock takes
  `now_unix_secs` (a device that lets an MLS library read a clock it does not
  have stamps 1970 and is refused as expired, so it never pairs at all), the
  crate registers no `getrandom` backend (firmware wires the part's hardware
  entropy source, and key generation is exactly as strong as what it returns),
  and `LeafStore` must be atomic per entry. The fourth is **authorization**: a
  session proves who a peer is and never that the owner meant them, since any
  address in radio range can complete a pairing, so firmware decides when the
  radio accepts one and what a given peer's messages may actuate. A lock that
  opens for whatever arrives on an established session opens for anyone patient
  enough to pair with it. `LeafDevice::peers` is how firmware audits what a
  device accumulated and `unpair` is how it removes one, and every route to a
  session puts the peer on that list, because a session firmware cannot see is
  one it can neither review nor revoke.

- **`Message::from_parts` in `offline-protocol-core`**, which is
  `Message::new` with its two ambient inputs, the clock and the entropy, made
  explicit. ADR 0020 made core build without `std` on the reading that a
  constrained node "receives frames rather than minting them". That is true of
  a node which only forwards and false the moment one answers, so without this
  a bare-metal node could not produce a `Message` at all. `Message::new`
  delegates to it, so there is one struct literal rather than two that drift.

### Changed

- **A leaf node now answers the frames it receives, and a phone stops retrying
  at it.** A leaf owed no delivery acknowledgement, so a phone that marks its
  frames as needing one settled nothing and ran the full retry ladder against
  every frame it sent: ten retransmissions of a sealed frame over about thirteen
  minutes, on the link `offline-protocol-leaf` exists to be careful with. Each
  retransmission arrived at the device as a replay of a generation the ratchet
  had already spent, so it was refused correctly and firmware saw a run of
  decrypt failures indistinguishable from somebody replaying frames at it on
  purpose. The one signal that would tell an integrator they are under attack
  was buried under traffic the protocol generated itself
  ([#402](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/402)).

  The answer is an ordinary message: no prefix, no signature, empty content, and
  one metadata entry naming the frame it answers. It costs a fraction of what it
  prevents, and it is also the only way an application ever learns that the
  command it sent to a lock arrived, since `message_delivered` now fires for a
  leaf peer instead of `MessageFailed` after retry exhaustion.

  Three rules go with it, and each closes something that answering naively would
  open. A device answers **only a peer it holds a record for and only a frame
  that proved it came from that peer**, so a stranger in radio range cannot make
  it transmit on demand or learn that a node exists at an address: the record
  says an address once paired, and a frame's sender is a plaintext field, so
  either gate alone admits whoever overheard the pair once. It answers **only
  what it accepted**, because handing a receipt to
  whoever just failed the signature gate tells them their frames are being
  processed. And it **repeats an answer for a frame it already answered**,
  without opening it again, because the answer is the frame most likely to have
  been lost and a device that stayed quiet for the retransmission would leave
  the ladder running anyway. That memory is a few frames deep; past it a replay
  is refused exactly as before, which is deliberate, because absorbing every
  replay would trade one invisible attack for another.

  **For integrators**, two things change on the device. A leaf emits one small
  frame per accepted frame that asked for an answer, where it previously emitted
  none, and a run of `LeafError::Mls` is now worth investigating rather than
  being the protocol talking to itself. See
  [the leaf provisioning spec](./docs/spec/leaf-provisioning.md#what-a-leaf-owes-for-a-frame-it-received).

  The leaf image grows 3.2 KiB for it, from 445.9 KiB to 449.1 KiB. The
  shipping profile measures 449.5 KiB by the end of this release, the rest
  being the pairing fix below; `tools/embedded-footprint/measure.sh` prints the
  table and this note does not pin it.

- **The envelope codec and `GroupId::new` now return `SealedError`** rather
  than `MlsError`, having moved into `offline-protocol-sealed`. Nothing else
  changes: `From<SealedError>` exists for both `MlsError` and the engine's
  `Error` and passes the inner message through, so every rendered error string,
  every FFI error code and every wire byte is what it was. This is visible only
  to Rust code that matches on the error type of `EncryptedMessage::from_bytes`,
  `EncryptedMessage::from_base64` or `GroupId::new` directly.

- **The embedded footprint harness measures the shipping crate.** Its leaf
  image drove mls-rs directly, so it linked neither the envelope codec, nor the
  control-frame signing, nor the address derivation: it priced an image nobody
  could ship. It now runs `offline-protocol-leaf`, and the whole-image figure
  moved from **391.3 KiB to 445.6 KiB** of flash, a little over a quarter of a
  1536 KiB xG24.

  The 400 KiB figure in [ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md)
  was a decision gate, set to answer whether MLS on a leaf node was viable
  before anything was built, and it did that job. It is not a budget the
  shipping image is held to, and the recovery lever recorded beside it is still
  worth more than the growth: about 111 KiB of the image is P-384 and P-256
  arithmetic that nothing uses, linked because the crypto provider keeps all
  four curves in one enum with no feature gating.

  The `leaf-min` image is gone. It priced application messages with the
  resilience features off, and once the workload moved onto the crate, which
  requires all four mls-rs features, cargo's feature unification made it
  measure the same bytes as `leaf`. A row reporting a number for a
  configuration nobody can build is worse than no row.

### Fixed

- **A phone could not start a pairing with a leaf node.** A sender builds the
  freshness-bound control payload for a recipient that has advertised it and the
  older one for a recipient that has not, and capabilities travel in a key
  package, so the first key package to a peer never met is signed under the
  older payload however new the sender is. ADR 0023 records that as inherent to
  the negotiation rather than a gap to close.

  A leaf verified only the freshness-bound payload, on the reasoning that no
  phone old enough to need the other one had ever paired with a device. That
  reasoning was about releases, and this is not about releases: today's engine
  signs the older payload on the frame that opens a phone-initiated pairing,
  because at that moment it has never seen the device's `ctrl_versions`.

  So the device refused it, and refused every retransmission. The pairing could
  not start from that side at all: the device learned nothing about the phone,
  so it had no address to advertise back to, and the phone's ladder delivered
  ten signature failures to firmware whose only account of itself is that error
  stream. Only a pairing the device began could complete, and beginning one
  needs an address it could only have learned from the frame it refused.

  A leaf now accepts the older payload on `__MLS_KEY_PKG__` and on nothing else,
  which covers both first contact and the case the specification already
  required, a peer whose record of the device was lost. **A frame admitted under
  it has its `session_reset` ignored**, so the directive that destroys state
  still needs a stamp inside the signature and the replay closed by issue 403
  stays closed; what survives is capability advertisement, which this protocol
  treats as unauthenticated hint data everywhere else. Its age is not judged,
  because that payload leaves the timestamp outside the signature. A Welcome and
  a confirmation probe keep refusing it and need no exception, neither being
  able to arrive before the device's own key package has reached the peer.

  Found by running the engine against the leaf crate for the first time.

- **A Python node was never routable by its `off1…` address over the Internet
  transport.** The transport authenticated to the relay but never answered the
  address-routing challenge, so the relay kept attributing the connection by
  account name and could not resolve messages addressed to the node. Addressed
  sends and service discovery therefore never reached it. The transport now
  signs the same domain-separated proof the Swift and Kotlin bridges sign and
  replies with `DeclareAddress`, pinned against the relay's own hex vector so
  the three implementations cannot drift apart. Relays that do not advertise
  `address_routing_v1` see byte-identical behaviour to before, and a
  declaration that cannot be made leaves the connection authenticated and
  working in account-name space, exactly as it worked before addresses
  existed.

  The declaration is written before the outbox flush, because the relay
  attributes each frame by whatever the connection has proved at the moment it
  reads that frame and never re-stamps retroactively. Anything sent ahead of
  the proof stays attributed by account name for good, and its address-stamped
  `Message.sender` then fails the receiver's strict sender check, which is
  what drops the key-package and welcome frames a new MLS session needs.

  The relay's two answers reach the core through `internet_address_declared`
  and `internet_address_declaration_refused` rather than the log, so a Python
  node performs the same binding-mismatch lockstep check as the other
  bridges. The relay's advertised capabilities are also injected before the
  status flip, so the group broadcast gate sees them on the connection they
  belong to.

- **Four benchmarks were compiling to empty stubs, and a fifth was timing an
  error return.** `message_throughput` and `protocol_performance` guarded their
  real bodies behind `#[cfg(test)]` and paired each with an empty
  `#[cfg(not(test))]` stub. A bench target never sees the transport crate's own
  `cfg(test)`, so `MockTransport` was unreachable and `send_message`,
  `process_loop`, `protocol_start_stop` and `transport_send_receive` measured
  nothing at all while `cargo bench` reported success. They now reach
  `MockTransport` through the transport crate's `test-utils` feature, the way
  the engine's own tests already did, so a benchmark that cannot compile fails
  the build instead of silently measuring an empty function.

  The revived `send_message` came back worse than empty. It built its engine
  from a stock `ProtocolConfig`, where encryption fail-closes and
  `initialize_mls` is never called, so every iteration took the "MLS required
  but not initialized" arm and returned before a `Message` was ever
  constructed. Criterion reported a confident 535 ns for validating a recipient
  and allocating an error string, and a `.ok()` on the result is what hid it.
  The honest figure is 3.86 us. Results are now `expect`ed rather than
  swallowed, so a send that stops short of the transport fails the run instead
  of timing something else. `transport_send_receive` was never a round trip
  either, because `MockTransport::send` records outbound frames without looping
  them back; it now feeds the queue and asserts the message comes back.

  The crypto term that send path excludes is measured for the first time, by
  two new benches: sealing a DM into an MLS ciphertext over an established 1:1
  session, and opening one. 79.5 us and 91.3 us on an Apple Silicon core, so a
  full encrypted send is about 83 us of software time end to end, the seal
  dominating dispatch by roughly twenty times. The open bench draws a fresh
  ciphertext every iteration through `iter_batched`, because the ratchet
  refuses replays and a reused one would time an error return, which is exactly
  the trap `send_message` had just been pulled out of. Both figures are the
  crypto cost alone: storage is the in-memory test store, where on a device the
  session-state writes land in a platform keystore.

  This is the second time these rotted. A CI job that compiled every benchmark
  was added and removed forty-nine minutes later, and nothing has compiled the
  bench targets since.

### Documentation

- **The Bluetooth LE framing a leaf's firmware must speak is specified** in
  [docs/spec/ble-framing.md](./docs/spec/ble-framing.md). Everything above the
  link had a chapter and the link itself did not, so a firmware author had to
  read the Swift and Kotlin managers to learn the service and characteristic
  UUIDs, the fragment header, and what a receiver owes a sender that fragments
  differently. The chapter states the GATT contract, the fragment codec's
  header and payload, sizing, the reassembly obligations and the constants, and
  `crates/offline-protocol-transport/tests/data/ble-framing-v1.vectors.json`
  carries frames in hex with the outcome each one requires: four reassembly
  cases including out-of-order and duplicated indices, and nine refusals. They
  are computed from the chapter rather than from the implementation, so a
  disagreement is evidence about one of them rather than two copies of one
  mistake agreeing.

  **The identity assertion had no specification anywhere.** Its layout lived
  only in comments in `PeerIdentityBinding.swift` and `.kt`: 32 bytes of
  Ed25519 public key, 64 of signature, and the remainder is the data that
  signature covers. Two properties that follow from it are now written down
  rather than left to be rediscovered. The signature carries no domain
  separation, so any signature that key has ever produced verifies as an
  identity assertion; and `signed_data` is never decoded, never compared
  against the advertisement received over the air, and never routed on, so the
  assertion is a static read and therefore a replayable one. Neither is a new
  hole, and both are what gets simplified into one by somebody who cannot see
  why the binding is only ever used to name a peer.

  **On this radio the fragment count binds long before the message ceiling.**
  `BLE_MAX_FRAGMENT_COUNT` is 512, so one message tops out at 512 x
  (`mtu` - 10 - `id_len`): 71,168 bytes at the 185-byte MTU floor and 238,592
  at the 512-byte clamp, both far under the 1 MiB `DEFAULT_MAX_MESSAGE_SIZE`.
  Anyone sizing something large, an MLS Welcome into a big group most of all,
  needs that number and not the 1 MiB one, and it fails at the sender rather
  than in flight. The count cap is agreed rather than local policy, sender and
  receiver both enforcing 512, so a receiver that lowers it drops conforming
  senders. A guard test recomputes the stated figures from the constants they
  derive from, because a computed number written into prose is the kind that
  goes stale in silence.

- **Why a leaf node speaks MLS through a second implementation is recorded** in
  [ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md), along with the interop
  harness that keeps the decision honest. ADR 0020 left a device that can parse
  a frame, validate its addressing and forward it but cannot read one word of
  it, every payload in this protocol being MLS ciphertext, which makes a lock a
  relay rather than an endpoint: the phone can send it "unlock" and it can
  neither open the message nor produce an answer anyone would believe.

  `tools/mls-interop` answers the question the footprint tool cannot. An image
  that links 390 KiB of MLS and cannot decrypt a single frame the phone sends
  has passed the size gate and failed at the only thing that matters, so the
  harness runs OpenMLS and mls-rs against each other with this SDK's
  credentials and this ciphersuite, and reports a pass or a failure. Both
  stacks run the MLS working group's own vectors already, which is evidence
  about conformance in general and not about this pairing.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
| [0.23.x](docs/changelog/0.23.md) | 0.23.0 |
| [0.22.x](docs/changelog/0.22.md) | 0.22.0 |
| [0.21.x](docs/changelog/0.21.md) | 0.21.0 |
| [0.20.x](docs/changelog/0.20.md) | 0.20.1, 0.20.0 |
| [0.19.x](docs/changelog/0.19.md) | 0.19.0 |
| [0.18.x](docs/changelog/0.18.md) | 0.18.3, 0.18.2, 0.18.1, 0.18.0 |
| [0.17.x](docs/changelog/0.17.md) | 0.17.0 |
| [0.16.x](docs/changelog/0.16.md) | 0.16.6, 0.16.5, 0.16.4, 0.16.3, 0.16.2, 0.16.1, 0.16.0 |
| [0.15.x](docs/changelog/0.15.md) | 0.15.0 |
| [0.14.x](docs/changelog/0.14.md) | 0.14.0 |
| [0.13.x](docs/changelog/0.13.md) | 0.13.1, 0.13.0 |
| [0.12.x](docs/changelog/0.12.md) | 0.12.0 |
| [0.11.x](docs/changelog/0.11.md) | 0.11.1, 0.11.0 |
| [0.10.x](docs/changelog/0.10.md) | 0.10.0 |
| [0.9.x](docs/changelog/0.9.md) | 0.9.4, 0.9.3, 0.9.2, 0.9.1, 0.9.0 |
| [0.8.x](docs/changelog/0.8.md) | 0.8.0 |
