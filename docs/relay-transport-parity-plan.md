# Relay-Transport Parity — Implementation Plan

Companion to `docs/relay-transport-parity-spec.md`. Spec validated against three repos on 2026-07-10:
`offline-protocol-sdk` (this repo), `../fernweh_v2` (JS relay client), `../relay-server` (relay source — full ground truth).

> **Implementation status (2026-07-10, branch `feat/relay-parity-phase-a`):** Phase A (A0–A8) and
> Phase B (B1–B4, B6, B7) are implemented and verified (802 Rust tests, clippy `-D warnings`, fmt,
> Android CI-harness suite, `tsc`). B5 (WI-7 `MessageSent` reconciliation) dropped by design. Still
> open: B8 integration verification against the production relay (needs devices/relay access), and
> `npm run build:uniffi:all` to rebuild native libs before device testing (bindings are regenerated,
> `.so`/`.a` are not). B3 step-0 checks resolved from relay source: `AddGroupMember` is admin-gated
> ("Only admins can add members" — translator stops deltas per group on that error), and idempotent
> `CreateGroup` re-sync does NOT update a stored group name (renames don't propagate to the relay
> registry — known v1 limitation).

## Ground-truth corrections to the spec (verified against relay-server source)

These change the design, not just details:

1. **The relay does NOT intercept `__GRP_RELAY_REG__` / `__GRP_RELAY_BCAST__`.** `handle_send_message`
   (`relay-server/src/websocket.rs:273-376`) never inspects `content` and doesn't special-case
   `recipient == sender` — a self-addressed frame is echoed back to the sender. Core's relay-optimized
   group path is a silent no-op against production, and worse:
2. **Live group data-loss bug (not in the spec).** `try_relay_register_group` (`group_mesh.rs:2925-2962`)
   inserts into `relay_synced` when `send_internal_message` returns `Ok` — but `send_internal_message`
   returns `Ok(message_id)` unconditionally (`send.rs:284-328`; internet `send()` only enqueues). With
   `config.group.relay_enabled` **defaulting to `true`** (`config.rs:234`), any group registry mutation
   while internet is up marks the group relay-synced, and every subsequent `send_group_message` takes the
   broadcast path (`group_mesh.rs:2303`) — whose frame the relay echoes to self. **Group messages over
   internet are silently lost** (sender still gets `Event::group_message_sent`). Nothing ever unsets
   `relay_synced` except internet-loss, leave, or removal.
3. **No store-and-forward anywhere.** The relay never persists message content. Offline recipient →
   FCM push attempt; if the push *succeeds* the sender gets `MessageSent` (content is never delivered by
   the relay — a false wire-confirm); only if the push *fails* does `DeliveryError {recipient, reason}`
   fire (`websocket.rs:328-334, 369-372`). So in production-with-FCM, **`DeliveryError` is not the primary
   offline signal — `CheckPresence` is**, and WI-1+WI-2 alone don't stop welcome-budget burn.
4. **Connection requests are NOT stored for offline recipients** (spec §2 item 4 assumed they were).
   Sender gets `ConnectionRequestError {recipient, reason:"User is offline"}` (`websocket.rs:1268-1274`);
   accept/reject/cancel to offline targets are silently dropped. Translation (WI-5) is still right — for
   legacy-JS-client interop and the error feedback — but not for offline delivery.
5. **`CheckPresence`** needs no contact relationship, always answers `PresenceStatusWithLastSeen
   {user_id, online, last_seen?: String}` (plain `PresenceStatus` is never emitted). Server rate limit is a
   global per-connection token bucket (burst 30, 10/s) — the 20s/10-peer watch cadence is safe.
6. **Invite-link reality for the WI-6 port:** `GetGroupInviteLinkPreview` has no server handler (client
   8s-timeout → null); `AckGroupInviteJoin` carries no `request_id`; joins need an online admin ack within
   85s (re-fanout every 25s); `GroupError` carries the `request_id` for rejections.
