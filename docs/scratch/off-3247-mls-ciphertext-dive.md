# OFF-3247: MLS `INVALID_CIPHERTEXT` on receiver during 2-phone federated training

Diagnosis-only. No code fix in this branch. Links back to the Linear ticket at
the end.

## Symptom (reproduced in a 2-phone session, 2026-10-19)

- iPhone 12 (coord, `off1q87ryjpw5...`) + Pixel/Seeker (worker, `off1q9hh4wgh7...`),
  both running mesh-sdk `0.29.0`.
- 20+ minute session, 8 rounds of federated training.
- On iPhone's side, 128 `message_decryption_failed` events landed for frames
  originating from Seeker:

  ```json
  {
    "type": "message_decryption_failed",
    "message_id": "<fresh uuid each time, not a retransmission>",
    "sender": "off1q9hh4wgh7y0u0m5wfskengfq5krqmmp67udwalwf",
    "code": "INVALID_CIPHERTEXT",
    "reason": "Failed to decrypt MLS message (InvalidCiphertext); not delivery-ACKed, so the sender can re-seal it",
    "sealed_at_ms": <timestamp>
  }
  ```

- Zero `group_epoch_fork_detected`, `secure_session_failed`, or `group_error`
  events for the full run. So from the SDK's point of view the two sides
  agree on the MLS epoch, but *specific ciphertexts* fail decryption.
- Round 1's gradient_chunks landed cleanly (2 of them). Round 2+ started
  failing. iPhone → Seeker direction continues to work throughout.
- Volumes: 45,933 `message_retrying` and 8,673 `message_failed` with
  "Max retries exceeded" over the run. Only 45 `message_delivered` events:
  almost nothing is being ACKed. Retry count on `message_failed` is 5
  (the IM app's `max_retries`).

## Where the `INVALID_CIPHERTEXT` reason originates

### Receive path, phone-side

`crates/offline-protocol/src/protocol/message_dispatch.rs` 1130-1569,
`handle_encrypted_message`:

1. Parse envelope (`parse_encrypted_payload`). Both JSON and compact
   base64 forms accepted. ← Envelope was decodable (no `InvalidPayload`).
2. Call `mls.decrypt(&encrypted, sender)` →
   `MlsManager::decrypt_from_user` →
   `SessionManager::decrypt_message`
   (`crates/offline-protocol-mls/src/session.rs:228-259`).
3. That calls `GroupManager::decrypt_message`
   (`crates/offline-protocol-mls/src/group.rs:367-479`) which runs
   `group.process_message(provider, protocol_message)`.
4. The error match at `group.rs:399-405` classifies the OpenMLS error:

   ```rust
   match &e {
       ProcessMessageError::ValidationError(ValidationError::WrongEpoch)
       | ProcessMessageError::ValidationError(ValidationError::NoPastEpochData) => {
           MlsError::SessionDesync(e.to_string())
       }
       _ => MlsError::Decryption(e.to_string()),
   }
   ```

5. `MlsError::Decryption | InvalidMessage | Deserialization`
   → `DecryptionFailureKind::InvalidCiphertext` via
   `crates/offline-protocol/src/mls_observability.rs:65-67`.
6. Which drives `DecryptResult::Failed { kind: InvalidCiphertext, retriable: true }`
   in `message_dispatch.rs:1289-1324` (because
   `crypto_recovery_enabled` is on).
7. Event is built with the final reason text seen in the log at
   `message_dispatch.rs:1544-1559`.

So on the wire, the receiver *didn't* see a `WrongEpoch` or `NoPastEpochData`.
The two sides agree on `(group_id, epoch)`. The ciphertext itself failed to
decrypt.

### What OpenMLS errors land in that `_ =>` arm (the actual root cause set)

In OpenMLS 0.7.4 (`openmls-0.7.4/src/group/mls_group/errors.rs` and
`framing/errors.rs`), `ProcessMessageError` for an application message that
is NOT epoch-mismatched can produce:

| `ProcessMessageError` → `ValidationError::UnableToDecrypt` → `MessageDecryptionError` | What it means |
|---|---|
| `AeadError`                           | AEAD authentication failed. Keys don't match. |
| `GenerationOutOfBound`                | No ratcheting secret for the given sender and generation. The generation was already consumed, or has not yet been derived and is beyond the forward window. |
| `SecretTreeError(TooDistantInThePast)` | Received a generation whose key was already pruned by the receiver's `out_of_order_tolerance` window. |
| `SecretTreeError(TooDistantInTheFuture)` | Received a generation that would require skipping `maximum_forward_distance` generations. |
| `SecretTreeError(SecretReuseError)`   | The exact same generation was already consumed (duplicate at the ratchet level). |
| `MalformedContent`                    | Ciphertext bytes malformed. |

All five of these would render as `MlsError::Decryption("Decryption failed: <openmls text>")`
on the wire, and arrive at the IM app with the opaque `code: INVALID_CIPHERTEXT`.
The IM app has no way, today, to tell them apart.

The current diagnostic log is thrown away before it reaches the app:

- `warn!(sender = %sender, error = %e, error_code = session_state_error.code(), "Failed to decrypt message")`
  at `message_dispatch.rs:1312-1316` writes the full `MlsError` to tracing.
- The uniffi binding has an `android_logger` + `oslog` bridge
  (`crates/offline-protocol-uniffi/src/host_log.rs`) so these DO reach
  logcat/oslog, but the IM app's Metro bundler captures only `console.log`
  from JS. The `warn!` text is not in `bz9dqjrbz.output`.

**The single most valuable short-term improvement is to carry the OpenMLS
error chain's text, which is privacy-safe (library-constant strings; see
below), through to the event's `reason` field.** This would turn the next
2-phone run's log into actual evidence for which of the five variants above
is firing.

