# Relay-Transport Parity Spec

**Goal:** an app configures only the SDK —

```ts
transports: { internet: { enabled: true, serverAddress: '<relay-ws-url>', authToken: '<jwt>' } }
```

— and gets full feature parity with fernweh's bespoke JS relay client (`src/hooks/useWebSocketRelay.ts`, ~2,300 lines), so that client can be deleted. **Constraint: the relay server is not modified.** The SDK adapts to the relay's existing JSON protocol.

---

## 0. Current state (verified 2026-07-09)

The platform bridge (`bindings/react-native/android/.../InternetManager.kt`, mirrored in `ios/InternetManager.swift`) already speaks the relay protocol:

| Already working | Where (Kotlin) |
|---|---|
| `Authenticate {token}` on connect; `Authenticated`/`AuthError` handling; token rotation via `setAuthToken()` re-auths live | `InternetManager.kt:323-341, 153-167` |
| Transport reported up **only after** `Authenticated` (`internetStatusChanged(true)`) | `InternetManager.kt:343-371` |
| Outbound: polls `internetGetNextMessage()` (100ms + post-auth flush), wraps frame as `SendMessage {recipient, content: <Message JSON>, reply_to_msg}` | `InternetManager.kt:924-1025` |
| Wire-write outcome → `internetConfirmSent` / `internetSendFailed`; ≥2 consecutive failures force disconnect (down-before-fail for DORS) | `InternetManager.kt:995-1024` |
| Inbound `MessageReceived`: content parsed as full Message JSON, injected via `internetMessageReceived(sender, bytes)` | `InternetManager.kt:536-628` |
| Inbound relay-native events translated to internal control messages: `ConnectionRequestReceived→__CONN_REQ__`, `ConnectionAccepted→__CONN_ACC__`, `ConnectionRejected→__CONN_REJ__`, `GroupCreated/__GROUP_CREATED__`, `GroupMessageReceived/__GROUP_MSG__`, `GroupMemberAdded/Removed`, `GroupInfo`, `UserGroups`, `GroupError` | `InternetManager.kt:650-871` |
| Reconnect with backoff (1s → 30s, ×2) | `InternetManager.kt:422-450` |

Core (Rust) already has:

- Welcome lifecycle with **no-carrier parking**: `park_welcome_no_carrier` (`session.rs:666`), attempt rollback + TTL push-forward on no-carrier failures (`session.rs:974-1003`), slow retry `WELCOME_NO_CARRIER_RETRY_SECS=15`.
- Inbound internet traffic re-arms reachability: `internet_message_received` → `notify_neighbor_reachable(sender, "Internet")` (`offline-protocol-uniffi/src/lib.rs:3000`) → `on_neighbor_discovered` → immediate welcome retry. ⚠️ This is the issue-#136 fix — it is on `main` but landed after the v0.11.0 npm release; the release fernweh consumes for this migration must include it.
- `internet_send_failed_with_reason(message_id, reason)` exposed in UDL (`offline_protocol.udl:807`).

fernweh already sets SDK `userId = storedUsername` (relay username) (`fernweh_v2/context/MeshCoreContext.tsx:217`), and the RN module wires `config.authToken → setAuthToken` (`OfflineProtocolModule.kt:1524-1526`).

### Mental model: two planes

- **Data plane (works today):** anything expressible as a peer→peer SDK `Message` rides `SendMessage`/`MessageReceived` with the serialized Message JSON as opaque `content`. This already carries `__MLS_WELCOME__`, `__MLS_ENC__`, `__TYPING__`, `__READ_RECEIPT__`, `__PRESENCE__` (push), connection-request control messages, and 1:1 chat. The relay forwards it verbatim.
- **Server plane (the gaps):** relay-native ops where the *server* is a participant — presence queries, delivery errors, the group registry, invite links. Inbound handling is mostly done; **outbound translation and the feedback loops are missing.**

---

## 1. Work items

### WI-1 (core) — Reason-based no-carrier classification