7. Spec detail fixes: `__CONN_REQ__` payload is `{sender_name, timestamp_ms, key_package?: Vec<u8>}`
   (no `initial_message`); `__CONN_ACC__` field is `accepted_by_name`; `__CONN_REJ__`/`__CONN_CAN__`
   builders exist with empty payloads (`send.rs:2134, 2158`); core emits **no** outbound
   `__GROUP_CREATED__`/`__GROUP_MEMBER_ADDED__` — group registry intent goes out solely as self-addressed
   `__GRP_RELAY_REG__ {group_id, group_name?, members[]}` (on create/invite/remove/rename and on internet
   0→1 via `sync_groups_to_relay`); welcome max attempts is config-driven
   (`reliability.retry.max_retries`, default **10**, not 6); `TypingUpdate` is already handled
   (Kotlin :650, Swift :545); Kotlin sets OkHttp `pingInterval` (`InternetManager.kt:205`), so the iOS/
   Android ping asymmetry is mostly benign.

## Root cause / core insight

The SDK was built assuming a relay that *participates in its protocol* (prefix interception,
store-and-forward, request replay). The production relay is a dumb online-only forwarder with a separate
server-plane API. Everything in this plan is one move: **make the platform bridge the relay adapter the
core always assumed existed** — translate core's server-plane intents (group registration, broadcast,
connection ops) into the relay's native API, and feed the relay's server-plane signals (DeliveryError,
presence) back into core's existing parking/re-arm machinery. Plus fix the false `relay_synced` gate that
this assumption created.

## Design decisions

### D1 — Group plane (replaces spec WI-5's group rows)

**Alternatives considered:**

- **(a) Full bridge translation including broadcast.** Tag `__GRP_RELAY_REG__`/`__GRP_RELAY_BCAST__` via
  `control_op`; bridge sends `CreateGroup`+member deltas / `SendGroupMessage`; gate `relay_synced` on the
  relay's `GroupCreated` ack. Pros: realizes the O(1) fan-out core was designed for; registry populated
  automatically (invite links work with zero app code); fixes data loss at the root. Cons: three coupled
  pieces (tagging, ack-gating, translation) must ship atomically — once ack-gating can set `relay_synced`,
  the broadcast path *will* be taken, so BCAST translation must exist. Effort: ~medium (core ~60 lines,
  bridge ~150/platform). Reversible: yes (config `relay_enabled=false` restores per-member).
- **(b) Kill the relay optimization; registry via raw channel.** Never set `relay_synced`; fernweh ports
  its mesh-first `CreateGroup` sync onto the WI-6 raw channel. Pros: smallest SDK change. Cons: keeps
  ~100 lines of group-sync logic in every app (defeats the parity goal); N× wire fan-out forever; the dead
  broadcast code rots.
- **(c) Chosen — two-stage:** **Phase A** ships the one-line root fix (stop inserting `relay_synced` on
  enqueue) so group sends are per-member-and-correct immediately; **Phase B** ships (a) as one coherent
  unit. Decisive factor: Phase A must be independently shippable and the data-loss fix cannot wait for the
  translation work; Phase B then restores the O(1) design intent safely because ack-gating + translation
  land together.

During the interim (Phase A shipped, B not), `relay_synced` is never true → always per-member fan-out
(the spec's own "keep per-member for v1"). REG frames still self-echo through the relay until B translates
them; fernweh's `sdkControlPrefixes` drop-list already defends, and core drops unknown inbound prefixes.

### D2 — WI-1 classification

Reason-prefix string, as the spec proposed. `pub(crate) const SEND_FAIL_REASON_RECIPIENT_UNREACHABLE:
&str = "recipient_unreachable"` in `protocol/types.rs` (every const there is `pub(crate)`; the bridge side
hardcodes the literal — document the cross-layer contract at both sites). Add
`WelcomeReasonCode::PeerUnreachable` (`events.rs:29-53`, stable `as_str` = `"peer_unreachable"`) so
`welcome_send_failed` events are truthful; events cross FFI as JSON, so this is additive with no regen.

### D3 — WI-3 presence ingestion closes the FCM hole

The spec's `internet_peer_presence(peer_id, online, last_seen_ms)` UDL method, with one addition grounded
in finding #3: **`online=false` parks pending welcomes for that peer** (`park_welcome_no_carrier` — rolls
back the attempt, pushes TTL, schedules the 15s slow retry). Combined with the watch loop, this is what
actually stops budget burn when FCM push "succeeds" against an offline peer: welcome confirm-timeout burns
1–2 attempts (of default 10), the watch tick's `CheckPresence` says offline → parked; presence online →
`on_neighbor_discovered` → immediate retry. Implement as one core entry point
`OfflineProtocol::on_peer_presence(peer_id, online, last_seen_ms)` that also emits `presence_updated`
(single emission point, consistent with the inbound `__PRESENCE__` path). Skip the spec's bridge-only
`__PRESENCE__`-injection fallback — it can't express offline-parking, and we're regenerating bindings in
Phase A anyway.

