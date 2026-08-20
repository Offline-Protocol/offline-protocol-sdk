# Upgrading

Everything an application team has to change to move off `v0.16.x` and onto the
current `v0.23.x` line.

The breaking changes all landed in the **storage-split release**, `v0.17.0` —
`initialize_mls` changes shape, three config updaters become fallible, and
several previously-accepted inputs are now rejected at the boundary. Sections
1–12 below cover that release and are the ones that can stop your build.

Since `v0.17.0` three releases can break a build. `v0.20.0` enables iOS
autolinking, so a manual `pod 'MeshSdk'` line left in your `Podfile` now fails
`pod install` — React Native on iOS only; it is a one-line deletion, and
[§12.1](#121-react-native-ios-delete-your-manual-pod-meshsdk-line-v0200) has the
full list of Podfile leftovers to remove, and the same release is what makes iOS
**simulator** builds link correctly. `v0.21.0` is breaking on **every** surface:
`ProtocolConfig.userId` is replaced by `profile`, and the device's identity on
the wire becomes a self-certifying `off1…` address derived from an identity key
it mints for itself. [§14](#14-your-identity-is-derived-not-chosen-v0210) is the
migration guide — read it before bumping, because there is deliberately no
in-place migration of existing sessions. `v0.23.0` breaks every binding once
more, in the one place nothing was reading: the learned-route API is deleted,
eight methods and four types that wrote into a table the delivery path never
consulted, along with `ProtocolConfig.path`.
[§15](#15-the-learned-route-api-is-gone-v0230) lists them; delete the calls,
because there is nothing to replace them with.

Otherwise, where a later section documents an
addition or a behaviour change, it is labelled inline with the release that
introduced it (for example, `wipePersistedState` in
[§10](#10-storage-is-now-isolated-per-app_id-user_id) arrived in `v0.18.2`). If
you are already on `v0.17.0`, those labelled paragraphs are the only parts you
still need — coming from `v0.18.x` that means
[§1.10](#110-the-inbound-plaintext-gate-no-longer-reads-session_states) and
[§1.11](#111-key-packages-are-checked-against-the-peers-identity), two
receive-side behaviour changes in `v0.19.0` that compile fine and can still
surprise you at runtime.

`v0.20.0` adds one more of that kind, and it needs no section because it needs
no code change: React Native's exported `ProtocolState` enum now holds the
*strings* `getState()` has always resolved (`"Stopped"` / `"Running"` /
`"Paused"`) rather than `0` / `1` / `2`, so `state === ProtocolState.Running` —
which could never be true before — now works, and `state === 'Running'` stops
failing `tsc`. Nothing read back from `getState()` changes. The single hazard is
a `ProtocolState` your app *persisted itself* (AsyncStorage, redux-persist):
that value returns as a number and now matches nothing, so treat an
unrecognised persisted value as `Stopped`.

`v0.20.1` adds one more, and this one *does* need a change if your UI settles a
message on it: `messageDecryptionFailed` is now advisory and fires once per
failed **attempt** rather than once per message, because a receiver that cannot
decrypt or parse a frame now withholds the delivery ACK so the sender's resend
can deliver.
[§11.1](#111-messagedecryptionfailed-is-advisory-not-terminal-v0201) has the
full contract and the events to settle on instead.

`v0.21.0`'s changes are otherwise compile-breaking rather than quiet, but it
adds one more of these: `secure_session_failed` can now fire while the session
with that peer stays **live**
([§11.2](#112-secure_session_failed-no-longer-implies-the-session-is-gone-v0210)),
so an app that tears down session state on that event alone must stop.

`v0.22.0` adds two more. The first is the larger: the battery-aware relay policy
was complete but unreachable, because nothing ever wrote the charge that all of
it reads. `relay_promoted` and `relay_demoted` could not fire, and the
forwarding battery floor was decorative. The feed is now wired through, so the
policy starts deciding behaviour the moment your app calls `setBatteryState`
([§11.3](#113-relay_promoted-and-relay_demoted-can-now-actually-fire-v0220)),
and on React Native two `relay` config fields that were parsed by nothing begin
taking effect. The second needs no code change and is pure relief: React
Native's `updateDorsConfig` rebuilt the entire `DorsConfig` from literals on
every call, so a payload naming one field silently reset everything the caller
did not mention. Both bridges now merge onto the live config read back from the
engine, so if you were restating every field to survive that, you can stop.

`v0.23.0` adds two that compile fine, and the first is a whole layer.
Replicated documents arrive **on by default**
([§16](#16-replicated-documents-are-available-11-and-in-groups-v0230)): this
build advertises document sync to its peers even if your app never opens a
`DataStore`. Nothing is stored until you write a document and nothing is sent
until you share one, and `data.enabled: false` refuses the layer outright, but
one obligation follows a custom storage backend, which is that
`DataStore.wipeAll()` joins `wipePersistedState()` on your logout path. The
second is quieter: transport choice now asks where the recipient is, so a
device holding a live mesh link to the recipient sends over that link even when
an internet carrier is up and scoring would have put the relay first. What is
delivered does not change; which radio carries it does, and so does how fast it
settles when the recipient is standing next to you.

Work through it in order. [§0](#0-before-you-ship-downgrade-is-not-a-rollback)
is a release-engineering decision, not a code change, and it is the one that
cannot be undone later.

---

## This release at a glance

| # | Change | Rust | Swift/Kotlin (UniFFI) | React Native | Python |
|---|--------|------|------------------------|--------------|--------|
| 0 | [Downgrade is not a rollback](#0-before-you-ship-downgrade-is-not-a-rollback) | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| 1 | [`initialize_mls` takes two providers](#1-initialize_mls-now-takes-two-storage-providers) | **breaking** | **breaking** | no code change | **breaking** |
| 2 | [Protocol-state values are `bytes`](#2-protocol-state-values-are-bytes-not-an-element-wise-sequence) | n/a | **breaking** (custom providers) | no code change | **breaking** (custom providers) |
| 3 | [Config updaters now throw](#3-the-three-reliability-config-updaters-are-now-fallible) | **breaking** | **breaking** (Swift needs `try`) | no code change | **breaking** |
| 4 | [Zero dedup values rejected](#4-zero-max_tracked_messages--retention_time_secs-is-now-refused) | **breaking** | **breaking** | check your config | **breaking** |
| 5 | [Recipient validation everywhere](#5-recipient-tokens-are-validated-at-every-outbound-boundary) | **breaking** | **breaking** | **breaking** | **breaking** |
| 6 | [256 KiB content cap](#6-send_message-rejects-content-over-256-kib) | **breaking** | **breaking** | **breaking** | **breaking** |
| 7 | [Pending-queue lifetime + caps](#7-the-pre-session-queue-has-a-lifetime-and-hard-caps) | new config | new config | new config | new config |
| 8 | [RN bridge fallbacks realigned](#8-react-native-bridge-fallbacks-now-match-the-sdk-defaults) | n/a | n/a | behaviour change | n/a |
| 9 | [Synthesized relay frames](#9-react-native-synthesized-relay-frames-must-be-unattributed) | n/a | n/a | only if you wrote native relay code | n/a |
| 10 | [Per-account storage namespaces](#10-storage-is-now-isolated-per-app_id-user_id) | your providers | your providers | behaviour change | behaviour change |
| 11 | [Events you must handle](#11-events-you-must-now-handle) | all | all | all | all |
| 12 | [Build & packaging](#12-build-and-packaging) | — | regenerate bindings | rebuild native | — |
| 12.1 | [iOS: delete your manual `pod 'MeshSdk'` line](#121-react-native-ios-delete-your-manual-pod-meshsdk-line-v0200) | n/a | n/a | **breaking** (`pod install` fails) | n/a |
| 13 | [Upgrade test checklist](#13-upgrade-test-checklist) | — | — | — | — |
| 14 | [Your identity is derived, not chosen](#14-your-identity-is-derived-not-chosen-v0210) | **breaking** | **breaking** | **breaking** | **breaking** |
| 15 | [The learned-route API is gone](#15-the-learned-route-api-is-gone-v0230) | **breaking** | **breaking** | **breaking** | **breaking** |
| 16 | [Replicated documents](#16-replicated-documents-are-available-11-and-in-groups-v0230) | new API, on by default | new API, on by default | new API, on by default | new API, on by default |

---

## 0. Before you ship: downgrade is not a rollback

The first launch on this release **moves** pre-split delivery state out of the
credential store into the app container and deletes the credential-store copy
once the move is durable. An older build reads the old location and finds none
of it.

A downgraded install therefore comes up with:

- an empty outbox (queued messages gone),
- an empty pending queue,
- an empty **block list** — *every previously blocked peer silently unblocked*.

Blocking is a safety control. Treat a downgrade as a decision to reset it, not
as an undo.

**What to do:**

1. Ship this as an explicitly breaking release in your own versioning.
2. Do not stage it behind a rollback plan that reverts the binary. Roll
   *forward* — keep a hotfix lane on top of this release instead.
3. If you run phased rollout, make sure your halt procedure is "stop promoting",
   not "roll back the previous version to devices that already updated".

---

## 1. `initialize_mls` now takes two storage providers

Secure key material and restartable protocol state are now two different
storage contracts with two different lifecycles.

| | `MlsStorageProvider` | `ProtocolStateStorageProvider` |
|---|---|---|
| Holds | MLS identity, sessions, groups, peer capability records, install secrets, the record-sealing key | Outbox, pending messages, session/Welcome lifecycles, peer snapshots, media descriptors, block list, Lamport clock |
| Backing store | Platform credential store (Keychain, Keystore-backed encrypted prefs, Secret Service) | **App container** — must be removed when the app is deleted |
| Value type | `sequence<u8>` (unchanged) | `bytes` (see [§2](#2-protocol-state-values-are-bytes-not-an-element-wise-sequence)) |

The one-provider API is gone, and so is the public `enable_message_persistence`
path — persistence is now wired by `initialize_mls` alone.

> **Why split at all?** Credential stores can outlive an app container. Before
> the split, uninstalling the app could leave message plaintext and cloud-media
> `encryption_key`/`iv` values in the Keychain with nothing that ever read or
> deleted them.

### 1.1 Rust

```rust
// Before
protocol.initialize_mls(storage)?;

// After
use offline_protocol::{MlsStorage, ProtocolStateStorage};

protocol.initialize_mls(
    secure_storage,          // Arc<dyn MlsStorage>
    protocol_state_storage,  // Arc<dyn ProtocolStateStorage>
)?;
```

New public exports from `offline_protocol`:

```rust
pub use protocol_state_storage::{
    ProtocolStateError, ProtocolStateResult, ProtocolStateStorage,
    MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES,  // 8 MiB
};
```

`ProtocolStateStorage` deliberately does **not** reuse the MLS crate's error
type — it returns `ProtocolStateResult<T>` / `ProtocolStateError`. The UniFFI
callback surface is unchanged (`ProtocolStateStorageProvider` still throws
`MlsStorageError`; an adapter maps it), so this only affects Rust
implementations.

### 1.2 Native Swift / Kotlin (UniFFI)

```swift
// Before
try mesh.initializeMls(storage: keychainStorage)

// After
try mesh.initializeMls(
    secureStorage: keychainStorage,
    protocolStateStorage: appContainerStorage
)
```

```kotlin
// Before
protocol.initializeMls(encryptedStorage)

// After
protocol.initializeMls(encryptedStorage, appContainerStorage)
```

You must regenerate bindings — see [§12](#12-build-and-packaging).

### 1.3 React Native

**No application code change.** The bridge constructs both providers itself and
`initializeMlsWithSecureStorage()` keeps its signature (as does the automatic
initialization inside `start()`).

Locations the bridge uses:

| Platform | Secure store | Protocol state |
|---|---|---|
| iOS | Keychain, per-account service suffix | `Application Support/…/protocol-state-v1`, `isExcludedFromBackup = true` |
| Android | `EncryptedSharedPreferences`, per-account prefs file | `noBackupFilesDir/offline-protocol/protocol-state-v1` |

What *does* change for RN apps is covered in [§8](#8-react-native-bridge-fallbacks-now-match-the-sdk-defaults),
[§9](#9-react-native-synthesized-relay-frames-must-be-unattributed),
[§10](#10-storage-is-now-isolated-per-app_id-user_id), and
[§11](#11-events-you-must-now-handle).

### 1.4 Python

`ProtocolManager` now requires an explicit state root. Python has no portable
uninstall-scoped container — `Application Support`, `LOCALAPPDATA`, and XDG data
directories commonly survive package removal — so the SDK refuses to guess one.

```python
# Before
pm = ProtocolManager(config, event_handler=print)

# After — pass state_root=...
pm = ProtocolManager(
    config,
    event_handler=print,
    state_root="/app/install-owned-data/offline-protocol",
)

# ...or set OFFLINE_PROTOCOL_STATE_ROOT in the environment.
```

Neither present raises:

```
protocol state has no safe process-wide default; pass root=... or set
OFFLINE_PROTOCOL_STATE_ROOT to an application-owned directory that is removed
when the application is uninstalled
```

**Your installer must remove that directory on uninstall.** That is the whole
contract the split exists to enforce; the SDK cannot enforce it for you.

Passing a custom `state_storage=` bypasses `state_root` entirely, and the custom
provider then owns both account isolation and uninstall cleanup.

Directories are created `0700` and an existing store is tightened on open.

Also note: the `keyring` credential store now holds
`protocol_state_record_key`, the per-install key that seals delivery state. On a
host where `keyring` resolves to a null or plaintext backend, that key sits in a
readable file — you get separation of *lifecycle* but not of *confidentiality*.
Install a real secret service (gnome-keyring, kwallet) anywhere that matters.

### 1.5 The custom-provider contract

If you supply your own `ProtocolStateStorageProvider`, these are obligations,
not suggestions. Each one exists because something breaks on a device without
it.

**Store bytes verbatim.** Sensitive categories arrive already sealed. Do not
inspect, re-encode, compress, or truncate.

**Never use a store that can outlive the app container.** Not Keychain, not
`EncryptedSharedPreferences` backed by a surviving Keystore namespace.

**Writes must be atomic *and* durable before `store` returns.** The SDK treats a
successful `store` as persisted and immediately writes state that depends on it
— most sharply the record-sealing key, after which sealed records start landing
in the container. A rename that commits ahead of its data blocks, or an
`apply()` that only staged in memory, can crash into a container full of records
whose key was never written. The built-in providers use `AtomicFile` on Android,
`commit()` for encrypted prefs, and fsync of the file *and* its parent directory
on iOS (`F_FULLFSYNC`) and Python — **including on delete**, since an unflushed
unlink can resurrect an entry the SDK already settled.

**`load` must bound its read.** Stat the entry first; never materialize or hand
back more than **8 MiB** (`MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES`). The SDK
refuses to write anything near that, so a larger entry is corrupt or tampered.
This cannot live in the SDK: by the time it can check a length, you have already
allocated the bytes. Keep `list_keys` bounded for the same reason.

**Report destruction as `Corrupted`, not as absence.** Three-state reads are
load-bearing:

| You return | SDK reads it as | Consequence |
|---|---|---|
| `Ok(None)` / `null` | nothing was ever here | nothing restored, nothing settled |
| `Corrupted` / `CorruptedData` | a record existed and is permanently gone | record dropped **and settled** — `message_failed` for an outbox entry, `pending_state_lost` for a pending queue |
| `LoadFailed` (or any other error) | this read failed, retry later | record left on disk, nothing settled |

Returning `null` for a record you destroyed is accepted, but it costs the
settlement: the app is told nothing and the message id it holds never resolves.
Reporting a *transient* failure as `Corrupted` is worse — the app is told a
message failed terminally and then the next launch delivers it.

`NotFound` from `load` **or** from `list_keys` is read as absence / empty
category, so a backend that can only spell emptiness that way is safe.
(UniFFI's `MlsStorageError.KeyNotFound` maps onto it.)

**Do not encode the key into a filename.** Key ids are peer and message ids, so
an encoding is case-unsafe (`AAG` and `AAa` are the same file on APFS's macOS
default and on Windows — one record silently overwrites the other) and unbounded
(a valid long id overruns the 255-byte `NAME_MAX`). Use a fixed-length lowercase
digest and put the exact key inside the record. The built-in providers share one
format across iOS, Android, and Python:

```
bytes 0..4   magic "OPS1"
bytes 4..6   key_type length, big-endian u16
bytes 6..8   key_id   length, big-endian u16
then         key_type UTF-8, key_id UTF-8, value bytes
```

**Serialize on a process-wide lock, not a per-instance one.** Two providers over
one root are not hypothetical — the RN bridge constructs a fresh one on every
`initializeMls` call. Per-instance locking lets one provider's stale-temporary
sweep unlink a temporary another's atomic write is about to rename into place.

**Sweep your own write temporaries.** A crash between "write temp" and "rename"
orphans a file that enumeration filters out, so nothing ever looks at it again
and it accumulates for the life of the install.

### 1.6 The custom-`MlsStorageProvider` upgrade trap

**If you ship your own `MlsStorageProvider`, upgrading installs will not inherit
their pre-split delivery state.**

The one-shot adoption sweep enumerates records through the
`MlsStorageProvider` it is handed. The built-in providers find pre-split records
because they read through to the pre-namespace store they replaced. A custom
provider has no such fallback, so the sweep finds nothing.

The state is not deleted — it is simply never picked up, and the install comes up
with an empty outbox, an empty pending queue, and an **empty block list**.

**Fix before shipping:** either have your provider read through to wherever your
previous version wrote, or migrate that data yourself before calling
`initialize_mls`. The key types to move are in the table below.

### 1.7 What lives where

Protocol-state key types (the closed set the sweep moves out of secure storage):

| Key type | Keyed by | Sealed? |
|---|---|---|
| `pending_message_entries` | message id | ✅ |
| `pending_messages` *(legacy, read-only)* | recipient | ✅ |
| `outbox` | message id | ✅ |
| `media_descriptors` | file id | ✅ |
| `session_states` | peer id | ❌ |
| `welcome_lifecycles` | peer id | ❌ |
| `peer_key_packages` | peer id | ❌ |
| `peer_capabilities` | peer id | ❌ |
| `blocked_users` | peer id | ❌ |
| `both_create_awaiting_decrypt` | peer id | ❌ |
| `lamport_clock` | `current` | ❌ |
| `protocol_state_adoption` | `v1` (sweep marker) | ❌ |

Stays in secure storage: all MLS/OpenMLS material,
`encryption_capable_peers`, `scrub_secret`, `nostr_signing_secret`, and `protocol_state_record_key`.

### 1.8 Confidentiality: what sealing does and does not cover

Sensitive record *values* are sealed with ChaCha20-Poly1305 under a per-install
key in secure storage (`protocol_state_record_key`), with each record's
associated data binding it to its `(key_type, key_id)` slot — so records cannot
be moved between peers or categories by anyone with container write access.

Sealing **fails closed**: with the key unavailable, those categories are not
persisted at all rather than written in the clear. Delivery still works from
memory; only crash recovery for them is lost. Records already on disk are left
alone, so a later launch that can read the key recovers them.

What sealing does **not** hide — and what you should now treat as exposed to
anyone who can read the app container of an unlocked device:

| In the clear | What it reveals |
|---|---|
| `blocked_users` | which peers you have blocked |
| `both_create_awaiting_decrypt` | which peers are mid-handshake |
| `session_states`, `welcome_lifecycles` | which peers you have sessions with, and their delivery state |
| `peer_key_packages`, `peer_capabilities` | which peers you have exchanged with |
| sealed categories' **keys** | which peers you have queued messages for, and their message ids |

And because unsealed categories carry no integrity protection, container *write*
access degrades safety and liveness controls, not just privacy: deleting a
`blocked_users` marker silently unblocks that peer; writing `Confirmed` into
`session_states` promotes a still-pending session. On stock iOS/Android the
container is app-private, so this matters on rooted/jailbroken devices. If your
threat model includes that, re-derive the block list from a source you trust —
sealing `blocked_users` is *not* the fix, because a block list that stops
persisting whenever the seal key is unavailable is a worse failure than a
readable one.

Note that **sealing protects against edits, not deletions**. The AEAD tag binds a
record to its slot, so a modified or relocated record will not open — but nothing
binds it to a version or to the set it belongs to, so removing a record, or
restoring an older copy of it, is undetectable. Two controls that used to depend
on a deletable record have been moved off it (see 1.10); if you build your own,
do not assume a sealed category can notice something going missing.

The full discussion lives in
[MLS Integration → Protocol-State Confidentiality](mls-integration.md#protocol-state-confidentiality).

### 1.9 `initialize_mls` failure modes moved in both directions

Handle a failed `initialize_mls` explicitly. It got **more** forgiving about
individual bad records and **less** forgiving about one specific thing.

| Situation | Before | Now |
|---|---|---|
| One record in `session_states` or `welcome_lifecycles` won't read or won't decode | **failed init**, on every launch, forever — and with `require_encryption` on by default that install could send nothing, with no in-app recovery | record dropped, restore continues; a session whose confirmation can't be read is re-bootstrapped as `Pending`, never `Confirmed` |
| A store reports a record `Corrupted` | left in place, re-read on every boot, never settled | dropped **and settled** |
| A transient persistence failure on the restore path (Welcome-lifecycle repair, missing-session bootstrap) | failed init | logged, continues; the repair holds in memory for the run |
| **A `blocked_users` listing failure** | swallowed — came up with an **empty block list** and told no one | **fails init and rolls back** |

That last row is the intentional new hard failure. A listing error is
indistinguishable from an empty store, so swallowing it means every blocked peer
silently unblocked from a transient error. Blocking is a safety control, so it
fails closed.

**What to do:** if `initialize_mls` (or RN's
`initializeMlsWithSecureStorage()` / the automatic init inside `start()`) fails,
do not proceed as if the SDK is usable and do not present the user as unblocked.
Surface it and retry — initialization is transactional, so a rolled-back attempt
leaves no partial state and a retry is safe.

### 1.10 The inbound plaintext gate no longer reads `session_states`

*New in `v0.19.0`.* Only affects `encryption.enabled = true` **with**
`requireEncryption: false` — the mixed-mode opt-out. Every other configuration is
unchanged.

The gate that rejects inbound cleartext used to ask whether a *confirmed* MLS
session existed with the claimed sender, which it answered from the
`session_states` protocol-state record. That record lives in the app container,
so deleting it made the peer look unconfirmed on the next launch and re-opened
the gate for them. It now asks whether the peer is **known to run MLS at all**,
sourced from the MLS session list and the durable encryption-capability records
— both in the credential store.

What changes in practice:

- Cleartext from a peer you hold an MLS session for, *or* who has signed a
  control message this install verified, is now
  rejected even when the session was never confirmed. No honest peer sends
  cleartext in that state (a sender with a pending session queues rather than
  downgrading), so this should only ever fire on an injection or a real
  downgrade.
- Capability is **not** forgotten when a session is torn down, because teardown
  can be triggered remotely by an injected frame. It is never forgotten at all
  (see §14): under derived addresses a peer that re-keys is a different address,
  so nothing learned about the old one stops being true.
- A peer that genuinely loses its MLS state — a failed `initialize_mls`, a
  reinstall — and then sends plaintext will have it rejected, with the usual
  once-per-peer `PLAINTEXT_RECEIVE_REJECTED` warning, and no delivery ACK, so
  they will retry. Under derived addresses that peer comes back as a *new
  address*, so it reaches you as a new contact rather than as a downgrade.
- Peers that have never shown any MLS signal are unaffected, so plaintext-only
  interop keeps working.
- Capability is learned from unauthenticated signals as well as authenticated
  ones: a well-formed `__MLS_WELCOME__` marks its sender even if the join fails,
  deliberately, so a peer whose handshake is breaking does not keep the gate
  open. The trade-off is that an injected frame naming a plaintext-only peer can
  mark that peer capable and suppress their cleartext for the rest of the run.
  It does not persist — a restart clears it, since restore seeds only from
  sessions and durable capability records. **If a legacy peer goes unreadable
  with `PLAINTEXT_RECEIVE_REJECTED` and you hold no session for them and have
  verified no signed control frame from them, that is the case you are looking
  at.** The set is
  also capped; past the cap new peers fall back to the old session-state check
  rather than displacing anyone already in it.

### 1.11 Key packages are checked against the peer's identity

*New in `v0.19.0`.* `peer_key_packages` is now a sealed category, and every use
of a key package is checked against the peer's identity.

> **Superseded by [§14](#14-your-identity-is-derived-not-chosen-v0210).**
> In `v0.19.0` the check compared the key package against a TOFU-*pinned*
> signature key. The pin store is gone; the check now re-derives the address
> from the key package's own signature key and compares it to the peer id the
> package arrived under. The observable behaviour below is unchanged — a key
> package that does not match its peer is still refused — but there is no pin
> to manage, reset, or lose, and the error code is `SENDER_ADDRESS_MISMATCH`
> rather than `TOFU_KEY_MISMATCH`.

- **Cached key packages written by earlier builds are unsealed and will be
  dropped on first launch after upgrade.** This is not an error and does not
  fail initialization; it costs one key-package re-exchange with those peers,
  which the SDK performs automatically on next contact.
- `mlsImportKeyPackage` now applies the same check, and returns an error if the
  package's leaf signature key does not correspond to that peer id. Apps
  driving the low-level MLS API against a *different* identity than the one the
  peer signed with will start seeing that error — which is the point, but it is
  a behaviour change for that entry point. The FFI signature is unchanged, so no
  bindings regeneration is needed.
- `inviteToGroup` now also verifies the invitee's key package identity, which
  that path previously did not check at all.

---

## 2. Protocol-state values are `bytes`, not an element-wise sequence

`ProtocolStateStorageProvider` uses `bytes`, so a custom provider receives and
returns:

| Language | Type |
|---|---|
| Kotlin | `ByteArray` |
| Swift | `Data` |
| Python | `bytes` |

`MlsStorageProvider` is **unchanged** — still `sequence<u8>` (`List<UByte>` /
`[UInt8]` / `list[int]`). It carries key material a few hundred bytes at a time,
where representation does not matter. Protocol-state records reach megabytes,
and `List<UByte>` boxes every element with no `valueOf` cache — on the order of
two million short-lived objects per call for a 2 MiB record, inbound *and*
outbound.

Python custom providers:

```python
class MyStateStorage(ProtocolStateStorageProvider):
    def store(self, key_type: str, key_id: str, data: bytes) -> None: ...
    def load(self, key_type: str, key_id: str) -> bytes | None: ...
    def delete(self, key_type: str, key_id: str) -> None: ...
    def list_keys(self, key_type: str) -> list[str]: ...
```

---

## 3. The three reliability config updaters are now fallible

`update_ack_config`, `update_retry_config`, and `update_dedup_config` all
return `Result` in Rust and are `[Throws=ProtocolError]` over UniFFI.

They now validate by building the candidate configuration and running the real
`ProtocolConfig::validate` on it, so a runtime update cannot install something
`OfflineProtocol::new` would have rejected. What they refuse:

- zero ACK timeout, zero max pending ACKs
- zero retry delays, non-finite or `< 1.0` backoff multiplier
- `outbox_max_lifetime_ms` or `pending_message_max_lifetime_ms` of `0`, or above `i64::MAX`
- zero `max_tracked_messages` or `retention_time_secs` (see [§4](#4-zero-max_tracked_messages--retention_time_secs-is-now-refused))

On rejection the **previous configuration is kept** — nothing is partially
applied.

```rust
// Rust
protocol.update_retry_config(retry_config)?;
```

```swift
// Swift — all three now need `try`
try mesh.updateAckConfig(config: ackConfig)
try mesh.updateRetryConfig(config: retryConfig)
try mesh.updateDedupConfig(config: dedupConfig)
```

Kotlin and Python surface the throw through their normal exception path; the
declaration change still requires regenerated bindings.

No `ProtocolError` variants were added — the append-only FFI error taxonomy is
unchanged. Rejections arrive as `InvalidConfiguration`.

**React Native:** no code change. The bridge already wraps all three in
try/catch. Two behaviours to know:

- A `reliability` block passed to the `OfflineProtocol` **constructor** is applied
  during `start()`, where a rejection is `console.warn`ed and swallowed — the SDK
  keeps its defaults. **A silently-defaulted reliability block looks like it
  worked.** Grep your logs for `Failed to apply … configuration`.
- A direct `protocol.updateDedupConfig(...)` call **rejects the promise**
  (`ERROR_CONFIG`). Handle it.

---

## 4. Zero `max_tracked_messages` / `retention_time_secs` is now refused

`ProtocolConfig::validate` now constrains the two dedup fields that are not
behind `use_bloom_filter` — which is what makes the change reachable from a
binding caller at all, since the UniFFI `DedupConfig` carries only those two.

Neither failed safe. At `max_tracked_messages == 0` the exact-match tracker
evicts on every insert, holding a single id, so duplicate suppression — a replay
defence — was effectively off for a configuration the SDK accepted in silence.
`retention_time_secs == 0` expires every entry immediately for the same result.

This rejects only the **degenerate** value. A floor of 1 is not a floor on how
well duplicates are suppressed (`1` behaves indistinguishably from `0`, and a
retention shorter than the link's retry backoff suppresses nothing either).
Sizing the window for your deployment stays your call.

**Two places this binds:**

1. **Runtime update** — `update_dedup_config` returns `InvalidConfiguration` and
   keeps the previous configuration. Both RN bridges read these straight from
   JSON, so an app passing `0` is affected.
2. **Construction (Rust only)** — `OfflineProtocol::new` now fails outright for a
   `ProtocolConfig` with either field at `0`. The FFI init surface carries no
   dedup fields, so no binding caller can hit this form.

**What to do:** grep your config for `maxTrackedMessages: 0` /
`retentionTimeSecs: 0` (and the snake_case forms) and pick real values. Defaults
are `1000` and `3600`.

---

## 5. Recipient tokens are validated at every outbound boundary

Every user-targeted send API now validates the recipient as a `UserId` **before
any queue, outbox, clock, or transport side effect**. An app-owned placeholder
can no longer become indefinitely-retried durable protocol state.

`UserId` rejects: empty strings, `.` and `..`, ASCII control characters, `/`,
`\`, **`:`**, and anything over 256 bytes.

**That `:` is the behavioural break.** These APIs previously accepted any
non-empty string:

- `send_presence_update`
- `send_typing_indicator`
- `send_read_receipt`
- `send_service_request`
- `respond_to_service_request`

So namespaced identifier forms that used to work now fail with
`InvalidArgument`:

```
unresolved:token      ❌
did:key:z6Mk…         ❌
npub:abc…             ❌
```

**Apps carrying such tokens must resolve them before calling the SDK.** There is
no compatibility mode — the whole point is that an unresolvable address cannot
enter durable state.

Full list of validating entry points:

| API | Notes |
|---|---|
| `send_message`, `send_message_with`, `send_message_rich` | |
| `forward_message` | |
| `send_media`, `send_media_with` | |
| `send_connection_request`, `accept_connection_request`, `reject_connection_request`, `cancel_connection_request` | |
| `send_presence_update`, `send_typing_indicator`, `send_read_receipt` | previously unvalidated |
| `send_service_request`, `respond_to_service_request` | previously unvalidated |
| `invite_to_group` | admission is where the gate belongs |

**Deliberately exempt:** `remove_from_group` and group role/admin mutation. A
gate there would turn "a member with a stale-format id is on the roster" into
"that member can never be removed or demoted" — the wrong direction for a
moderation control.

Restore also applies this: a persisted pending queue whose recipient does not
validate is settled with the reason
`Recipient is not a valid user ID; queued message cannot be delivered`. Expect
some of these on the first launch after upgrading if you ever queued to a
placeholder.

---

## 6. `send_message` rejects content over 256 KiB

`send_message`, `send_message_with`, and `forward_message` now fail with
`InvalidArgument` for content over **256 KiB**:

```
Message content too large: N bytes (max 262144); use send_media for large payloads
```

The cap sits at the send boundary rather than at transmit time because a message
waiting on MLS session establishment is queued — in memory *and* on disk — long
before it reaches the transport's own 1 MiB check, so a transmit-time cap would
never run for exactly the messages that accumulate. It sits well under 1 MiB to
leave room for MLS ciphertext expansion, base64, and the JSON wire envelope, so
anything accepted can actually be delivered.

**Large payloads belong on `send_media` / `sendMedia`**, which chunks and is not
subject to this limit.

**Group sends are exempt from this cap** — a group send encrypts to group state
that already exists, so it has no durable pre-session queue behind it and the
boundary check does not run. That is unchanged behaviour. (It is exempt from the
*cap*, not from delivery tracking: as of the per-member fan-out default, each
recipient's copy of a group message carries its own outbox entry, ACK, and retry
ladder. See [Group sends](message-delivery.md#group-sends).)

**What to do:** if your app can produce large text bodies (pasted logs, embedded
data URIs, JSON blobs in message content), add a length check at your composer
and route over media instead. Surface the rejection — it arrives as a thrown
error / rejected promise, not as a `message_failed` event.

---

## 7. The pre-session queue has a lifetime and hard caps

### New configurable field

| Field | Default |
|---|---|
| `pending_message_max_lifetime_ms` / `pendingMessageMaxLifetimeMs` | `604800000` (7 days) |

Mirrored through UniFFI (`RetryConfig`) and React Native (`RetryConfig` in
`types.ts`). An entry that exceeds it is removed from memory and from
protocol-state storage and settled with
`message_failed` / reason `Pending session lifetime exceeded`.

The lifetime is **absolute** — measured from when the message *first* entered
the queue. A flush that finds the session still unavailable carries the original
timestamp forward, so reconciliation cannot keep a message alive past its
window. The flush path also refuses to dispatch an entry past its deadline, so
an expired message can never settle `MessageSent`.

### Fixed, non-configurable caps

| Bound | Value | At capacity |
|---|---|---|
| Messages per peer | 64 | oldest settled `message_failed`, then the new message is admitted |
| Messages globally | 4096 | globally oldest settled the same way |
| Bytes per peer | 2 MiB | oldest evicted until the new message fits |
| Bytes globally | 16 MiB | globally oldest evicted until it fits |
| Single protocol-state record | 4 MiB | refused on write, dropped on read |

All four evictions emit `message_failed` with reason
`Pending session queue capacity exceeded`. Restore applies both budgets too, so
a record written by an older build cannot re-inflate memory on boot.

Byte bounds exist because an entry count alone bounds neither memory nor durable
storage — message content is application-supplied, so four very large messages
sit there reporting 4/64 and looking fine.

### Behaviour worth knowing

- Expiry work is scheduled from the earliest queued deadline instead of scanning
  the queue every 100 ms `process()` tick.
- Expiry is bounded to 64 entries per tick; whatever a pass leaves behind is
  still past its deadline and drains on the next tick. So a large batch of
  expiries settles over several ticks rather than in one.
- **A pre-split install can hold far more than these caps admit** (the pre-split
  build had no pending-queue caps at all). Expect a burst of capacity
  `message_failed` events on the first launch after upgrading. Make sure your UI
  can render that without looking like a mass send failure.

---

## 8. React Native: bridge fallbacks now match the SDK defaults

Two independent fields where the RN layers substituted their own value for an
omitted config field, and that value had drifted from the Rust default. Neither
affects an app that passes the field explicitly.

### 8.1 ACK timeout fallback: 5 s → 10 s

Both RN bridges substituted `5000` when `updateAckConfig` was called without
`defaultTimeoutMs`, silently halving the timeout against the SDK default. The
fallback is now `10000`, matching `DEFAULT_ACK_TIMEOUT_MS`.

Apps that omit it: ACK waits — and therefore retry timing — return to the
documented default. If you were relying on the 5 s behaviour, set it explicitly.

### 8.2 Pending-decryption TTL fallback: 2 min → 30 min

`DEFAULT_PENDING_TTL_MS` moved to 30 minutes when delivery ACKs became deferred
(see [§7](#7-the-pre-session-queue-has-a-lifetime-and-hard-caps) for the
*outbound* queue — this is the **inbound** pending-*decryption* queue, a
different one). Rust and UniFFI were updated; all three RN layers kept the old
`120000`, and because the JS wrapper materializes the field before it crosses
the bridge, an RN app that omitted `pendingQueue.pendingTtlMs` got 2 minutes no
matter what the SDK default said.

All three now use `1800000`. Apps that omit the field hold an
arrived-before-the-session-was-ready message for 30 minutes instead of 2,
which is the window the deferred-ACK model needs — that queue is the primary
recovery path before the session confirms, and a message evicted from it is not
delivered and was never ACKed. Memory is unchanged: the count caps (64 per peer,
4096 global) and byte caps (4 MiB / 32 MiB) still bound it, so a longer TTL lets
entries linger *within* those caps rather than raising the ceiling.

If you were relying on the 2-minute behaviour, set `pendingTtlMs` explicitly.

Drift tests now pin all of these bridge literals to the Rust constants, so they
cannot separate again.

---

## 9. React Native: synthesized relay frames must be unattributed

Only relevant if you wrote native code that calls `internetMessageReceived`, or
you synthesize relay answers yourself. The bundled bridges are already fixed.

Relay *answers* — `__GROUP_CREATED__`, `__GROUP_ERROR__`, and the
`__GROUP_INFO__` / `__USER_GROUPS__` snapshots — are synthesized locally from a
relay notification. No peer transmits them. The bridges were passing the literal
string `"relay"` as the FFI `sender_id` and stamping the frame
`requires_ack: true`.

Both are claims about a peer that does not exist, and the core acted on both:
`"relay"` was inserted into `known_peers`, emitted as `NeighborDiscovered`,
enrolled in service-discovery fan-out, sent an unsolicited key-package DM under
default `auto_key_exchange` — and, far more often, every injected frame produced
a delivery ACK addressed back to `relay`. All undeliverable, each drawing a relay
`DeliveryError` that re-armed the presence watch, which is why
`Presence check for relay: false (last seen: None)` never aged out.

**The rule:** `senderId` on `internetMessageReceived` asserts the peer is
*reachable*. It drives outbox flush, Welcome re-arm, auto key exchange, and
`neighbor_discovered`.

| Frame | `senderId` | `requires_ack` |
|---|---|---|
| Locally synthesized (no peer sent it) | `""` | `false` |
| Names a real relay-reported actor (a group message's `sender`, `added_by`, `removed_by`) | that id | unchanged |

Unattributed ingestion is a supported, tested mode of the reachability seam.
Never pass a placeholder id.

---

## 10. Storage is now isolated per `(app_id, user_id)`

Both built-in stores derive an opaque namespace — `account-<sha256 hex>` over a
domain-separated `(app_id, user_id)` — and use it as a path component and
credential-store suffix. Multiple accounts on one install can no longer share
keys or delivery state.

**Changing either `appId` or `userId` selects a fresh storage namespace.** If
your app treats `userId` as mutable (renames, re-registration), understand that
this now means "start from a fresh identity and an empty outbox".

Namespaces are validated on all three platforms before becoming a path — a
custom namespace must match `account-[0-9a-f]{64}` exactly.

### The legacy-store claim (upgrades only)

The pre-namespace store was shared by every account on the install, so **at most
one account may inherit it.** The first to launch writes a claim, reads it back
to verify, and then adopts by read-through: a miss in the namespaced store falls
through to the legacy store and promotes what it finds. `delete` removes the
legacy copy too, so a deleted key cannot be resurrected; if that removal fails,
the key is tombstoned in the namespaced store instead and read-through treats it
as absent from then on, so the guarantee holds either way. The whole
probe → claim → read-back sequence holds a process-wide lock, so two accounts
starting at once cannot both adopt.

An account that does **not** win the claim gets neither identity nor delivery
state: a fresh MLS identity, an empty outbox, an empty pending queue, and an
empty block list. That is reported, never silent:

| Platform | How it surfaces |
|---|---|
| React Native | `diagnostic` event, level `error` — `Legacy secure store belongs to another account…` or `Could not record this account's claim…` |
| Python | `SecureStorage(...).legacy_adoption`, plus a logged warning |

**What to do:** log and alert on those diagnostics. They mean a user lost their
sessions, groups, peer capability records, queued messages, and block list. If your app
supports multiple accounts on one device, decide which one should inherit — the
SDK's answer is "whichever launches first", which may not be yours.

Python callers building `SecureStorage` directly: pass a namespace, or you land
on the new service name, find nothing, and mint a fresh identity. A no-namespace
construction now warns. `adopt_legacy_store=False` opts out quietly, since that
is a decision rather than an accident.

Both adoption mechanisms — the secure-store read-through and the protocol-state
sweep — are one-shot upgrade scaffolding. The sweep is resumable (a crash leaves
the remainder for the next launch), non-destructive (a key already present in
protocol-state storage wins), and marked complete only when it finished without
a storage error.

### Logging out and switching accounts

Namespacing keeps accounts apart; it does not erase one when a user signs out.
Destroying the protocol instance releases memory and nothing else — the outbox,
the pending queue, the block list, and the whole MLS identity stay on disk under
that account's namespace. Two consequences are worth planning for:

- On the next sign-in as the same user, the restored outbox is **re-driven**.
  Undelivered messages are retried on every launch and reconnect until they
  expire (`outbox_max_lifetime_ms`, seven days by default) or exhaust their
  retries.
- On iOS the Keychain **outlives the app container**, so an uninstall does not
  take the secure store with it. A reinstall followed by a sign-in as the same
  user adopts that material again — identity, sessions, and, through the
  pre-split store, delivery state.

React Native applications can erase all of it (**added in `v0.18.2`**; on
`v0.17.0` this API does not exist):

```ts
await protocol.destroy();
await protocol.wipePersistedState(appId, userId);
```

**Order matters, and the identity is passed explicitly.** The protocol persists
as it works — outbox entries on the send path, pending snapshots, sealed state
records — so a wipe underneath a live instance races those writes; the native
side rejects the call if the account named is the one the current instance is
running. The identity is an argument because `destroy()` clears the config the
namespace would otherwise be derived from. Pass the same `appId`/`userId` the
protocol was created with; any other pair names a different account and wipes
nothing.

What it erases, for that account only:

| Store | iOS | Android |
|---|---|---|
| Namespaced secure store | Keychain service `<bundle>.mls.v2.<namespace>` | `mls_secure_storage_v2_<namespace>` |
| Namespaced protocol state | `Application Support/<bundle>/protocol-state-v1/<namespace>/` | `noBackupFilesDir/offline-protocol/protocol-state-v1/<namespace>/` |
| Pre-namespace secure store | Keychain service `<bundle>.mls` | `mls_secure_storage` |

The pre-namespace store is only erased when this account owns the claim or the
store is unclaimed. Another account's claim makes it off-limits, and a claim
that cannot be *read* also stops the wipe — unreadable and foreign are
indistinguishable, and only one of those two mistakes is recoverable. A claim
that is present but not decodable counts as another account's, for the same
reason: bytes this SDK cannot interpret are still evidence that something
claimed the store. The androidx master key is never touched: it is shared with
every other account's store.

Three things to know before wiring it in:

- **It rotates the account's MLS *and* Nostr identities.** Peers holding a
  session will see a desync on next contact and re-establish from a fresh key
  package. Because the identity key is what the address derives from, wiping it
  gives this device a **new address**: peers reach the old one and find nobody,
  and must be given the new one out of band. Read it back with `localAddress()`
  after the next `initializeMls`.
- **It is irreversible and it is not a "clear my messages" button.** There is no
  partial mode; the outbox, block list, and every group membership go together.
- **Retry on failure.** The wipe is idempotent, attempts every store even if one
  fails, and reports the first error. Secure storage goes first, so an
  interrupted wipe leaves protocol-state records as ciphertext whose key is
  already gone rather than as readable state.

Applications that supply **their own** storage providers must erase their own
containers — the SDK only knows how to wipe the built-in ones.

There is no equivalent API in the Python bindings. The state directory could be
removed trivially, but the secure store cannot be enumerated: `keyring` has no
listing operation, the SDK's per-`key_type` index has no index *of* key types,
and the MLS key-type set is open (OpenMLS contributes its own labels). A partial
wipe that left signing-identity material behind would be worse than none, so
Python callers should scope `SecureStorage` to a namespace they can drop
wholesale at the backend instead.

---

## 11. Events you must now handle

No event *types* were added — `MessageFailed` and `ConvergenceDiag` already
existed — but they now fire in new situations, and one of them can name a
message you never saw fail before.

### Install your event callback before `start()`

**Restore settlements are parked until `start()`, not emitted from
`initialize_mls`.** Apps routinely call `initialize_mls` before installing a
callback, so anything emitted there would be lost. `resume()` drains them too
(a `pause()`d app that shortens `pendingMessageMaxLifetimeMs` settles messages
while parked).

The parked queue is capped at 8192 settlements, keeping the oldest and reporting
the suppressed count when it drains.

### `message_failed` reasons

| Reason | Meaning |
|---|---|
| `Pending session queue capacity exceeded` | evicted by any of the four pending caps |
| `Pending session lifetime exceeded` | `pending_message_max_lifetime_ms` elapsed |
| `Outbox capacity exceeded` / `Outbox lifetime exceeded` | pre-existing |
| `Recipient is not a valid user ID; queued message cannot be delivered` | restored queue failed the new recipient validation |
| `Outbox entry from a previous version was too large to migrate` | pre-split record over the 4 MiB record cap |
| an unrecoverable outbox record | its record key *is* the message id, so the loss is named |

### `convergence_diag` with stage `pending_state_lost`

Emitted when a whole pending *queue* is unrecoverable. The message ids live
inside the record that would not open, so the `peer_id` is the most that can be
reported. `detail` carries the reason.

### The contract to internalize

- **A dropped record is reported, not swallowed.** Anything the app was told was
  queued gets settled when it cannot be recovered.
- **But only a record that is actually gone is settled.** A record that merely
  could not be read *this session* — seal key unavailable, one refused read —
  stays on disk and produces no event, because settling it would be a terminal
  answer the next launch overturns by restoring the entry and delivering it.
- Therefore: **do not treat a quiet startup as proof everything restored.** Treat
  `message_failed` as proof that something did not.
- A retry of a failed `initialize_mls` can settle the same id twice. A duplicate
  terminal event is deliberate — it is a far smaller lie than silence. Make your
  handler idempotent.

### 11.1 `messageDecryptionFailed` is advisory, not terminal *(v0.20.1)*

No new event type — but this one changes meaning, and an app that settles a
message on it now settles too early.

A receiver that cannot decrypt or parse an inbound frame used to drop it and
send a delivery ACK anyway, so the sender marked the message delivered and
stopped retrying: silent loss behind an ACK claiming the opposite. It now
**withholds the ACK**, which is what lets the sender's resend deliver — and for
a DM that resend is re-sealed against the peer's current session, so it carries
a live ratchet generation rather than replaying bytes that already failed.

The consequence for your event handler:

| Event | Before | Now |
|---|---|---|
| `messageDecryptionFailed` | effectively terminal — one per lost message | **advisory**, once per failed *attempt*, bounded by the sender's ACK retry budget |
| `messageFailed` | terminal | terminal — **settle here** |
| `fileReceiveFailed` | terminal | terminal — **settle here for media** |

**What to change.** If your UI marks a message lost, removes it from a list, or
resolves a promise on `messageDecryptionFailed`, move that to `messageFailed`
(or `fileReceiveFailed` for media). Treat `messageDecryptionFailed` as "this
attempt did not decrypt" — useful for diagnostics, and expect repeats for the
same message. Make the handler idempotent, as with the settlements above.

Media has no sender-side re-seal (chunks are re-encoded rather than replayed),
so an undecryptable chunk recovers the way an interrupted transfer already
does: the withheld ACK drives the media outbox to surface `MediaResendRequired`
and the app re-supplies the bytes.

**What did not change is the re-key.** Only a proven epoch mismatch tears down
and rebuilds a session. These failures withhold the ACK but never re-key —
turning every malformed frame into a session teardown would be an unbounded
churn vector. And **failures *after* a successful decrypt stay terminal and
still ACK** (an empty or non-UTF-8 plaintext, a decrypted media body that does
not parse): the generation is spent, so no resend could ever deliver.

All of the above sits under the existing `encryption.cryptoRecoveryEnabled`
kill switch (default on); setting it `false` restores the previous
drop-and-ACK. One related change is deliberately **not** under that switch: a
media chunk failing its identity binding is now answered with silence, matching
what the text path has always done, because it governs what the receiver
reveals to whoever injected the frame rather than whether anything can be
recovered. A repeated injection of the identical frame therefore re-emits
`MEDIA_SENDER_GROUP_MISMATCH` on each attempt instead of being suppressed by
dedup after the first — the rate is the signal, as with `SESSION_REKEY_TRIGGERED`.

### 11.2 `secure_session_failed` no longer implies the session is gone *(v0.21.0)*

The event has always meant "an establishment *attempt* failed", but until now
every path that fired it also left no live session, so tearing down session
state on it happened to work. `v0.21.0` adds the first counter-example: a
session Welcome refused for carrying an unprovable identity
([§14](#14-your-identity-is-derived-not-chosen-v0210)'s leaf binding) is
refused **non-destructively** — the Welcome is declined, a
`GROUP_LEAF_IDENTITY_UNPROVEN` security warning fires alongside it, and the
session you already had with that peer keeps working.

**What to change.** If your app clears conversation encryption state, resets a
"secure" indicator, or re-triggers establishment on `secure_session_failed`
alone, stop — the peer may still be reachable over the surviving session.
Settle session liveness on `secure_session_established` and actual send
results; treat `secure_session_failed` as a diagnostic about one attempt.


### 11.3 `relay_promoted` and `relay_demoted` can now actually fire *(v0.22.0)*

The SDK has carried a complete battery-aware relay policy for several releases:
DORS scores energy, the relay role promotes and demotes, and message forwarding
refuses to spend the last few percent of somebody's battery carrying other
people's traffic. All of it read the charge out of a per-transport metrics map
that nothing ever wrote to. The policy was complete and unreachable, so on every
real device the relay role was never evaluated, `relay_promoted` and
`relay_demoted` could not fire, and forwarding always took its "unknown battery
means willing" branch. The floor that exists to stop a dying phone relaying was
decorative.

`v0.22.0` lands the feed on the `TransportManager` and merges it into the two
snapshot loops that build the metrics map, so everything downstream of it comes
alive at once. Three consequences, none of which changes a signature:

- **Nothing happens until your app supplies the feed.** Call
  `setBatteryState(level, isCharging)`. Prefer it over `setBatteryLevel(level)`:
  a plugged-in device is deliberately excused the soft floor, and reporting the
  level alone strips relay duty from exactly the devices that should keep it. An
  app that calls neither gets the same nothing it got before, now documented
  rather than accidental.
- **Once the feed arrives, forwarding can stop.** A device under the floor now
  declines to carry other people's frames, which it never did before. That is
  the floor working rather than a regression, and it is the whole point of
  reporting the level.
- **React Native only: `allowRelay` and `minBatteryForRelay` were ornamental and
  now take effect.** Both crossed the bridge and were then parsed by nothing,
  because only `relayPriority` was read. An app that set `allowRelay: false` and
  watched the device relay anyway will now see it stop.

**What to change.** Subscribe a platform battery observer (`BatteryManager` on
Android, `UIDevice` on iOS) and push into `setBatteryState`; this release ships
the API, not the subscription. Then re-read any `relay` config you set while it
was inert, because those values now decide behaviour instead of being recorded
and ignored.
---

## 12. Build and packaging

### 12.1 React Native iOS: delete your manual `pod 'MeshSdk'` line *(v0.20.0)*

**This is the one change in this document that can stop an iOS build outright,
and it is the only step required.** Open `ios/Podfile` and delete:

1. Any `pod 'MeshSdk', ...` line, with or without `:modular_headers => true`.
2. Any `post_install` hook that configured `MeshSdk` — in particular one setting
   `DEFINES_MODULE`, `SWIFT_INCLUDE_PATHS`, `LIBRARY_SEARCH_PATHS`, or
   `OTHER_LDFLAGS` for it. The podspec now sets what is needed; a hook naming
   `$(PODS_TARGET_SRCROOT)/Generated` points at a path that no longer exists.
3. Any flag naming `offline_protocol_uniffi_sim` or `offline_protocol_uniffi_device`.
   Those two archives are gone, and linking them fails with *library not found*.

Then `pod install`. Its output should list `MeshSdk` under "Auto-linking React
Native modules".

If a `pod 'MeshSdk', :path => '.../mesh-sdk/ios'` line survives the upgrade,
`pod install` fails immediately with a "no podspec found" error naming that
path — loud, not silent. Deleting the line is the fix.

**What changed and why.** Two packaging defects were fixed together:

- **iOS autolinking is enabled.** It was off because the podspec lived in the
  package's `ios/` directory, and React Native resolves a dependency's podspec by
  globbing `*.podspec` in the package root only, without recursing — so
  autolinking could never see it and every consumer had to declare the pod by
  hand. `MeshSdk.podspec` now sits at the package root.
- **The native binary is an XCFramework.** It previously shipped as two loose
  archives (`..._device.a`, `..._sim.a`) declared through `vendored_libraries`,
  which made CocoaPods emit an unconditional `-l` for *both* into the app's link
  line. Simulator builds then tried to link the device archive and failed on the
  architecture mismatch. The podspec's own sdk-conditional `OTHER_LDFLAGS` could
  not prevent this: they sat in `pod_target_xcconfig`, and the flags that matter
  are on the app target, which a podspec cannot reach. Slice selection is now
  CocoaPods' job, and it gets it right without help.

**Simulator builds work.** If you had concluded this SDK was device-only, that
was this bug. Both slices have always shipped in the npm package — the publish
gate refuses to publish without the simulator slice — but the linker flags made
the simulator one unusable. No workaround is needed now, and any workaround you
carried must be removed per the list above.

Nothing else changes: same package name, same import, no JS/TS API change, no
wire-format change, Android untouched.

### 12.2 Regenerate UniFFI bindings

**Regenerate UniFFI bindings.** The UDL changed: a new
`ProtocolStateStorageProvider` callback interface, the two-argument
`initialize_mls`, `[Throws=ProtocolError]` on the three config updaters, and
`pending_message_max_lifetime_ms` on `RetryConfig`.

```bash
./scripts/generate-bindings.sh   # after UDL changes — Swift, Kotlin, Python

cd bindings/react-native
npm run build:uniffi:all         # or :ios / :android
```

Requires `uniffi` CLI `0.30.0` matching the workspace pin.

**React Native native rebuild required** — new source files are compiled in:

| Platform | New files |
|---|---|
| iOS (`MeshSdk.podspec`) | `ProtocolStateStorage.swift`, `StorageNamespace.swift`, `LegacyStoreAdoption.swift` |
| Android | `ProtocolStateStorage.kt`, `StorageNamespace.kt`, `LegacyStoreAdoption.kt` |

Run `pod install` for iOS. A JS-only update will not pick these up.

**Android test dependency:** `org.robolectric:robolectric:4.16` was added to
`testImplementation`. The `build.gradle` react-native detection also now checks
for `react-native/android/` rather than just the package directory, so newer RN
packages no longer resolve to an unversioned `react-android` dependency.

Events cross the FFI as JSON strings, so **event field changes need no bindings
regeneration** — only `bindings/react-native/src/types.ts`, which can drift.

---

## 13. Upgrade test checklist

Run these against a build of your **previous** version, then upgrade in place.

**Migration correctness**

- [ ] An install with queued (undelivered) messages still has them after upgrade,
      and they deliver.
- [ ] An install with blocked peers still has them blocked after upgrade. *(This
      is the sharpest failure mode — verify it explicitly.)*
- [ ] MLS identity survives *within a release line*: existing sessions still
      decrypt and existing groups still work. **This does not hold across
      [§14](#14-your-identity-is-derived-not-chosen-v0210)** — the identity
      becomes the MLS credential there, so every session and group from a
      pre-§14 build is invalidated by design. Upgrading across §14, verify the
      opposite: that peers re-establish cleanly from fresh key packages rather
      than appearing stuck.
- [ ] Multi-account installs: exactly one account inherits; the others log the
      `error` diagnostic and start clean without crashing.
- [ ] Uninstall removes protocol state (Python: verify your installer removes
      `state_root`).
- [ ] Kill the app mid-first-launch, relaunch: the sweep resumes and converges.
- [ ] A failed `initialize_mls` is surfaced and retried, not treated as success
      ([§1.9](#19-initialize_mls-failure-modes-moved-in-both-directions)).

**New rejections**

- [ ] No code path sends to a `:`-containing recipient (`unresolved:`, `did:`,
      `npub:`) — including presence, typing, read receipts, and service
      requests.
- [ ] Oversized text sends are caught at your composer, not surfaced as an
      opaque error.
- [ ] No reliability config passes `0` for `maxTrackedMessages` or
      `retentionTimeSecs`.
- [ ] Swift: all three `update*Config` calls compile with `try` and handle the
      throw.

**Events**

- [ ] Event callback is installed **before** `start()`.
- [ ] `message_failed` handler is idempotent and can render a burst without
      looking like a mass failure.
- [ ] `convergence_diag` / `pending_state_lost` is at least logged.
- [ ] RN: `Failed to apply … configuration` warnings are surfaced, not buried.

**Custom providers only**

- [ ] `store` is durable before it returns (test with a forced power loss or an
      fsync-counting fake).
- [ ] `load` refuses over 8 MiB without allocating it.
- [ ] Destroyed records report `CorruptedData`; transient failures report
      `LoadFailed`.
- [ ] Entries are addressed by digest, not by an encoding of the key.
- [ ] A custom `MlsStorageProvider` reads through to your previous location, or
      you migrated the pre-split key types yourself
      ([§1.6](#16-the-custom-mlsstorageprovider-upgrade-trap)).

---

## 14. Your identity is derived, not chosen (v0.21.0)

**`ProtocolConfig.userId` is gone.** It is replaced by `profile`, and the two
are not the same thing wearing a new name.

`userId` used to be your identity on every surface at once: the app picked a
string and that string *was* the device — its `sender`, its `recipient`, its
`peer_id`, and the thing peers trusted. Nothing authenticated it, so
impersonating someone cost typing their name.

Now the device holds an Ed25519 identity key and its address is the hash of
that key, rendered `off1…`. Peers verify an address by re-deriving it from the
key its owner presents, so claiming one you do not hold a key for is not
possible rather than merely discouraged.

`profile` is what is left of the old field's *other* job: choosing which stored
identity this instance runs as. It never leaves the device.

```diff
  const protocol = new OfflineProtocol({
    appId: 'my-app',
-   userId: currentUserId,
+   profile: currentUserId,
  });
```

Keeping the same string in `profile` that you passed as `userId` keeps you in
the same storage namespace — the namespace is still
`SHA-256(domain ‖ 0x00 ‖ appId ‖ 0x00 ‖ <that string>)`, unchanged. An app with
one account per install can pass a constant like `'default'`.

### Read your address instead of assuming it

```ts
const address = await protocol.localAddress();   // "off1q..." or null
```

It is `null` until startup completes, because the key that defines it lives in
storage that is not open before then. The `identity_ready` event carries the
same value at the moment it becomes known:

```ts
protocol.on('identity_ready', ({ address }) => setMyAddress(address));
```

Equivalents: `local_address()` in Rust and Python, `localAddress()` over
UniFFI.

### What this breaks in your app

**Read this before you re-key anything.** The single most common way to get
this migration wrong is to conclude "our user id changed, so everything keyed
by it must change". That is half right, and the wrong half destroys data.

The migration splits in two, and only one side moves:

| | Keyed by | Changes? |
|---|---|---|
| **Your own** storage — your local DB filename, MMKV/UserDefaults namespace, cache directories, the SDK storage namespace | the string you pass as `profile` | **No.** Keep passing the same string you passed as `userId`. |
| **Peer** identity — `recipient`, conversation keys, contact rows, group rosters | the peer's `off1…` address | **Yes.** |

`profile` never goes on the wire and no peer ever sees it. Passing your old
`userId` through unchanged is not a migration shim — it is the intended use,
and it keeps every namespace you derived from that same string intact.

With that split in mind:

- **Sending.** `recipient` must be a peer's address. A username reaches nobody.
- **Comparing.** "Is this message mine?" compares against `localAddress()`, not
  against the profile.
- **Storing peer-keyed rows.** Conversation rows, contact records, and group
  membership keyed by a *peer's* username must be re-keyed by that peer's
  address. There is no mapping from the old peer ids to the new ones — those
  identities genuinely changed.
- **Storing self-keyed state.** Leave it alone. See the two traps below.
- **Displaying.** An `off1…` string is not a name. Keep your own display names
  (the `senderName` / `accepterName` fields already carry them) and treat the
  address the way you would a phone number: the thing you route on, not the
  thing you show.

### Two ways this fails silently

Both of these were hit by a real app during this migration. Neither throws,
neither logs, and both look like a successful launch.

**1. A per-user database or namespace opened under the new id.** Code shaped
like this is extremely common:

```ts
const db = open(`db-${userId}.sqlite`);       // or `user-${userId}` for MMKV
```

An `off1…` address passes the usual identifier validation (it is plain
lowercase alphanumerics), so this **opens a different, empty database**. No
error, no fallback — the entire message history simply stops existing, and a
fresh empty store looks exactly like a first launch.

*Fix:* feed these the value you pass as `profile`, not the address.

**2. A teardown/switch check that reads the id change as an account switch.**

```ts
const shouldWipe = nextUserId !== tornDownUserId;   // now true for everyone
```

If an id-changed comparison gates wiping SDK state, upgrading looks like every
user switched accounts, and the app wipes its own MLS sessions on first launch.

*Fix:* compare profiles, which do not change.

The general rule: **if a string was doing double duty as "who I am" and "which
storage is mine", the second job stays with `profile`.** Only the first job
moves to the address.

### Cold contact by username

Reaching someone by typing their username was only ever possible because
usernames were addresses. Two paths give it back, and they are not equivalent.

**Invite/QR is the primary path, and permanent.** `createInvite()` produces a
compact blob carrying `{address, pubkey, petname?, sig?}`, and `parseInvite()`
verifies it offline: `derive_address(pubkey) == address` is checked at scan,
before `create()`. Nothing about this is transitional — the out-of-band
confirmation a scanned code represents is what makes the directory below safe.

**A signed username directory is additive, and off by default.** Setting
`transports.nostr.usernameDiscoveryEnabled` publishes a record binding this
install's profile to its address and enables `resolveUsername()`. It is
deliberately opt-in: publishing binds a human-readable name to an address in a
public place, where the mapping *is* the payload.

The directory is **not authoritative** — anyone may claim any name — so a
resolution returns the whole set of claimants in one `username_resolved` event
and a human must choose. Do not auto-select, and store the address rather than
the name. See [docs/spec/username-discovery.md](spec/username-discovery.md).

**If you already run an account system, do not wait for that layer.** A serverless
discovery record is the right design for peers with no infrastructure, and the
wrong one for an app that already has authenticated accounts and unique
usernames — it is squattable by construction, where your own directory is
authoritative. Binding an address to an account needs no new SDK surface; the
four primitives already ship:

1. Client calls `getIdentityPublicKey()` and `localAddress()`.
2. Client signs a server-issued nonce with `signData(nonce)`.
3. Server checks the signature and that `deriveAddress(publicKey) === address`.
4. Server stores the address on the account row and serves it from its existing
   user lookup.

That gives reach-by-username with your own uniqueness guarantees, and it stays
the better answer now that the discovery layer has shipped: yours is
authoritative and unsquattable, and the serverless directory by design is
neither.

### There is no in-place migration, and that is deliberate

Changing the identity changes the MLS credential, which invalidates every
existing session regardless of anything else — and 1:1 session slots are named
after the two ids, which OpenMLS cannot rename. So this release starts a fresh
identity world: old sessions do not carry over, and peers on an older build
fail cleanly at credential verification rather than half-working.

Old containers are left untouched rather than deleted, so nothing is destroyed
on upgrade. Clean them up when you are confident, by passing the *old* user id
where `wipePersistedState` now expects a profile:

```ts
await protocol.wipePersistedState(appId, oldUserId);
```

### Relay-backed deployments

Relay accounts, JWTs, and group rosters move into address space with the
server-side change; a build that sends address-shaped ids to a relay that still
expects usernames will fail its identity check. Sequence the relay first.

### Trust-on-first-use is gone

Deriving the identity from the key makes the pin store redundant, so it is
deleted. There is no `resetTofuForPeer` and no `tofu_reset` event, and the
`TOFU_KEY_MISMATCH` / `TOFU_STORE_FULL` / `SIGNATURE_DOWNGRADE` warning codes no
longer exist.

**What replaces them.** A control message is accepted only if it carries an
Ed25519 signature *and* the key that produced it re-derives to the address in
`sender`. That check has no first-contact window — the pin store's weakest
point, where whoever claimed a name first became its owner — so impersonation
goes from winning a race to finding a 160-bit second preimage (~2^160). That is
the cost of aiming at an address that already exists. The birthday bound on the
same truncation is ~2^80, which produces two keys sharing one address rather
than a chosen peer's address — a deliberate trade against BLE frame budget,
documented on `Address::HASH_LEN`.

**Three behaviour changes to plan for:**

1. **Unsigned control frames are refused, always.** Previously they were
   accepted from any peer without a pin, and refused only from peers with one
   (or under `requireTransportIdentity`). Now every security-gated control
   prefix must be signed. In practice this means **`initializeMls` is
   mandatory** — an instance that never initializes has no identity key, so its
   control traffic is dropped by every peer. The relay server's own answers
   (`__GROUP_CREATED__`, `__GROUP_MEMBER_ADDED__`, `__GROUP_MEMBER_REMOVED__`,
   `__GROUP_INFO__`, `__USER_GROUPS__`, `__GROUP_ERROR__`) are exempt, because
   no peer signs them; that exemption is narrow — relay ingest only, and only
   for frames the relay did not attribute to a peer.
2. **`SENDER_ADDRESS_MISMATCH` replaces `TOFU_KEY_MISMATCH`, and means something
   stronger.** The old code was ambiguous: a peer who reinstalled looked exactly
   like an impersonator, which is why a reset action had to exist. The new one
   is not ambiguous. A peer who reinstalls gets a new key and therefore a **new
   address**, arriving as a new contact. If you see this code, the frame was
   signed by someone who is not who they say they are. Surface it as such; do
   not offer a "trust anyway" affordance.
3. **Relay-native member-removal reconciliation is now inert.** A
   `__GROUP_MEMBER_REMOVED__` frame is authorized off its *wire* `sender`, which
   must be a group admin. The relay's own answer is injected unattributed (that
   is what the exemption above requires), so its placeholder sender can never be
   an admin: the frame is dropped and **no `group_member_removed` event fires**.
   Before this release the bridges passed the relay-reported `removed_by` as the
   sender, so the reconciliation could take effect for an admin no pin had been
   established for yet.

   The path the SDK itself uses is unchanged and still works: the removing admin
   sends a signed `__GROUP_MEMBER_REMOVED__` directly to the removed member
   (`removeMember`). **If your app removes members by calling the relay
   directly**, the other members will no longer see the roster update — drive
   removals through the SDK instead, or wait for the follow-up that moves relay
   answers onto dedicated FFI entry points (the same work that closes the
   exemption's residual). `__GROUP_MEMBER_ADDED__` is unaffected: its handler
   reads the payload and runs no sender check.

`requireTransportIdentity` keeps its `false` default and no longer gates the
signature requirement. Its one remaining effect is to reject control frames that
arrive with no transport peer identity at all — which on a deployment running
Nostr or sender-less relay delivery rejects their entire control plane. Leave it
off unless you run neither.

---

## 15. The learned-route API is gone (v0.23.0)

Eight methods and four types were removed from every binding:

| Removed | Surface |
|---|---|
| `learnRoute`, `getBestRoute`, `getAllRoutes`, `hasRoute`, `removeNeighborRoutes`, `cleanupExpiredRoutes`, `getRoutingStats`, `updateRoutingConfig` | UniFFI, React Native (TS/Kotlin/Swift), Python |
| `RouteEntry`, `RoutingStats`, `GradientRoutingConfig`, `PathConfig` | dictionaries and their TypeScript interfaces |
| `ProtocolConfig.path` (`forwardToTopK`, `maxCongestionLevel`) | config, every language |

**Delete your calls; there is nothing to replace them with.** The table they
read and wrote was never consulted by the delivery path: forwarding chooses
among the neighbors a device can address at that moment, and always did. A
route recorded through `learnRoute` changed nothing, and `getBestRoute` answered
from a table no frame ever followed. Keeping the surface implied a routing layer
that did not exist.

If you passed a `path` section in your config, drop it. It was parsed and
carried to the engine, which never read it.

Applications that never called these methods (the expected case, since nothing
in the delivery path depended on them) need no changes beyond removing the
config section if present.

---

## 16. Replicated documents are available, 1:1 and in groups *(v0.23.0)*

A new `DataStore` object ships on every binding: offline-first documents any
member of a space can edit while disconnected, merging deterministically when
replicas meet again. Messaging is synced events; this is synced state. Two
peers with a secure session converge on the documents they share, and so does
every member of a group.

**Existing applications need no changes.** `data.enabled` defaults to `true`,
but nothing is persisted until a document is written, nothing is sent until a
document is shared, and no existing API changed shape. An application that
never opens a store pays nothing at rest and nothing on the wire; set
`data.enabled` to `false` if you would rather the layer refuse outright. Skip
the rest of this section if you are not using it.

### What replicates, and with whom

A space replicates with the scope its name refers to:

| Space name | Replicates with |
|---|---|
| A peer's address | That peer |
| A group id (`groupId` from `createGroup`) | Every member of that group |
| Anything else | Nobody; it stays on the device |

Nothing else needs configuring, and there is no sharing call: the space name
*is* the sharing decision.

A group space uses the group's own roster, so adding or removing a member
changes who replicates without any second membership list to maintain. A
change is encrypted once for the whole group rather than once per member.

**A group replicates only when every member is running a build that supports
it.** Group replication is negotiated separately from 1:1, because an install
shipping only the 1:1 layer would display a group replication frame to its
user as raw text. A single member on such a build stops the group's documents
replicating for everyone until they upgrade, and the SDK will keep probing
them. 1:1 spaces are unaffected. There is no event for this today; if a
group's documents are not converging, check that every member is on a current
build.

A document too large to catch up inside a single frame is reported rather than
sent.

Replicas that stay in contact converge. Two that are separated by a partition
outlasting a compaction may not, because compaction deletes the history a
change made on the other side depends on. Nothing is lost on either device and
nothing crashes; the documents simply stay apart, and the refusal is logged.

**Deleting a document does not delete it from the peer, and does not keep it
deleted here.** There are no deletion tombstones in this release. `deleteDoc`
removes the records on this device, and the peer's next version offer names the
document again, so it is recreated and refilled from their copy. In a space
named after a peer, treat deletion as local cleanup that replication may undo,
not as a way to remove content: to retire content from both sides, empty the
document (deletions inside a document replicate like any other change); to stop
a space replicating at all, use a space name that is neither a peer address
nor a group id. The
same is true of `wipeAll()` on a running engine, for the same missing
tombstone: see the storage section below.

### Turning it on

```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  profile: 'default',
  data: { enabled: true },
});
await protocol.start();

const store = new DataStore();
await store.mapSet('space-1', 'profile', 'fields', 'name', {
  kind: 'text',
  value: 'Ada',
});
await store.flush('space-1', 'profile');
```

`enabled` defaults to `true`, so the block above is showing you the flag
rather than requiring it. It was `false` while the layer could store documents
but not replicate them, because advertising a capability with no sync behind it
invites peers to expect a sync that never comes; both halves ship together, so
the switch is on.

### Storage: nothing to configure, but one thing to know

Documents live wherever protocol state already does, so there is no setup.
If you point them somewhere else — `DataStore.withStorage(protocol, provider)`,
or `DataConfig::storage` in Rust — then **`wipePersistedState()` no longer
covers your documents.** It clears the account directory of the *default*
provider, which your backend is not inside. Call `DataStore.wipeAll()` on
logout as well, or documents outlive the account that created them. `wipeAll()`
throws if the backend refused any delete, and that error is the only signal
that records survived the wipe: treat it as a failed logout rather than
logging it, because nothing inside the application will show the difference.

**A wipe is only durable once replication has stopped.** It is the same
missing tombstone described above: nothing distinguishes a space this device
wiped from one it has never seen. Called while the engine is still running
with live sessions, every document comes back, from both directions. The
peer's next version offer names the documents and they are recreated and
refilled from the peer's copy, and an offer of our own naming nothing reads to
the peer as a replica that has never seen the space, which it answers with all
of it. There is no error and no event; the documents simply return. On the
logout path this does not bite, because the engine is being torn down anyway.
Anywhere else, stop it before wiping, and read `wipeAll()` as clearing this
device rather than as deleting content.

Verify any custom backend with `runStorageConformance(provider)`: an empty
`failures` array is the definition of supported. Reference adapters live in
[`examples/storage-adapters/`](../examples/storage-adapters/README.md).

Two things to know about the seam. Sealing covers record **values**, not
record **keys**: a backend sees `{space}/{doc}`, so treat space and document
names as metadata and do not put secrets in them. And switching backends
while documents are open migrates each open document into the new backend
before it returns, so if the migration cannot be written the switch fails and
nothing moves.

### Four new error codes

`DataDisabled`, `DataStorageUnavailable`, `DocTooLarge` and `DataCorrupted`
are appended to the error enum. Appended, so existing codes keep their
positions and no existing mapping changes — but a `switch` with an exhaustive
default may now reach it.

`DocTooLarge` is the one worth handling deliberately: a document is capped at
1 MiB compacted, with a `data_doc_size_warning` event at 768 KiB. When the cap
is passed the breaching change **is still durable** (refusing it would lose
work the user believed they had made), and deletions keep working while growth
is refused. So the recovery is: hear the warning, prune before the cap; and if
you hit it, delete content and the document resumes accepting edits.

### Two new events

`data_changed` and `data_doc_size_warning`. Both are ordinary events with no
one-shot semantics, so a listener that ignores unknown types needs no change.
`data_changed` fires **after** the change is durable, so a UI that re-renders
on it is rendering state that survives a restart.

### Rust consumers who only want messaging

The CRDT engine adds roughly 1.5 MB. Native crates.io consumers can drop it:

```toml
offline-protocol = { version = "0.23", default-features = false }
```

The mobile artifact carries it either way: two binding flavors would mean a
runtime FFI checksum mismatch rather than a build error, which is a worse
failure than the bytes.

---

## Appendix A: limits reference

| Limit | Value | Where enforced |
|---|---|---|
| Message content | 256 KiB | `send_message*`, `forward_message` → `InvalidArgument` |
| Rich extras (serialized) | 32 KiB | `send_message_with` boundary |
| Recipient / app id length | 256 bytes | `UserId` / `AppId` |
| Pending queue, per peer | 64 messages / 2 MiB | eviction + `message_failed` |
| Pending queue, global | 4096 messages / 16 MiB | eviction + `message_failed` |
| Pending lifetime | 7 days (configurable) | `pending_message_max_lifetime_ms` |
| Outbox lifetime | 7 days (configurable) | `outbox_max_lifetime_ms` |
| One replicated document | 1 MiB compacted | `DocTooLarge` at commit, warning event at 768 KiB |
| One value in a document | 1 MiB | `InvalidArgument` at the operation |
| One protocol-state record | 4 MiB | refused on write, dropped on read |
| Provider transfer ceiling | 8 MiB | `MAX_PROTOCOL_STATE_RECORD_TRANSFER_BYTES`, enforced *inside* each provider |
| Transport frame | 1 MiB | transport layer (unchanged) |

The 4 MiB / 8 MiB relationship is deliberate — the provider ceiling is a
superset of core's cap plus the seal envelope, and both halves are pinned by
tests (`bounded_load_ceiling_is_a_superset_of_the_record_cap` and
`built_in_providers_mirror_the_transfer_ceiling`, which reads the three provider
source files and asserts their literals).

## Appendix B: error mapping

| Situation | Rust | UniFFI / RN |
|---|---|---|
| Recipient not a valid `UserId` | `Error::InvalidArgument` | `ProtocolError.InvalidArgument` |
| Content over 256 KiB | `Error::InvalidArgument` | `ProtocolError.InvalidArgument` |
| Rejected reliability config | `Error::InvalidConfiguration` | `ProtocolError.InvalidConfiguration` |
| Provider destroyed a record | `ProtocolStateError::Corrupted` | `MlsStorageError.CorruptedData` |
| Provider read failed transiently | `ProtocolStateError::LoadFailed` | `MlsStorageError.LoadFailed` |
| Provider cannot express absence | `ProtocolStateError::NotFound` | `MlsStorageError.KeyNotFound` |
| Data layer off in config | `Error::DataDisabled` | `ProtocolError.DataDisabled` |
| `initialize_mls` has not run | `Error::DataStorageUnavailable` | `ProtocolError.DataStorageUnavailable` |
| Document past the 1 MiB cap | `Error::DocTooLarge` | `ProtocolError.DocTooLarge` |
| Document bytes will not decode | `Error::DataCorrupted` | `ProtocolError.DataCorrupted` |
| Bad space / document / collection name | `Error::InvalidArgument` | `ProtocolError.InvalidArgument` |
| Single value over the 1 MiB value limit | `Error::InvalidArgument` | `ProtocolError.InvalidArgument` |

Four `ProtocolError` variants were **appended** this release for the data layer
(`DataDisabled`, `DataStorageUnavailable`, `DocTooLarge`, `DataCorrupted`). The
taxonomy is append-only, so every code that existed before keeps the
discriminant it shipped with and no existing mapping changes.

---

## See also

- [CHANGELOG](../CHANGELOG.md) — the full entry for this release, with the
  reasoning behind each fix
- [MLS Integration](mls-integration.md) — provider contracts, custom storage,
  protocol-state confidentiality
- [Message Delivery](message-delivery.md) — queue bounds, lifetimes, flush
  triggers
- [Configuration](configuration.md) — every parameter, including the fixed
  message-plane limits
- [API Reference](api-reference.md) — `send_message` boundary rules,
  `initialize_mls`