**Problem.** `on_transport_send_failed` (`crates/offline-protocol/src/protocol/send.rs:1742-1763`) computes
`no_carrier = self.transport_manager.get_available_transports().is_empty()`.
When the relay socket is up but the *recipient* is offline, `no_carrier=false`, so a `DeliveryError`-driven failure would burn a Welcome attempt (max 6) instead of parking. This is the "fails-while-up burns the budget" bug; over pure internet an offline peer expires the Welcome terminally in ~1–2 min.

**Change.** Define a well-known reason marker, e.g. in `types.rs`:

```rust
pub const SEND_FAIL_REASON_RECIPIENT_UNREACHABLE: &str = "recipient_unreachable";
```

In `on_transport_send_failed`, treat a reason starting with that marker as per-peer no-carrier:

```rust
let peer_unreachable = transport_error
    .as_deref()
    .map_or(false, |r| r.starts_with(SEND_FAIL_REASON_RECIPIENT_UNREACHABLE));
let no_carrier = peer_unreachable
    || self.transport_manager.get_available_transports().is_empty();
```

**Acceptance.** Welcome sent to an offline peer over a connected relay: `welcome_send_failed` fires with unchanged `attempt`, `expires_at` pushed forward, welcome stays parked; peer comes online → delivered.

Also audit the non-welcome path: `report_send_failure` on the internet transport after `confirm_sent` already removed the id from `pending_confirmation` may be a no-op for regular messages — acceptable (the e2e ACK timeout re-queues via the reliability layer), but verify no double-retry.

### WI-2 (bridge) — Map relay `DeliveryError` to fail-fast

**Problem.** `DeliveryError {recipient, reason}` is only logged (`InternetManager.kt:630-638`). It's the relay's authoritative "recipient offline" signal and arrives well before the 10s welcome confirm-timeout (which fails with `Timeout` = budget-burning).

**Change.** In `sendMessage()` record `messageId` into a bounded in-flight map `recipient → [(messageId, sentAtMs)]` (TTL ~60s, cap ~32/peer). On `DeliveryError{recipient, reason}`:

1. For each live in-flight id for that recipient: `protocol.internetSendFailedWithReason(id, "recipient_unreachable: " + reason)` (pairs with WI-1), then drop it from the map.
2. Add the recipient to the presence watch set (WI-4).
3. Feed presence ingestion (WI-3) with `online=false`.

`DeliveryError` carries no `message_id` — recipient-keyed correlation is the best available and is safe: everything in-flight to an offline peer failed.

### WI-3 (core + bridge) — Presence ingestion

**Problem.** `PresenceStatus` / `PresenceStatusWithLastSeen` are only logged (`InternetManager.kt:640-648`). Nothing re-arms parked traffic when a peer comes online, and the app has no presence/last-seen signal from the SDK.

**Change (core).** New UDL method:

```udl
void internet_peer_presence(string peer_id, boolean online, i64? last_seen_ms);
```

Implementation: if `online`, call `notify_neighbor_reachable(peer_id, "Internet", None)` (same path as `lib.rs:3000` — re-arms parked welcomes and flushes retry queues). Always emit `presence_updated {peer_id, status, timestamp, last_seen_ms?}` (add the optional `last_seen_ms` field to the event; TS type update in `types.ts`).

**Change (bridge).** The `PresenceStatus`/`PresenceStatusWithLastSeen` handler parses `user_id`, `online`, `last_seen` (ISO-8601 or epoch — reuse `parseTimestampToMs`) and calls `internet_peer_presence`.

*Zero-core-change fallback (if you want a bridge-only v0):* on `online=true` only, inject an internal `__PRESENCE__` message via `internetMessageReceived` — reachability comes free via `lib.rs:3000`. Don't inject for `offline` (it would falsely mark the peer reachable). The UDL method is the correct long-term shape.

### WI-4 (bridge) — Presence watch loop + query API

**Problem.** Nothing *asks* the relay about presence. fernweh's deferred-delivery machinery drives `CheckPresence` from JS; post-migration the SDK must self-drive it for parked traffic.

**Change.**