### D4 — Watch-set sourcing

Bridge-observed signals (`DeliveryError`, `ConnectionRequestError` recipients) alone miss the FCM case, so
add a core-owned source: UDL `sequence<string> internet_presence_watchlist()` returning peers with a live,
un-Sent welcome lifecycle. The bridge unions it into the watch set each 20s tick (poll-as-reconciliation —
no missed-event risk, trivially testable). Removal: peer went online, inbound traffic from peer, or ~10 min
idle.

### D5 — WI-7 (`MessageSent`)

Recommend **dropping** from scope. Ground truth makes it actively misleading: `MessageSent` fires even
when the recipient is offline and only an FCM poke went out. If kept, telemetry-only; never treat as
delivery, never clear WI-2 in-flight entries with it.

## Blast radius

**Rust core** (`crates/offline-protocol/src/`): `protocol/types.rs` (+1 const), `protocol/send.rs`
(classification in `on_transport_send_failed:1742`), `protocol/session.rs` (call `park_welcome_no_carrier`
from the presence path), `protocol/mod.rs` (`on_peer_presence`, `welcome_pending_peers` accessor,
control-op detection helper), `events.rs` (`PresenceUpdated.last_seen_ms`, `PeerUnreachable` variant),
`group_mesh.rs` (drop enqueue-time `relay_synced` insert), `message_dispatch.rs` (`GroupCreated` ack-gate),
`protocol/tests/mod.rs`.
**UniFFI**: `offline_protocol.udl` (2 methods Phase A; 2 `InternetMessage` fields Phase B), `lib.rs`.
Regen must be `npm run build:uniffi:all` — `generate:bindings` alone leaves stale `.so`/`.a` (ABI
mismatch).
**Bridges**: `InternetManager.kt` / `InternetManager.swift` (major), `OfflineProtocolModule.kt` /
`.swift` / **`.m`** (new methods need `RCT_EXTERN_METHOD` entries), new pure policy classes + tests.
**TS**: `src/types.ts`, `src/index.ts`; `npm run build` to refresh `lib/` before fernweh consumes via
`file:`.
**Contracts**: UDL additions are append-only (ProtocolError enum untouched); event JSON changes are
additive (`last_seen_ms?`, new `internet_server_message` type); new cross-layer string contract
`"recipient_unreachable"` bridge→core.
**Migrations**: none. `WelcomeLifecycleRecord` persistence unchanged.
**Rollback**: every step is revert-safe except A0 — reverting A0 restores the group data-loss bug, so
never revert it without also setting `group.relay_enabled=false`.

**Risks**: (1) B2/B3 coupling — mitigated by shipping as one commit-set and keeping per-member fallback
when `relay_synced` is false; (2) stale relay presence parking a welcome for a peer that's actually online
— bounded at 15s (`WELCOME_NO_CARRIER_RETRY_SECS`) and inbound traffic re-arms instantly; (3) relay
`AddGroupMember` admin-gating semantics unverified → possible `GroupError` noise from non-admin members'
re-registrations — resolved by the B3 step-0 check against local relay source; (4) release vehicle: Phase A
must ship in an npm release that includes the issue-#136 fix (on `main` post-v0.11.0 — verify by tag
ancestry, not changelog).

---

## Implementation steps

### Phase A — reliability release (independently shippable)