## Why the OpenMLS error text is safe to forward

Walking `ProcessMessageError`'s display chain:

- `ValidationError::UnableToDecrypt` is `#[error(transparent)]`
  → delegates to `MessageDecryptionError`.
- `MessageDecryptionError::AeadError` renders `"An error occurred during AEAD decryption."`
- `MessageDecryptionError::GenerationOutOfBound` renders `"Couldn't find a ratcheting secret for the given sender and generation."`
- `MessageDecryptionError::SecretTreeError` delegates to `SecretTreeError`
  which renders static strings only (`"Generation is too old to be processed."`,
  `"Generation is too far in the future to be processed."`,
  `"The requested secret was deleted to preserve forward secrecy."`, …).
- `MessageDecryptionError::MalformedContent` renders `"The content is malformed."`
- `MessageDecryptionError::WrongWireFormat` renders `"The WireFormat was not PrivateMessage."`

Every leaf is a static library string. Not a user id, group id, session id,
peer id, hashed address, or any runtime data. `MlsError::Decryption(e.to_string())`
in `group.rs:404` wraps that chain into `"Decryption failed: <leaf text>"`,
and that's what would reach the `reason` field under the proposed
diagnostic enrichment. (It is NOT safe for every `MlsError` variant, see
"what to avoid" below.)

## Hypothesis A (primary): Tier 2 reseal + lossy BLE + many in-flight messages blows past the sender-ratchet out-of-order window

### The mechanism

Tier 2 reseal is a correctness feature that the SDK applies to **every**
retry of an encrypted DM, not just retries after a session rekey:

- `OfflineProtocol::try_flush_send_via` (`protocol/mod.rs:2843`) calls
  `reseal_for_resend_in_place(&mut message)` before every outbound send
  attempt.
- `OfflineProtocol::process_retry_queue` (`protocol/mod.rs:3906`) does the
  same before every retry transmit.
- Inside, `reseal_resend_content` (`protocol/send.rs:3319`) calls
  `prepare_outbound_content` → `encrypt_bytes_for_recipient` →
  `MlsManager::encrypt_for_user` → `group.create_message()`
  (`openmls-0.7.4/src/group/mls_group/application.rs:16`), which calls
  `self.encrypt(...)`, **and advances the sender's encryption ratchet
  one generation**.

