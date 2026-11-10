# Identity and addressing for integrators

**Purpose:** One short guide for application developers. It explains which
strings go in `profile`, which go in `recipient`, and where to get a peer's
`off1…` address. It does not replace the wire spec.

**Related (deeper detail):**

| Document | Role |
|----------|------|
| [UPGRADING §14](UPGRADING.md#14-your-identity-is-derived-not-chosen-v0210) | Migration from `userId` and legacy storage |
| [spec/identity.md](spec/identity.md) | Derivation, canonical form, security margins |
| [mls-integration.md](mls-integration.md) | MLS sessions keyed by peer address |
| [Integrator map](integrator-map.md) | Platform entry and lifecycle |

---

## Three names (do not confuse them)

| Name | Example | On the wire? | Used for |
|------|---------|--------------|----------|
| **`appId`** | `my-chat-app` | No | Application id, telemetry, storage namespace |
| **`profile`** | `default`, `user123` | **No** | Local account slot on **this** device only |
| **Mesh address** | `off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn` | **Yes** | `recipient`, group membership, `sender` on frames |

Your device does **not** choose its address. The SDK derives it from an
identity key it generates and stores per `(appId, profile)`. Peers verify a
claimed address against the key material the peer presents.

---

## The rule

**Anything that names another device for routing or crypto** (`recipient`,
`inviteeUserId`, `otherUserId`, MLS group member ids, `respondToServiceRequest`
`requester`, and so on) takes that peer's **`off1…` address**, not:

- your own `profile`,
- a login username,
- a phone number,
- or a shortened nickname like `bob` (unless you built your own resolver on
  top; the SDK does not ship one).

**`profile` is only for `ProtocolConfig` / constructor config** to pick which
stored identity this install runs as. It never substitutes for a peer address.

---

## How to get addresses

### This device

After `start()` (or when the engine reports readiness):

| Binding | API / event |
|---------|-------------|
| React Native / TypeScript | `await protocol.localAddress()`; `identity_ready` event |
| Native Swift / Kotlin | `localAddress()` on the engine; restored events on `start()` |
| Python | Manager address APIs per [Python README](../bindings/python/README.md) |
| Rust | Engine surface that exposes the derived address |

Until the address is known, `localAddress()` may return `null`. UI that shows
"my id" should wait for `identity_ready` or poll after start.

### A peer

| Source | Field |
|--------|--------|
| Discovery | `neighbor_discovered` → `peer_id` |
| Incoming message | `message_received` → `sender` |
| Invite / QR / deep link | Your app's exchange payload (must carry `off1…`) |
| HTTP front aliases | Map display names to addresses in **your** config (see [http-front](spec/http-front.md)) |

Store the address your product shows to users separately from the mesh address
if you need human-readable labels. The protocol routes on `off1…` only.

---

## Copy-paste patterns

### React Native / TypeScript

```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  profile: 'default', // local account slot — never the peer address
});

protocol.on('neighbor_discovered', (e) => {
  console.log('Peer address:', e.peer_id); // off1…
});

protocol.on('identity_ready', (e) => {
  console.log('My address:', e.address);
});

await protocol.start();

const peerAddress = 'off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn'; // from discovery, not a placeholder forever

await protocol.sendMessage({
  recipient: peerAddress,
  content: 'Hello',
});
```

The 44-character strings in tests and docs (for example
`off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn`) are **real address-shaped
values** used as illustrations. They only work between two devices that actually
hold the matching keys. In production, always use addresses from discovery or
your invite flow.

### Rust (engine)

```rust
// profile in config — local only
let config = ProtocolConfig::builder("my-app", "default")
    .build()?;

// recipient — peer's off1… address
let peer = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn";
protocol.send_message(peer, "Hello!", Some(MessagePriority::High), None::<String>)?;
```

---

## Common mistakes

| Mistake | Symptom | Fix |
|---------|---------|-----|
| `recipient: profile` or `recipient: 'user456'` | No delivery, encrypt failures, empty sessions | Use peer `off1…` from discovery |
| Show `profile` in "share my id" UI | Other devices cannot reach you | Show `localAddress()` |
| Reinstall app, expect same address without restoring keys | New `off1…`, old peers still have the old one | Treat as new device; migrate via UPGRADING §14 |
| Invite flow stores username only | Handshake never completes | Exchange and store `off1…` |

---

## Reinstall, backup, and multiple accounts

- **Same `appId` + `profile` + intact MLS storage:** address stays stable across
  restarts.
- **New install or wiped secure storage:** new identity key → **new `off1…`**.
  Peers must learn the new address through discovery or your invite channel.
- **Multiple profiles on one device:** each profile has its own address and MLS
  state. `profile` selects which one runs; it is not a peer id.

---

## Where APIs still say `userId`

Some generated and native APIs keep parameter names like `otherUserId` or
`inviteeUserId` from older releases. The **value** is still the peer's mesh
address.

---

## React Native: development warnings

`@offline-protocol/mesh-sdk` logs a **console warning** (never throws) when a
peer id passed to send, invite, MLS, or service APIs does not **look like** a
44-character `off1…` string. Warnings are **on by default in `__DEV__` builds**
and **off in production** unless you opt in.

```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  profile: 'default',
  devPeerAddressWarnings: true, // force on in release while debugging
  // devPeerAddressWarnings: false, // silence in development
});
```

The check is shape-only (same idea as username-vs-address rejection in core),
not a checksum parse. Export `looksLikeMeshAddress` if you want the same rule
in your own UI before calling the SDK.

---

## See also

- [configuration.md](configuration.md) — `appId`, `profile`, and storage namespaces
- [react-native-integration.md](react-native-integration.md) — events and `localAddress`
- [mobile-permissions.md](mobile-permissions.md) — platform setup (orthogonal to identity)