**A0 — stop false `relay_synced` (group data-loss fix).**
`group_mesh.rs:2953`: remove the enqueue-time `relay_synced.insert`. Broadcast gate at `:2303` then never
passes → per-member `__GRP_MLS_MSG__` fan-out always (verified-working data plane). Keep
`try_relay_broadcast` and the REG call sites (Phase B needs them; `sync_groups_to_relay` re-registering on
every internet 0→1 is the desired Phase-B behavior). Tests: group send over internet after registration
goes per-member; REG frames still emitted on transition. *Commit: `fix(protocol): never mark groups
relay-synced on enqueue`.*

**A1 — WI-1 reason-based no-carrier.**
`protocol/types.rs`: add the `pub(crate)` const. `send.rs:1742-1763`: `peer_unreachable =
transport_error.as_deref().map_or(false, |r| r.starts_with(SEND_FAIL_REASON_RECIPIENT_UNREACHABLE))`;
`no_carrier = peer_unreachable || transports-empty`; reason code `PeerUnreachable` when classified (new
`events.rs` variant). Non-welcome ids: `on_transport_send_failed` no-ops (verified) — no double-retry;
note in a comment. Tests (in `protocol/tests/mod.rs`, mirroring
`test_welcome_transport_callbacks_out_of_order_converge_to_sent` at :4673): failure with
`"recipient_unreachable: …"` while internet is up → attempt unchanged, `expires_at` pushed, state parked
not expired, event reason `peer_unreachable`. *Commit: `fix(protocol): classify recipient-unreachable send
failures as per-peer no-carrier`.*

**A2 — WI-3 core presence.**
`events.rs`: `PresenceUpdated` += `last_seen_ms: Option<i64>` (`skip_serializing_if`), builder updated
(internal call sites: `message_dispatch.rs:810` passes `None`, tests). `protocol/mod.rs`:
`pub fn on_peer_presence(peer_id, online, last_seen_ms)` — online → `on_neighbor_discovered` (flushes
outbox, re-arms welcome, key exchange; benign for unknown peers — all side effects self-guarding);
offline → `park_welcome_no_carrier(peer_id)`; always emit `presence_updated`. Add
`pub fn welcome_pending_peers() -> Vec<String>` (lifecycle records with state ∉ {Sent, Expired}). Tests:
offline parks (attempt rollback + TTL push + 15s retry), online re-arms an expired welcome (mirror
:4611), event carries `last_seen_ms`. *Commit: `feat(protocol): ingest per-peer presence to park and
re-arm welcomes`.*

**A3 — UDL Phase A + regen.**
`offline_protocol.udl`: `void internet_peer_presence(string peer_id, boolean online, i64? last_seen_ms);`
and `sequence<string> internet_presence_watchlist();`. `uniffi/src/lib.rs`: thin delegations to A2 (empty
peer_id guard, mirroring `notify_neighbor_reachable:2355`). Run `npm run build:uniffi:all`. *Commit:
`feat(uniffi): expose internet_peer_presence and presence watchlist`.*

**A4 — WI-2 bridge DeliveryError fail-fast (Kotlin + Swift together).**
`sendMessage()` records `recipient → [(messageId, sentAtMs)]` (TTL 60s, cap 32/peer, pruned on poll tick).
`DeliveryError` handler (kt:630, swift equivalent): for each live in-flight id →
`internetSendFailedWithReason(id, "recipient_unreachable: " + reason)`; add recipient to watch set; call
`internetPeerPresence(recipient, false, null)`. Give `ConnectionRequestError {recipient}` (kt:773,
swift:673) the same treatment. Extract the in-flight map as a pure class (e.g.
`RecipientInFlightTracker`) with JVM-harness + XCTest tests.

**A5 — WI-3 bridge presence handler (both platforms).**
`PresenceStatusWithLastSeen` (keep `PresenceStatus` defensively): parse `user_id`, `online`, `last_seen`
(ISO-8601 via `parseTimestampToMs`; fix its fallback to return null rather than now-on-parse-failure) →
`internetPeerPresence(...)`; online → drop peer from watch set.

**A6 — WI-4 watch loop + RN query (both platforms).**
Watch set = A4/A5 signals ∪ `internetPresenceWatchlist()` polled each tick; removal per D4. Every 20s
while connected+authenticated: `{"type":"CheckPresence","username":…}` for ≤10 peers (round-robin).
Extract `PresenceWatchPolicy` as a pure tested class. RN method `checkInternetPresence(userId)` (Kotlin
`@ReactMethod` + Swift `@objc` + `.m` `RCT_EXTERN_METHOD`) → one CheckPresence; result arrives as the
`presence_updated` event. *Commits: A4; A5+A6 — `feat(bindings): relay DeliveryError fail-fast, presence
ingestion, and presence watch loop`.*