So every retry of an un-ACKed message bumps the sender's encryption
generation by **one**. The id is preserved; the on-wire bytes are different
and consume a fresh generation on the sender side.

### Why that interacts badly with federated training

1. The sender has many messages in flight at once (gradient_chunk fan-out
   per round, plus control frames, plus any CRDT-shadow writes). Each
   fresh send is a distinct generation; let the latest be `N`.
2. BLE drops some ACKs. The retry queue picks up the un-ACKed ones. Each
   retry re-seals ⇒ bumps the sender ratchet to `N+1`, `N+2`, …
3. Multiple messages retry concurrently in the same drain pass. The
   sender's `N` jumps forward by `(count of retried messages)` on each
   drain, not by 1.
4. BLE delivers frames out of order. The receiver's decryption ratchet
   advances to the highest received generation.
5. When an older frame (same ratchet, lower gen) arrives after a newer
   frame has advanced the receiver's head, it is tested against the
   `out_of_order_tolerance` window.

The SDK pins the window at

```
pub const SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE: u32 = 32;
```

(`crates/offline-protocol-sealed/src/constants.rs:43`), raised from OpenMLS's
default of 5 because of "windowed media transfer's in-flight budget (up to
8 chunks on internet transports, interleaved with text on the same 1:1
session ratchet)".

In this federated-training session, generation spread across in-flight
messages trivially exceeds 32 because:

- A single training round can emit 10+ MLS-wrapped frames.
- Each un-ACKed message in the retry queue burns one generation per
  retry (and the IM app's `max_retries = 5` means up to 5 bumps per
  message).
- Round 2+ sees sustained traffic where previous rounds' residuals
  (plus new frames) all co-exist in the outbox.

When this cap is exceeded, a frame arriving late at the receiver fails
decryption with `SecretTreeError::TooDistantInThePast`, which the SDK
catches as `_ => MlsError::Decryption(e.to_string())` in `group.rs:404`
and surfaces to the app as `code: INVALID_CIPHERTEXT`.

### Fits the symptoms

- **Round 1 works, round 2+ fails**: Round 1's traffic is initial and
  mostly delivered inside the 32-gen window. By round 2 the backlog +
  reseal retries have pushed the sender ratchet far past what the
  receiver can tolerate for any late arrival.
- **One direction works, the other doesn't**: whichever direction has
  the heavier traffic bump (gradient_chunks go worker → coord =
  Seeker → iPhone) will blow the window first. iPhone → Seeker is
  thinner: `round_start`, `round_complete`, `outer_lora_ready`,
  `aggregated_ready`, far fewer frames per round.
- **No `group_epoch_fork_detected`**: the epoch IS consistent. Only the
  per-generation key is unavailable, not the epoch.
- **Unique message_ids on each failure**: these are not retransmissions
  of the same id (receiver's dedup would short-circuit those at
  `receive.rs:166` before `handle_encrypted_message` runs). They are
  distinct new frames whose generations have drifted past the window.
- **The sender's retry loop re-seals and fails forever**: each retry
  advances the sender ratchet further, which if the receiver already
  advanced further still just lands the next retry inside the window:
  or if the receiver is advancing in lockstep, the retry succeeds. The
  symptom shows that in practice retries do not catch up in this
  workload (hence 128 terminal `message_failed` events and only 45
  `message_delivered`).

### What's NOT required to fit this hypothesis

- A session rekey. No `SessionDesync` event fired.
- A session recreate. No `secure_session_failed` or Welcome processing
  logged.
- An attacker. The failure is a self-inflicted ratchet-spread problem.

## Hypothesis B (secondary): Welcome-wins race leaves one side's early
  sealed bytes undecryptable by the other

This DOES produce `INVALID_CIPHERTEXT` without `group_epoch_fork_detected`
when the race is resolved cleanly, because:

- Both A and B create their own sessions `S_A`, `S_B` concurrently and
  send Welcomes.
- Both sessions are at epoch 0 with different leaf keys.
- Deterministic tiebreak (`message_dispatch.rs:812-818`): the device with
  the greater bech32 address adopts the remote Welcome; the other keeps
  its own.