- Watch set: peers added on `DeliveryError` (WI-2) and (optionally) peers with parked welcomes; removed on `online` presence, on any inbound traffic from the peer, or after ~10 min idle.
- Every ~20s while connected+authenticated, send `{"type":"CheckPresence","username":"<peer>"}` for each watched peer (batch cap ~10/tick). Response flows through WI-3.
- App-facing query for UI (last-seen display): RN method `checkInternetPresence(userId: string): void` on the module → sends one `CheckPresence`; result arrives as the `presence_updated` event. (Fire-and-event, not request/response — matches relay semantics and fernweh's existing throttling model.)

### WI-5 (core + bridge) — Outbound control-op translation

**Problem.** Inbound relay-native → control-message translation exists; the reverse doesn't. `sendConnectionRequest()` today produces a `__CONN_REQ__` Message that would go out as opaque `SendMessage` content — bypassing the relay's server-side connection-request state (pending lists for offline recipients, cancel semantics). Same for group registry ops, which invite links depend on.

**Change.** Preferred shape: have the **core** tag outgoing control frames so the bridge doesn't re-parse content. Extend the `InternetMessage` record (UDL + `lib.rs:1728-1738`):

```udl
dictionary InternetMessage {
  string message_id;
  string recipient_id;
  sequence<u8> data;
  string? reply_to_msg;
  string? control_op;      // e.g. "conn_req", "group_created" — null for normal traffic
  string? control_payload; // the JSON payload after the prefix, cleartext
};
```

`internet_get_next_message` (`lib.rs:3009`) already deserializes the Message; detect the known prefixes there. Bridge translation table (send relay-native instead of `SendMessage` when `control_op` matches; still `confirmSent(messageId)` on socket-write success):

| control_op (content prefix) | Relay-native op sent |
|---|---|
| `__CONN_REQ__{sender_name, key_package?, initial_message?}` | `SendConnectionRequest {recipient, sender_name, key_package?, initial_message?}` |
| `__CONN_ACC__{accepter_name, key_package?}` | `AcceptConnectionRequest {requester_id: recipient, accepter_name, key_package?}` |
| `__CONN_REJ__` | `RejectConnectionRequest {requester_id: recipient}` |
| `__CONN_CAN__` | `CancelConnectionRequest {recipient}` |
| `__GROUP_CREATED__{group_id, name}` | `CreateGroup {group_id, name}` |
| `__GROUP_MEMBER_ADDED__{group_id, user_id}` | `AddGroupMember {group_id, username: user_id}` |
| `__GROUP_MEMBER_REMOVED__{group_id, user_id}` | `RemoveGroupMember {group_id, username: user_id}` |
| (leave) | `LeaveGroup {group_id}` |

Everything else — `__MLS_*`, `__TYPING__`, `__READ_RECEIPT__`, `__PRESENCE__`, `__MLS_ENC__`, plain chat — continues verbatim as `SendMessage` (verified working carrier today; the round-trip is lossless because the inbound side already maps relay-native events back to the same prefixes).

**Group content messages: do NOT translate.** `meshSendGroupMessage` emits one `__GROUP_MSG__` Message *per member*; translating each to relay-native `SendGroupMessage` would multiply fan-out (N members × server fan-out). Keep per-member delivery for v1. Membership/registry ops *must* translate (above) because invite links require the relay to know the group and its admins — this replaces fernweh's manual "mesh-first group sync" (`useWebSocketRelay.ts:1723-1770`).

Match the exact payload field names against the core's control-message builders (grep the prefix constants in `crates/offline-protocol/src/`) — the table above reflects the relay-side field names from fernweh's client (`useWebSocketRelay.ts:1902-1975, 1714-1835`).

### WI-6 (bridge + TS) — Generic server-command channel

**Problem.** The invite-link lifecycle (10 message types: `CreateGroupInviteLink`, `RevokeGroupInviteLink`, `JoinGroupViaInvite`, `GetGroupInviteLinkPreview`, `AckGroupInviteJoin`, and their responses/pendings) plus misc server events (`GroupRoleChanged`, `GroupRenamed`, `GroupDeleted`, legacy `TypingUpdate`, …) are app/server concerns that don't belong as first-class SDK APIs — but they must ride the SDK's socket or fernweh keeps a second one.

**Change.**

- RN method `internetSendRawCommand(json: string): Promise<boolean>` → validates JSON, sends verbatim when connected+authenticated (else returns false).
- The `processReceivedData` `else` branch (`InternetManager.kt:866-871`) — plus explicitly unhandled types (`GroupInviteLink*`, `*InviteJoin*`, `GroupRoleChanged`, `GroupRenamed`, `GroupDeleted`, `TypingUpdate`, `MessageRead`) — emits an RN event `internetServerMessage { json: string }` instead of dropping.
- TS: `sendRawServerCommand(json)`, `addListener('internet_server_message', …)` in `bindings/react-native/src/index.ts` + `types.ts`.

fernweh ports its invite-link request/response correlation (request_id matching, 25s/90s/8s timeouts, early-complete) onto this channel — ~300 lines of portable JS instead of the 2,300-line client.

### WI-7 (bridge, minor) — `MessageSent` reconciliation

`MessageSent {message_id, recipient}` from the relay is logged only (`InternetManager.kt:515-534`). Optional: correlate server-generated ids with local ones for telemetry. Not load-bearing; do last.

### WI-8 — iOS parity

Every bridge WI (2, 4, 5, 6, 7 + the WI-3 handler) must be mirrored in `bindings/react-native/ios/InternetManager.swift` (structure parallels the Kotlin file ~1:1). Budget this as equal effort; the historical bugs here have been "fixed on Android only."

### WI-9 — Surface plumbing

- UDL: `internet_peer_presence`, extended `InternetMessage`; regenerate uniffi bindings for android/ios.
- RN module: `checkInternetPresence`, `internetSendRawCommand`, `internetServerMessage` event registration.
- TS: new methods/events/types; **rebuild `lib/`** (`npm run build`) so compiled `.d.ts` matches `src/` before fernweh consumes it via `file:`.

### WI-10 — fernweh migration (separate repo, after SDK ships)

1. Pass `authToken: <JWT>` in `MeshCoreContext` internet config, and re-set it on token refresh (module wiring exists). ⚠️ Today the socket authenticates with the *username* as token (`authToken ?: deviceId` fallback, `deviceId = userId = storedUsername`) and the relay accepts it — an impersonation hole. Passing the real JWT fixes the SDK-side half; relay-side enforcement is out of scope here but should be tracked.
2. Feature-by-feature cutover from relay contexts to SDK events: messages/receipts/typing/presence/connection-requests/groups → existing SDK events (+ new `presence_updated.last_seen_ms`); invite links → raw channel port.
3. Keep the JS relay client behind a kill switch during rollout (dual sockets for one user is today's production state, so it's a known-safe configuration), then delete `useWebSocketRelay.ts`, the dual-send paths in `WebSocketRelayContext.tsx`, and the relay/SDK dedup logic.

---

## 2. Verification checklist (run against the production relay before/while building)

1. **Carrier check:** two SDK-only clients (no JS relay socket): authenticate, establish MLS session (`__MLS_WELCOME__` through `SendMessage`), exchange messages, receipts, typing. Evidence says this works today (online welcomes deliver via the SDK socket); make it a repeatable integration test.
2. **DeliveryError timing:** send to an offline recipient; confirm the relay emits `DeliveryError` promptly and note whether it *also* queues the message server-side (mailbox). This determines how much client-side parking carries the offline-delivery UX.
3. **Welcome budget:** with WI-1+2, welcome to an offline peer leaves `attempt` unchanged; peer online + `CheckPresence` roundtrip → delivery without app involvement.
4. **Offline connection requests:** verify the relay stores `SendConnectionRequest` for offline recipients and replays on connect (the JS flow implies it does) — this is what makes WI-5 translation strictly better than opaque passthrough.
5. **Invite links over the raw channel** against the production relay, including the admin-approval (`AckGroupInviteJoin`) flow.
6. **Legacy interop during rollout:** a JS-relay sender's `SendGroupMessage` reaches an SDK-only client via `GroupMessageReceived → __GROUP_MSG__` (already implemented), and vice versa.

## 3. Rollout order

- **Phase A — WI-1..4.** Fixes internet-path session establishment for offline peers (today's top reliability bug) and is independently shippable regardless of the fernweh migration.
- **Phase B — WI-5..9.** Feature parity + surfaces.
- **Phase C — WI-10.** fernweh cutover and deletion.