**A7 — TS Phase A.** `types.ts`: `PresenceUpdatedEvent` += `last_seen_ms?: number`. `index.ts`:
`checkInternetPresence`. `npm run build`. *Commit: `feat(bindings): TS surface for presence`.*

**A8 — Phase A verification.**
`cargo clippy --workspace -- -D warnings`, `cargo test --workspace --lib`, `cargo fmt --all`; Android
harness (`android-ci-harness`, `:offlineprotocol:testDebugUnitTest`). Spec acceptance: welcome to an
offline peer over a connected relay parks with `attempt` unchanged; peer online (or CheckPresence
round-trip) → delivered with no app involvement. Then release (include #136 fix; verify by tag ancestry).

### Phase B — parity + surfaces

**B1 — core control-op tagging + UDL.**
Core helper `pub(crate) fn internet_control_op(recipient_is_self: bool, content: &str) ->
Option<(&'static str, String)>`: `__CONN_REQ__`→`conn_req`, `__CONN_ACC__`→`conn_acc`,
`__CONN_REJ__`→`conn_rej`, `__CONN_CAN__`→`conn_can` (any recipient);
`__GRP_RELAY_REG__`→`group_relay_register`, `__GRP_RELAY_BCAST__`→`group_relay_broadcast` (self-addressed
only). Unit-test in core. UDL `InternetMessage` += `string? control_op; string? control_payload;`;
`internet_get_next_message` (lib.rs:3100, incl. the fallback-queue path) populates them from the
already-deserialized `message.content`. Regen `build:uniffi:all`. Poll loops keep working untouched until
B3 reads the new fields.

**B2 — ack-gated `relay_synced` (ships with B3, same PR).**
`message_dispatch.rs:874` `GroupCreated` handler: if `group_mesh.members` contains the group →
`relay_synced.insert(group_id)`. A group-sync `GroupError` for the group → ensure removed. Tests: REG
enqueue alone doesn't sync; inbound `GroupCreated` syncs → next `send_group_message` returns the single
broadcast id; `GroupError` keeps per-member.

**B3 — bridge outbound translation (Kotlin + Swift, same PR as B2).**
Step 0: read `relay-server` `handle_add_group_member` for admin gating and `CreateGroup`
existing-group-rename semantics; adjust below if needed. Poll loop: `controlOp != null` → translate
instead of `SendMessage`; wire-write success of the primary frame → `internetConfirmSent(messageId)`,
failure → `internetSendFailed`:
| control_op | relay frame(s) |
|---|---|
| `conn_req` | `SendConnectionRequest {recipient, sender_name, key_package?}` (`key_package` = JSON number array pass-through; no `initial_message` — core has none) |
| `conn_acc` | `AcceptConnectionRequest {requester_id: recipient, accepter_name: accepted_by_name, key_package?}` |
| `conn_rej` | `RejectConnectionRequest {requester_id: recipient}` |
| `conn_can` | `CancelConnectionRequest {recipient}` |
| `group_relay_register` | `CreateGroup {group_id, name: group_name ?? group_id}` + per-group membership diff (bridge-tracked set, reset on reconnect) → `AddGroupMember`/`RemoveGroupMember {group_id, username}` |
| `group_relay_broadcast` | `SendGroupMessage {group_id, content: ciphertext, reply_to_msg: reply_to}` (forward_info not representable — falls back to per-member for forwards is NOT automatic; accept the loss, note it) |
Everything else — `__MLS_*`, `__TYPING__`, `__READ_RECEIPT__`, `__PRESENCE__`, `__GRP_MLS_*`,
`__GROUP_MEMBER_REMOVED__`, `__GRP_RENAME__`, chat — continues verbatim as `SendMessage` (the inbound side
already maps relay events back to the same prefixes, so the round-trip stays lossless). Extract
`RelayControlOpTranslator` as a pure tested class per platform. *Commit-set: `feat(protocol+bindings):
relay-native translation for connection and group-registry ops` (B1+B2+B3 atomic).*

**B4 — WI-6 raw channel + server-message event (both platforms).**
`internetSendRawCommand(json): Promise<boolean>` — validate JSON, require connected+authenticated, send
verbatim. Inbound: explicit server-plane list (`GroupInviteLinkCreated`, `GroupInviteLinkRevoked`,
`GroupJoinedViaInvite`, `GroupInviteJoinPending`, `GroupRoleChanged`, `GroupDeleted`, `RateLimited`) plus
the unknown-type else branch (kt:893) → emit `{type:"internet_server_message", json}` on the
`OfflineProtocol_Event` channel. `GroupError`: dual-emit (keep the `__GROUP_ERROR__` injection AND the raw
copy — fernweh's invite correlation needs `request_id`). Drop `TypingUpdate`/`MessageRead` from the spec's
list (`TypingUpdate` is handled; `MessageRead` is never emitted by the server). *Commit: `feat(bindings):
generic relay server-command channel and server-message event`.*

**B5 — WI-7:** dropped (see D5) unless telemetry demand appears.

**B6 — WI-8 iOS parity audit.**
A4–A6/B3/B4 are dual-platform by construction; finish with a handler-by-handler Kotlin↔Swift diff. Ping:
Kotlin already sets OkHttp `pingInterval` (kt:205) — verify OkHttp's missed-pong `onFailure` reaches
`handleConnectionClosed`; document rather than rewrite if so.

**B7 — WI-9 TS + rebuild.**
`types.ts`: `InternetServerMessageEvent` (+ union — `EventType` auto-extends); `index.ts`:
`sendRawServerCommand`. `npm run build` so compiled `lib/` matches `src/` before fernweh consumes via
`file:` (local `lib/` is gitignored and stale — check the built output, not the repo copy).

**B8 — integration verification (spec §2, updated by ground truth).**
Against the production relay: (1) two SDK-only clients — auth, MLS welcome via `SendMessage`, chat,
receipts, typing; (2) welcome budget: offline peer → parked, attempt stable; online → delivered; (3) group
registry: SDK `create_group` → relay `GroupCreated` → invite-link creation via raw channel succeeds
(replaces fernweh's mesh-first sync); (4) broadcast: relay-synced group message reaches a legacy JS-relay
member via server fan-out, and vice versa (`GroupMessageReceived → __GROUP_MSG__` inbound is already
implemented); (5) invite-link lifecycle over the raw channel incl. `AckGroupInviteJoin` admin approval;
(6) translated connection request visible to a JS-relay recipient. Items "does the relay queue offline
messages / store connection requests" are answered from source: **it doesn't** — drop them from the
checklist.

## Open questions (user input welcome, defaults chosen)

1. **B5/WI-7** — plan drops it. Object if you want `MessageSent` telemetry.
2. **`WelcomeReasonCode::PeerUnreachable`** — plan adds it (additive JSON). Say so if you'd rather reuse
   `TransportUnavailable`.
3. **Broadcast enablement** — plan ships B2+B3 active (ack-gating makes it safe; `group.relay_enabled`
   remains the kill switch). Alternative: leave broadcast permanently off and per-member forever.
4. **Watchlist scope** — welcome-pending peers only (chosen); extend to outbox-queued peers later if the
   parked-outbox case shows up.

## Summary

- **Problem:** SDK internet transport must fully replace fernweh's JS relay client against an unmodified relay.
- **Core insight:** the SDK assumed a protocol-aware relay; production is a dumb forwarder — make the bridge the adapter, and fix the false `relay_synced` gate that assumption created (live group data-loss bug, found during planning).
- **Approach:** Phase A = A0 data-loss fix + reason-classified parking + presence ingestion/park + watch loop (independently shippable). Phase B = control-op tagging + ack-gated relay broadcast + connection/group translation + raw server channel.
- **Blast radius:** ~8 core files, UDL + regen ×2, both bridge InternetManagers + modules, TS types; no migrations; additive contracts only.
- **Confidence:** High (Phase A), Medium-High (B3 — relay admin-gate semantics to confirm from local source at step 0).
- **Ready to implement:** yes.