- If A adopts `W_B`, A's prior `confirmed_sessions`-cached session
  switches to `S_B`. Any ciphertext A already sealed with `S_A` (if any)
  arrives at B. B's session is `S_B`. OpenMLS matches `(group_id, epoch)`
  (same shape, both at 0) but the AEAD key is wrong → `AeadError` →
  `MlsError::Decryption` → `INVALID_CIPHERTEXT`. No `WrongEpoch`.

Why it is secondary here: this is a **one-shot window** around session
establishment. It cannot explain round 2+ failures continuing to appear
after round 1 completed cleanly. Still worth watching for if the primary
hypothesis's fix doesn't fully resolve the symptom, it would show up as a
single burst of failures right at the start of a session with the
sealed-under-wrong-key error signature.

## Reasons already-known-good filters don't catch this

- `SessionDesync` detection catches `WrongEpoch` / `NoPastEpochData`, triggers
  `schedule_session_rekey`. Our failures are NOT in that class.
- `SecurityRejected` catches `SenderIdentityMismatch`,
  `SessionIdentityMismatch`, `LeafAddressMismatch`, `UnsupportedSender`.
  Our failures are not identity mismatches.
- `MlsNotInitialized` catches a cold-boot missing MLS manager, not applicable.
- The retriable path emits per-attempt
  `message_decryption_failed_for_frame` with `sealed_at_ms` /
  `session_established_at_ms` populated. That's what we're seeing. Working
  as designed; the problem is upstream in the error class the receiver
  can't decrypt.

## Proposed fix shape

**None landed in this PR.** This is a diagnosis-only hand-off. The fix
shapes below are listed in order of risk from least to most intrusive,
and each needs validation against a 2-phone federated-training session
after landing.

### F1. Diagnostic enrichment (zero behavior change)

Change `message_dispatch.rs::handle_encrypted_message` so the `reason`
field on `message_decryption_failed_for_frame` includes the OpenMLS
error chain text, not just the kind. The minimal shape:

- Add a `detail: String` field (captured as `e.to_string()`) to
  `DecryptResult::Failed`.
- Interpolate it into the retriable and non-retriable reason strings.

**Safety:** The strings that would reach the reason are static library
constants (see "Why the OpenMLS error text is safe to forward" above)
for every `MlsError` that classifies as `DecryptionFailureKind::InvalidCiphertext`
(`InvalidMessage`, `Deserialization`, `Decryption`). What to AVOID
widening: `CommitNotAuthorized { committer, added, removed }`,
`SessionIdentityMismatch { expected, found }`,
`LeafAddressMismatch { claimed, derived, ... }`, these carry
identifiers. They are already intercepted BEFORE reaching
`DecryptResult::Failed` (via `SecurityRejected` / `SessionSlotMismatch`
arms at `message_dispatch.rs:1196-1247`), so the gated branch is clean,
but the next person editing it needs to know.

**Why do this first:** gives us the exact OpenMLS variant on the next
run. If it's `TooDistantInThePast`, F2 is confirmed as the fix. If it's
`AeadError`, Hypothesis B is in play and F3 is the fix. If it's
`SecretReuseError` or `GenerationOutOfBound`, F2's variant is wrong
and the fix shape needs rethinking.

### F2. Raise `SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE`

In `crates/offline-protocol-sealed/src/constants.rs`:

```rust
pub const SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE: u32 = 32;
```

Raise to `128` or `256`. Memory cost: up to that many unused message
keys per sender ratchet (one `RatchetKeyMaterial` = `AeadKey` +
`AeadNonce`, so a few hundred bytes). That is a few tens of KB in the
pathological case per peer, bounded.

**Both sides must agree**: this is embedded in the sealed-layer constants
precisely because a mismatch produces a session that stops decrypting
under load, on a device, later (`constants.rs:1-7` is the warning). Both
phones ship the same SDK pin, so a bump is coordinated by a single SDK
version bump. Interop with mesh-node / Pi leaf nodes needs the same
version pin.

**Not sufficient on its own**: raising the window gives headroom but
doesn't fix the self-reinforcing retry/reseal loop. In a long-enough
session, 256 is reachable too.

### F3. Only re-seal on retry when the session epoch has advanced

The design purpose of Tier 2 reseal is to recover a message that was
sealed under an old epoch after a desync + rekey, see the docstring on
`reseal_resend_content` (`send.rs:3305-3318`). Running it on **every**
retry, even when the session's epoch has not changed, is overkill:

- When the session is still at the same epoch, replaying the verbatim
  ciphertext is correct from the key-schedule side, the receiver can
  still find the ratchet secret for that generation (as long as it's
  within `out_of_order_tolerance`).
- Replay DOES fail with `SecretReuseError` if the receiver already
  consumed that generation (i.e. the original was received but its ACK
  was lost). In that case the receiver's deduplicator catches the
  retransmit at `receive.rs:166` BEFORE calling `handle_encrypted_message`,
  so OpenMLS never sees the duplicate.
- The receiver's dedup is keyed on `message.id`, so the retransmit of
  the same id (whether verbatim or re-sealed) hits dedup first.

Fix shape:

1. Add `seal_epoch: u64` to `OutboxReseal`.
2. Set it when the entry is created (in `prepare_outbound_content` or at
   the stage point in `stage_outbox_reseal`).
3. In `reseal_resend_content`, read the current session epoch via
   `MlsManager::session_epoch(recipient)` (a method that does not yet
   exist, would need adding; both `encrypt_for_group` and the group
   path have `group.epoch().as_u64()`, so a sibling for 1:1 is a thin
   wrapper on `session_manager.get_session_id` + `load_group`).
4. If `current_epoch == seal.seal_epoch`, return `None` (replay
   verbatim); else re-seal.

**Risk:** verbatim replay is a correctness regression if the receiver
dedup is EVER missed. Audit path: `deduplicator.is_duplicate_mut` is
called before `process_internal_message_via`; the only way to bypass is
a receiver whose dedup state was wiped (restart or TTL eviction). TTL
is 7 days (`DEFAULT_OUTBOX_LIFETIME_MS`), and a restart followed by a
replay would hit `SecretReuseError` on the generation, which lands in
`InvalidCiphertext` and surfaces via the same `message_decryption_failed`
path, recoverable by the next epoch rekey. Net: no new silent-drop
mode; the failure mode after a restart is the same one we have today
WITHOUT a restart.

### F4. Dedup `message_decryption_failed` at the IM-app side

Phone-side, not SDK-side. Not a fix for the root cause but relevant to
the task description's mention of ~50 events. If a message is being
retried 5x and each attempt produces a `message_decryption_failed`,
the app's TrainingModule listener is seeing 5× the real failure count.
A dedup on `message_id` at the listener would surface the honest number.
Flagged here for the IM-side implementer to track in a separate ticket.

## What to collect on the next 2-phone run

If F1 lands first (and merges on a separate PR), the next run will show
the real OpenMLS variant inside the reason. Specifically grep for one of:

- `"Generation is too old to be processed"` → `TooDistantInThePast` → F2 wins.
- `"The requested secret was deleted to preserve forward secrecy"` →
  `SecretReuseError` → the receiver's dedup is being bypassed somehow;
  investigate dedup state separately.
- `"An error occurred during AEAD decryption"` → `AeadError` → session
  mismatch, hypothesis B in play. Check for a both-create race at
  establishment time.
- `"Couldn't find a ratcheting secret for the given sender and generation"`
  → `GenerationOutOfBound` → F2 (narrower variant) or an inconsistent
  secret tree.
- `"The content is malformed"` → `MalformedContent` → wire-level
  corruption; look at the transport layer, not MLS.

## Ticket pointers

- Linear: https://linear.app/offlineprotocol/issue/OFF-3247
- IM app CLAUDE.md: see "**Auto key exchange is ON** … `storePending` and
  `requireEncryption` stay `false`", the config this hits with.
- SDK CLAUDE.md: see "A UDL change also regenerates the C dispatcher"
  (changing the event type would need UDL regen; the reason-string
  enrichment proposed in F1 does not touch UDL).
