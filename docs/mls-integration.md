# MLS End-to-End Encryption Integration

This guide explains how to integrate MLS (Message Layer Security) end-to-end encryption into your app using the Offline Protocol SDK.

## Overview

The SDK provides end-to-end encryption via the MLS protocol (RFC 9420). MLS provides:

- **Forward secrecy**: Past messages remain secure even if keys are compromised
- **Post-compromise security**: Future messages become secure after key updates
- **Efficient group messaging**: Scalable encryption for groups of any size
- **1:1 messaging**: Direct encrypted conversations using 2-person groups

## Auto-Encryption (Recommended)

**New in v0.2.0**: The SDK now supports automatic encryption/decryption. When enabled (default), messages are transparently encrypted before sending and decrypted on receive—no manual MLS API calls needed.

### How It Works

```
┌─────────────────────────────────────────────────────────────────┐
│                    Auto-Encryption Flow                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. PEER DISCOVERY                                              │
│     ┌──────┐  neighbor_discovered   ┌──────┐                    │
│     │ Alice│ ─────────────────────► │ Bob  │                    │
│     └──────┘  ◄───────────────────  └──────┘                    │
│               exchange key packages                             │
│                                                                 │
│  2. FIRST MESSAGE                                               │
│     ┌──────┐  create session + welcome  ┌──────┐                │
│     │ Alice│ ──────────────────────────► │ Bob  │               │
│     └──────┘  encrypted "Hello!"         └──────┘               │
│                                                                 │
│  3. REPLY                                                       │
│     ┌──────┐  encrypted "Hi!"    ┌──────┐                       │
│     │ Alice│ ◄────────────────── │ Bob  │                       │
│     └──────┘                     └──────┘                       │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Quick Start with Auto-Encryption

```typescript
// React Native
import { OfflineProtocol } from '@offline-protocol/mesh-sdk';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'alice',
  // Encryption is enabled by default!
  // To disable: encryption: { enabled: false }
});

await protocol.start();

// MLS is auto-initialized on start() when encryption is enabled (default).
// Optional: call explicitly if you want to initialize earlier.
// await protocol.initializeMlsWithSecureStorage();

// Just send messages - encryption happens automatically!
await protocol.sendMessage({
  recipient: 'bob',
  content: 'Hello Bob!',  // Automatically encrypted
});

// Receive messages - decryption happens automatically!
protocol.on('message_received', (event) => {
  console.log(event.content);       // Already decrypted
  console.log(event.encrypted);     // true if was encrypted
});
```

### Configuration Options

```typescript
const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'alice',
  encryption: {
    // Enable automatic encryption (default: true)
    enabled: true,
    
    // Auto-exchange key packages when peers are discovered (default: true)
    autoKeyExchange: true,
    
    // Queue messages if no session exists yet (default: true)
    storePending: true,

    // Require encrypted delivery or fail send (default: true, fail-closed)
    requireEncryption: true,
  }
});
```

| Option | Default | Description |
|--------|---------|-------------|
| `enabled` | `true` | Enable automatic encryption/decryption |
| `autoKeyExchange` | `true` | Automatically exchange key packages on peer discovery |
| `storePending` | `true` | Queue messages when no session exists (sent after session established) |
| `requireEncryption` | `true` | Enforce encrypted delivery (send fails closed if encryption cannot be applied) |
| `pendingQueue.maxPendingPerPeer` | `64` | Per-peer cap for encrypted messages received before session readiness |
| `pendingQueue.maxPendingGlobal` | `4096` | Global cap for encrypted messages received before session readiness |
| `pendingQueue.pendingTtlMs` | `1800000` | TTL (30 min) for encrypted messages held before session readiness |
| `pendingQueue.overflowPolicy` | `drop_oldest` | Overflow policy: `drop_oldest` or `drop_newest` |
| `compactEnvelopeEnabled` | `true` | Emit the compact MLS envelope to recipients that advertise `env_versions` |
| `richPayloadEnabled` | `true` | Seal rich extras inside the MLS ciphertext for recipients that advertise `rich_versions` |
| `cryptoRecoveryEnabled` | `true` | Recover an undecryptable 1:1 message instead of dropping it and ACKing anyway ([below](#crypto-failure-recovery)) |

The `pendingTtlMs` default is 30 minutes, not the 2 minutes earlier releases
used: under the deferred-ACK model a message held here is not delivery-ACKed on
receipt, so this queue is the primary recovery window before the session
confirms. Memory stays bounded by the per-peer and global caps plus the
`drop_oldest` policy — a longer TTL lets entries linger within those caps, it
does not raise the ceiling.

Encryption is required by default (fail-closed): outbound sends fail without transport
transmission if encryption cannot be applied, instead of silently degrading to plaintext.
Set `requireEncryption: false` to explicitly opt in to plaintext operation — each
plaintext send then emits a `security_warning` event with the `PLAINTEXT_SEND` reason
code (once per peer). Connection bootstrap APIs (connection requests, key packages,
service discovery) are internal plaintext control messages and are exempt from strict
mode — they continue to work.

Queued encrypted messages received before session readiness use bounded storage with deterministic eviction:
- Enforced limits: per-peer + global
- Monotonic TTL expiration
- Explicit overflow policy (`drop_oldest` or `drop_newest`)
- Structured warning logs and queue pressure metrics

### What Happens Automatically

When auto-encryption is enabled:

1. **On peer discovery**: Key packages are automatically exchanged via BLE/WiFi Direct
2. **On first message**: If no session exists but we have the recipient's key package, a session is created and a Welcome message is sent automatically
3. **On send**: Messages are encrypted before transmission
4. **On receive**: 
   - Key package messages → stored for future sessions
   - Welcome messages → session is joined, pending messages are flushed
   - Encrypted messages → decrypted and delivered to your app

### When to Use Manual MLS APIs

Use the manual MLS APIs (described below) when you need:

- **Server-side key distribution**: Upload/download key packages from a server
- **Custom group management**: Create and manage encrypted groups
- **Advanced session control**: Manual session lifecycle management
- **Offline key exchange via QR codes**: Generate key packages for QR sharing

---

## Architecture

```
┌─────────────────────────────────────────────┐
│              Your App                       │
│                     │                       │
│                     ▼                       │
│  ┌────────────────────────────────────────┐ │
│  │       OfflineProtocol SDK              │ │
│  │  ┌──────────────────────────────────┐  │ │
│  │  │         MLS Manager              │  │ │
│  │  │  - Key generation                │  │ │
│  │  │  - Encryption/Decryption         │  │ │
│  │  │  - Session management            │  │ │
│  │  │  - Group management              │  │ │
│  │  └──────────────────────────────────┘  │ │
│  │  ┌──────────────────────────────────┐  │ │
│  │  │     Built-in Secure Storage      │  │ │
│  │  │  - iOS: Keychain                 │  │ │
│  │  │  - Android: EncryptedPrefs       │  │ │
│  │  └──────────────────────────────────┘  │ │
│  │  ┌──────────────────────────────────┐  │ │
│  │  │ App-Container Protocol State     │  │ │
│  │  │ - Outbox / pending messages      │  │ │
│  │  │ - Retry / delivery lifecycles    │  │ │
│  │  └──────────────────────────────────┘  │ │
│  └────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

## Quick Start

### 1. Initialize MLS

MLS initialization requires two storage providers with different lifecycles:

- `MlsStorageProvider` stores cryptographic material in Keychain, Keystore-backed
  encrypted preferences, or another credential store.
- `ProtocolStateStorageProvider` stores restartable delivery state inside the app
  container. It must be removed on app deletion and must not use a credential store.

The split is about *lifecycle*, not trust: some delivery state (queued message
plaintext, cloud-media `encryption_key`/`iv`) is as sensitive as anything in the
credential store. The SDK therefore seals those record values with a per-install
AEAD key held in `MlsStorageProvider` before they reach the protocol-state
provider — so the provider only ever sees ciphertext, and an app container lifted
without the credential store yields nothing. See
[Protocol-State Confidentiality](#protocol-state-confidentiality).

```swift
try mesh.initializeMls(
    secureStorage: keychainStorage,
    protocolStateStorage: appContainerStorage
)
```

```kotlin
protocol.initializeMls(encryptedStorage, appContainerStorage)
```

> **React Native:** the wrapper supplies both providers, so apps can call
> `initializeMlsWithSecureStorage()` — or let `start()` initialize MLS automatically
> when encryption is enabled.

### 2. Generate and Share Key Packages

Key packages allow others to initiate encrypted sessions with you:

```swift
// Generate a key package
let keyPackage = try mesh.mlsGenerateKeyPackage()

// Upload to your server for distribution
uploadKeyPackage(keyPackage.keyPackageData, userId: keyPackage.userId)
```

### 3. Send Encrypted Messages

#### 1:1 Messaging

```swift
// First, import the recipient's key package
try mesh.mlsImportKeyPackage(
    userId: "bob",
    keyPackageData: bobsKeyPackage
)

// Send an encrypted message
let encrypted = try mesh.mlsEncryptForUser(
    otherUserId: "bob",
    plaintext: "Hello, Bob!".data(using: .utf8)!
)

// Send the encrypted message using existing transport
_ = try mesh.sendMessage(
    recipient: "bob",
    content: encryptedToJson(encrypted),
    priority: .medium,
    replyToMsg: nil
)
```

#### Group Messaging

```swift
// Create a group (creator becomes admin)
let group = try mesh.createGroup(groupName: "Project Team")

// Invite members (admin only — handles key exchange + Welcome automatically)
try mesh.inviteToGroup(groupId: group.groupId, inviteeUserId: "alice")

// Send encrypted group message. Returns one id per recipient: the same MLS
// ciphertext is fanned out per member, so each copy carries its own outbox
// entry, ACK, and retry ladder.
let messageIds = try mesh.sendGroupMessage(
    groupId: group.groupId,
    content: "Hello team!",
    priority: nil,
    replyToMsg: nil
)

// Get group info
if let info = try mesh.getGroupInfo(groupId: group.groupId) {
    print("Members: \(info.members), Epoch: \(info.epoch)")
}

// Rename a group (admin only, broadcasts to all members)
try mesh.renameGroup(groupId: group.groupId, newName: "Engineering Team")
```

### 4. Receive and Decrypt Messages

```swift
// When receiving a message, check if it was encrypted
if message.metadata["encrypted"] == "true" {
    let encrypted = parseEncryptedMessage(message.content)
    if let plaintext = try mesh.mlsDecrypt(encrypted: encrypted) {
        let text = String(data: plaintext, encoding: .utf8)
        // Handle decrypted message
    }
}

// When receiving a Welcome message (invited to group)
if let welcomeData = message.metadata["mls_welcome"] {
    let welcome = parseWelcomeMessage(welcomeData)
    let groupInfo = try mesh.mlsProcessWelcome(welcome: welcome)
    // Now you're part of the group
}
```

---

## Custom Storage (Advanced)

The React Native wrapper ships both built-in providers. Native Swift/Kotlin
integrations provide an `MlsStorageProvider` for key material and a
`ProtocolStateStorageProvider` for app-container state. Both interfaces expose
the same CRUD operations, but they are intentionally different types so their
lifecycles cannot be wired accidentally.

### Using Custom Storage

```swift
let secureStorage = MyCustomMlsStorage()
let stateStorage = MyAppContainerStateStorage()
try mesh.initializeMls(
    secureStorage: secureStorage,
    protocolStateStorage: stateStorage
)
```

```kotlin
val secureStorage = MyCustomMlsStorage()
val stateStorage = MyAppContainerStateStorage()
protocol.initializeMls(secureStorage, stateStorage)
```

### Implementing the Providers

Implement `MlsStorageProvider` for secure material. Implement
`ProtocolStateStorageProvider` with the same methods for app-container state:

```swift
// iOS Custom Implementation
class MyCustomMlsStorage: MlsStorageProvider {
    func store(keyType: String, keyId: String, data: Data) throws {
        // Store data securely
    }
    
    func load(keyType: String, keyId: String) throws -> Data? {
        // Load data, return nil if not found
    }
    
    func delete(keyType: String, keyId: String) throws {
        // Delete data
    }
    
    func listKeys(keyType: String) throws -> [String] {
        // Return all key IDs for the given type
    }
}
```

```kotlin
// Android Custom Implementation
class MyCustomMlsStorage : MlsStorageProvider {
    override fun store(keyType: String, keyId: String, data: ByteArray) {
        // Store data securely
    }
    
    override fun load(keyType: String, keyId: String): ByteArray? {
        // Load data, return null if not found
    }
    
    override fun delete(keyType: String, keyId: String) {
        // Delete data
    }
    
    override fun listKeys(keyType: String): List<String> {
        // Return all key IDs for the given type
    }
}
```

Do not implement the protocol-state provider with Keychain,
EncryptedSharedPreferences backed by a surviving Keystore namespace, or any
other store that can outlive the app container.

**If you supply your own `MlsStorageProvider`, upgrading installs will not
inherit their pre-split delivery state.** On first launch the SDK sweeps
outbox entries, pending messages, lifecycles, the Lamport clock, and the block
list out of secure storage and into protocol-state storage — but it enumerates
them through the `MlsStorageProvider` it is given. The built-in providers find
them because they read through to the pre-namespace store they replaced; a
custom provider has no such fallback, so the sweep finds nothing and that state
stays where it is. It is not lost — it is simply never picked up, and the
install comes up with an empty outbox, an empty pending queue, and an **empty
block list**. If you have shipped a custom provider and are upgrading across
this release, have it read through to wherever your previous version wrote, or
migrate that data yourself before calling `initializeMls`.

**A custom provider is also not covered by the logout wipe.** React Native's
`wipePersistedState(appId, userId)` erases the *built-in* stores for one account
— it has no handle on a container it did not create. If you supply your own
providers, erase them yourself when the user signs out; otherwise that account's
outbox is restored and re-driven on the next sign-in, and its MLS identity
survives indefinitely. See
[UPGRADING §10](./UPGRADING.md#logging-out-and-switching-accounts) for what the
built-in wipe covers and in what order, which is the behaviour to mirror.

**Protocol-state values are `ByteArray` / `Data` / `bytes`**, not the
element-wise sequence `MlsStorageProvider` uses. That interface carries key
material a few hundred bytes at a time; these records reach megabytes, where a
boxed-per-element representation costs on the order of a million short-lived
objects per call on Kotlin. Store and return the bytes verbatim — never inspect,
re-encode, or truncate them, since the sensitive categories arrive sealed.

**Writes must be atomic *and* durable before `store` returns.** The SDK treats a
successful `store` as persisted and immediately writes state that depends on it
— most sharply the per-install record-sealing key, after which sealed records
start landing in the protocol-state container. A rename that commits ahead of
its data blocks, or an `apply()` that only staged the write in memory, can crash
into a container full of records whose key was never written. The built-in
providers use `AtomicFile` on Android, `commit()` for encrypted preferences, and
an fsync of the file *and* its parent directory on iOS and Python.

**`load` must bound its read.** Check the stored entry's size *before*
materializing it and never allocate — or hand back across the FFI — more than
8 MiB. The SDK refuses to write anything near that, so a larger entry is corrupt
or tampered. This obligation cannot live in the SDK, because by the time it can
check a length the provider has already allocated the bytes. `listKeys` should
stay bounded for the same reason; the SDK caps every category well below any
sane ceiling.

**Report a record you had to destroy as `CorruptedData`, not as absence.**
Destruction and absence are different answers. When a provider drops an entry it
can never decode — oversized, truncated, framing that does not parse — the SDK
settles it: a lost outbox entry emits a terminal `message_failed`, because the
application is holding the id `sendMessage` returned for it, and a lost pending
queue emits a `pending_state_lost` diagnostic for the recipient. Returning
`null` instead is accepted, but it is indistinguishable from a record that was
never written, so the application is told nothing and that id never resolves.
Reserve `CorruptedData` for permanent losses — a transient read failure is
`LoadFailed`, which the SDK leaves in place for a later launch rather than
settling.

**If you address entries by filename, do not encode the key into the name.**
Key ids are peer and message ids, so an encoding is both case-sensitive (`AAG`
and `AAa` become the same file on a case-insensitive volume — APFS's macOS
default — and one record silently overwrites the other) and unbounded (a
long-but-valid id overruns the 255-byte `NAME_MAX`). Use a fixed-length
lowercase digest and record the exact key inside the entry. The built-in
providers do exactly this, in a format shared across iOS, Android, and Python:
a `"OPS1"` magic, big-endian `u16` lengths for `keyType` and `keyId`, both keys
in UTF-8, then the value bytes.

### Protocol-State Confidentiality

A protocol-state provider is a byte store, not a trusted one. Store and return
the bytes you are handed **verbatim** — do not inspect, re-encode, compress, or
truncate them.

The SDK seals the record values that can carry message plaintext or media key
material — pending session messages, outbox entries, and media transfer
descriptors — with ChaCha20-Poly1305 under a per-install key kept in
`MlsStorageProvider` (key type `protocol_state_record_key`). Each record's
associated data binds it to its `(keyType, keyId)` slot, so a record cannot be
moved between peers or categories by anyone with write access to the container.
Record *keys* are not sealed: they are peer and message ids, and the store needs
them in the clear to address entries. Built-in providers name files by digest,
but each record carries its own `(keyType, keyId)` in its header, so treat the
key as readable by anyone who can read the container.

**What this does and does not hide.** Sealing protects message content and media
key material. It does not protect the peer graph. These categories are stored in
the clear, and for the marker-style ones the key *is* the whole content:

| Category | In the clear |
| --- | --- |
| `blocked_users` | which peers you have blocked |
| `both_create_awaiting_decrypt` | which peers are mid-handshake |
| `session_states`, `welcome_lifecycles` | which peers you have sessions with, and their delivery state |
| `peer_key_packages`, `peer_capabilities` | which peers you have exchanged with (public wire material) |
| sealed categories' *keys* | which peers you have queued messages for, and their message ids |

Before the storage split this metadata sat behind the OS keystore, so an
attacker with app-container access but no credential-store access learned
nothing from it; now they learn the graph. That is the deliberate cost of giving
delivery state the container's lifecycle. If your threat model includes an
attacker who can read the app container of an unlocked device, treat the peer
graph as exposed.

`blocked_users` in particular is *deliberately* left unsealed rather than folded
into one sealed record. Sealing fails closed, and a block list that silently
stops persisting whenever the record key is unavailable is a worse failure than
a readable one: blocking is a safety control, and it must survive every state in
which the SDK still runs.

**Sealing is confidentiality, not integrity — and only the sealed categories get
even that.** A sealed record is authenticated: its AEAD tag covers the value and
its associated data binds it to its `(keyType, keyId)` slot, so an edited or
relocated record does not open and is dropped. The categories in the table above
carry no such protection. They are written in the clear and read back at face
value, so an attacker who can *write* the app container of an unlocked device
gets more than the peer graph:

| Category | What a write buys | Consequence |
| --- | --- | --- |
| `blocked_users` | delete a marker | that peer is silently unblocked on the next launch |
| `blocked_users` | add many markers | the list is restored up to `MAX_BLOCKED_USERS` and stops there, so blocking keeps working — before that bound was applied, planted markers could push the set past the live cap and make every new `block_user` fail |
| `both_create_awaiting_decrypt` | delete a marker | the owner gate that requires a group-aware decrypt before confirming a peer is gone, so a stale plaintext probe can confirm a session the handshake never converged |
| `session_states` | write `Confirmed` | a peer whose session was still pending is treated as confirmed |
| `session_states` | **delete** a record | *(fixed)* the peer used to drop out of the confirmed set on the next launch, re-opening the inbound plaintext gate for them. The gate no longer reads this category — see below |
| `welcome_lifecycles` | edit state or retry schedule | Welcome delivery can be stalled or forced to retry |

Note the two `session_states` rows are not the same attack, and the second is
the one that mattered. Writing `Confirmed` makes this node *stricter*. Deleting
the record made it more permissive: restore treats an absent record as `Pending`
— it must, or a single unreadable record would brick initialization — so the
peer silently left the confirmed set, and the inbound plaintext gate, which used
to ask "is this peer's session confirmed?", opened for them. With
`requireEncryption: false` that admitted unauthenticated cleartext under an
attacker-chosen sender.

**Sealing would not have fixed that**, which is worth stating plainly because it
is the intuitive move: a seal authenticates bytes that are *present*, and this
attack removes them. The fix was to stop deriving the answer from a deletable
record. The gate now asks whether the peer is known to run MLS at all, sourced
from the MLS session list and the durable encryption-capability records — both in the credential store,
which a container write cannot reach.

This also means **container write access can no longer be used to forge message
content**, which an earlier version of this document claimed outright and should
not have. Substituting a cached record in `peer_key_packages` was enough to make
this node build a session around an attacker's leaf — MLS keys never left the
credential store, but they did not need to. That category is now sealed *and*
every use of a key package is checked by re-deriving the address from its leaf
signature key.

On stock iOS and Android the app container is writable only by the app itself,
so this matters on a rooted or jailbroken device, or wherever else your threat
model grants an attacker filesystem write. If it does, do not treat
`blocked_users` as durable security state: re-derive it from a source you do
trust. Sealing these remaining categories is *not* the fix — for the
marker-style ones the fact lives in the record **key**, which is never sealed,
so sealing the (empty) value is decorative; and for `blocked_users` it would
make the list fail closed, which is strictly worse (see the paragraph above).

Consequences worth knowing:

- **Sealing does not protect against deletion or rollback.** The AEAD tag covers
  a record's value and binds it to its slot, so an edit or a relocation is
  caught. Nothing binds a record to a *version* or to the set it belongs to, so
  removing one, or restoring an earlier copy of one, is undetectable. Adding
  that would need a manifest over the whole category plus a monotonic counter in
  the credential store — a credential-store write on every protocol-state write,
  which is far more expensive than the durable-write budget this restore path is
  already built around. So it is a known limitation, not an oversight: do not
  build a control that must survive a deletion on top of sealing.
- **Fail closed.** If the per-install key cannot be read or written, those
  categories are not persisted at all for that session rather than written in the
  clear. Delivery still works from memory; only crash recovery is lost. Records
  already on disk are left alone, not deleted — a later launch that can read the
  key recovers them.
- **The key is the container's undo button.** Clearing the credential store
  without clearing the app container leaves records that no longer open; the SDK
  drops them on read. Clearing the app container alone is the normal uninstall
  path and is always safe.
- **A dropped record is reported, not swallowed.** Anything the app was told was
  queued gets settled when it cannot be recovered: an unrecoverable outbox entry
  emits `message_failed`, and an unrecoverable pending queue emits a
  `convergence_diag` with stage `pending_state_lost` naming the recipient (its
  message ids are inside the record that would not open, so they cannot be named
  individually). These are emitted on `start()`, not during `initialize_mls`, so
  install your event callback before starting if you want to observe them.
- **But only a record that is actually gone is settled.** A record that merely
  could not be read *this session* — the seal key was unavailable, or the store
  refused one read — stays on disk and produces no event. Settling it would be a
  terminal answer the next launch overturns by restoring the entry and re-driving
  delivery, so you would see `message_failed` and then a delivery. Do not treat a
  quiet startup as proof that everything restored; treat `message_failed` as
  proof that something did not.
- **Records have a size ceiling.** The SDK refuses to write, and refuses to
  deserialize on restore, any single record over 4 MiB — a corrupted or tampered
  state file cannot become an unbounded allocation during startup.

### React Native

```typescript
import { NativeModules } from 'react-native';

const { OfflineProtocolModule } = NativeModules;

// Initialize with built-in secure storage (recommended)
await OfflineProtocolModule.initializeMlsWithSecureStorage();

// Generate key package
const keyPackage = await OfflineProtocolModule.mlsGenerateKeyPackage();

// Encrypt message
const encrypted = await OfflineProtocolModule.mlsEncryptForUser('bob', plaintext);
```

---

## Key Package Distribution

Key packages must be shared with other users before they can send you encrypted messages.

### Server-Side Storage

Your server should expose endpoints for key package management:

```
POST /keys/{userId}           # Upload key package (authenticated)
GET  /keys/{userId}           # Fetch user's key package
DELETE /keys/{userId}/{pkgId} # Delete used key package
```

### Syncing Key Packages

```swift
// Get pending key packages to upload
let pending = mesh.mlsGetPendingKeyPackages()

for pkg in pending {
    // Upload to server
    try await uploadKeyPackage(pkg)
    
    // Mark as synced
    try mesh.mlsMarkKeyPackageSynced(packageId: pkg.packageId)
}

// Fetch a contact's key package before messaging
let keyPackageData = try await fetchKeyPackage(userId: "bob")
try mesh.mlsImportKeyPackage(userId: "bob", keyPackageData: keyPackageData)
```

### Offline Key Exchange

When offline, key packages can be exchanged via:

1. **QR Code**: Encode the key package as a QR code
2. **BLE/WiFi Direct**: Send via mesh transport
3. **NFC**: Tap-to-share (future)

---

## Security Considerations

### Key Storage

- Always use platform-native secure storage (Keychain/Keystore)
- Keys should be device-bound when possible
- Consider biometric protection for sensitive operations

### Key Package Lifecycle

- Key packages are one-time use for forward secrecy
- Generate multiple key packages and upload to server
- Delete used key packages from server after session creation

### Group Roles and Permissions

Groups use a role-based permission model:

- **Admin** — Can invite/remove members, change roles, and send messages. The group creator is automatically an admin.
- **Member** — Can send and receive messages but cannot manage the group.

The SDK enforces a **last-admin invariant**: the last remaining admin cannot be demoted, removed, or leave the group. If the last admin leaves unexpectedly (e.g., crash), a deterministic election promotes the lexicographically smallest member ID to admin.

```typescript
// Set a member's role (admin only)
await protocol.meshSetMemberRole(groupId, userId, 'admin');

// Query roles
const role = await protocol.meshGetMemberRole(groupId, userId); // "admin" | "member"
const allRoles = await protocol.meshGetGroupRoles(groupId);     // { alice: "admin", bob: "member" }

// Listen for role changes
protocol.on('group_role_changed', (event) => {
  console.log(`${event.user_id} → ${event.new_role} (by ${event.changed_by})`);
});
```

### Group Security

- Use MLS's built-in member removal to ensure forward secrecy
- Role changes and group renames are admin-gated **on receive** — a non-admin's role or rename frame is rejected by every peer
- Invite and removal are admin-gated **on send**; see [Group authorization model](#group-authorization-model) for what that does and does not guarantee
- The last-admin invariant prevents orphaned groups
- Removed members receive a notification and should clean up local group state
- Rotate group keys periodically
- Consider re-creating groups for maximum security after member removal

### Group authorization model

The admin/member roles are an **application-layer overlay on top of MLS**, not
an MLS feature. MLS itself has no notion of an admin: [RFC 9420 §3.2](https://www.rfc-editor.org/rfc/rfc9420.html#section-3.2)
notes that any member being *able* to evict another "does not necessarily imply
that any member is actually allowed to evict other members; groups can enforce
access control policies on top of these basic mechanisms." The SDK's roles are
that policy layer.

What this means concretely:

| Operation | Enforced on send | Enforced on receive |
| --- | --- | --- |
| `set_member_role` | Yes | **Yes** — non-admin role frames are dropped |
| `rename_group` | Yes | **Yes** — non-admin rename frames are dropped |
| `invite_to_group` (MLS Add commit) | Yes | **No** (opt-in — see below) |
| `remove_from_group` (MLS Remove commit) | Yes | **No** (opt-in — see below) |

Membership changes travel as authenticated MLS commits. MLS verifies that the
committer is a genuine group member — an outsider cannot forge one — but the
SDK does **not** refuse a commit from a member who is not an admin. A group
member running a modified client can therefore add or remove anyone, and the
change is cryptographically real: an added member can read all subsequent group
traffic, and a removed member is cut off from it.

This is a deliberate trade-off, not an oversight. Rejecting a commit means
refusing to merge it, which advances everyone else's epoch but not yours —
permanently forking you from the group, with no recovery short of the app
re-inviting you. Because admin state is replicated best-effort (a role change is
a mesh notification, and a joiner receives a point-in-time snapshot), a member
whose role metadata merely *lagged* would partition itself out of a healthy
group with no attacker involved. An unrecoverable partition is a worse failure
mode than an insider membership change, so the SDK applies the change and
reports it instead.

Unauthorized changes are surfaced, not silent:

```typescript
protocol.on('group_unauthorized_membership_change', (event) => {
  // event.committer made a membership change they were not authorized to make.
  // event.added / event.removed list the affected members (sorted).
  // event.reason is 'sender_not_admin' or 'affected_member_mismatch'.
  // event.enforced is false by default: the change HAS been applied and an
  // admin can undo it. It is true only when enforce_admin_commits refused the
  // commit, in which case nothing changed locally — but this device is now an
  // epoch behind the group and needs re-inviting.
});
```

The corresponding `group_member_added` / `group_member_removed` events still
fire (your roster must not diverge from MLS state) and carry an `authorized`
field so a single handler can render the distinction inline. The field is
tri-state: `true` (passed the local admin check), `false` (judged
unauthorized), or **absent** when authorization was not evaluated on that
path — your own join from a Welcome, relay reconciliation frames, or an
older core. Only a present value is a positive statement either way.

Reports are rate-limited per `(group, committer)`: a repeat within a short
window does not re-emit `group_unauthorized_membership_change` (divergent
role metadata would otherwise re-fire it on every commit), but every
affected roster event still carries `authorized: false`.

**How the local replica is built.** A member's view of who is an admin comes
from two sources, in order: the per-member roles it has stored, and — only when
it has *no* admin role stored at all — the group creator, who is always an
admin. A joiner receives both in its Welcome: a point-in-time snapshot of the
inviter's roles, and the inviter's record of the group creator. The creator
field is what keeps an incomplete snapshot from collapsing to
deny-everyone: without it, a joiner whose snapshot arrived empty would judge
*every* member — including the real admin — unauthorized. It is adopted only
when the joiner has no creator on record already (first write wins), so a later
invite cannot rewrite an established admin fallback, and it is absent from
Welcomes sent by SDKs older than this field, where absence means "no
information" rather than "no creator".

**The signal can false-positive.** "Unauthorized" is judged against the
receiving member's *local replica* of role state — the same best-effort
metadata that makes receive-side enforcement unsafe. A member whose role map
lags (a joiner's point-in-time snapshot, a missed role-change notification, a
divergent auto-promotion) will report a perfectly legitimate change — for
example a voluntary leave committed by a member it does not yet know is an
admin — and different members can disagree about the same commit. Treat one
member's report as a suspicion, not a verdict; if you aggregate these events
(telemetry, moderation dashboards), corroborate across members — a change
flagged by every member is almost certainly real, one flagged by a single
member is usually role-metadata lag.

**Known limitation:** the member *removed* by an unauthorized Remove receives
no `group_unauthorized_membership_change` event. It can no longer decrypt the
commit that removed it, and it deliberately refuses to trust a non-admin's
unencrypted claim that it was removed — so it simply stops receiving group
traffic, and may later surface an epoch-fork signal instead. Only the
remaining members report the unauthorized removal. Closing this requires
replicating the admin set with the group state, which is planned follow-up
work.

**Opt-in enforcement.** `GroupConfig::enforce_admin_commits` (RN:
`group.enforceAdminCommits`, default `false`) makes the SDK *refuse* a
membership commit it cannot authorize, rather than applying and reporting it.
The refusal happens before the MLS merge, so nothing changes locally and the
`group_unauthorized_membership_change` event carries `enforced: true` with no
accompanying roster event.

Everything above about partitions still applies, which is why it is off by
default: a refused commit leaves this device an epoch behind every member that
accepted it, unrecoverable without an app-level re-invite. The check fails open
on absent knowledge — no metadata, no admin role stored, an unreadable roster —
but cannot detect divergent knowledge, so enable it only for a closed
deployment that controls role distribution, and never on part of a fleet. Pure
key-update commits and 1:1 sessions are never gated. See
[Group Configuration](configuration.md#group-configuration) for the full
trade-off.

**Re-inviting is not by itself the remedy.** What enforcement guarantees is
that this device never *merges* an unauthorized commit — not that it never ends
up in a group the commit changed. A re-invite arrives as a Welcome, and a
Welcome is not a commit and is not policy-gated: it readmits the device to the
group as it stands, including the member an unauthorized Add spliced in (whose
roster attribution then carries `authorized: null`, since a Welcome join
evaluates nothing). So on `enforced: true`, resolve the refused change as well
— have an admin remove the intruder, or confirm the removal was legitimate
before restoring the member — rather than re-inviting and considering the
matter closed.

**Guidance for apps that need stronger guarantees:** treat
`group_unauthorized_membership_change` as a moderation alert for a *human*
admin, who can reverse the change with `remove_from_group` /
`invite_to_group`. **Never reverse the change automatically** off a single
member's event — on a false positive, automated reversal would evict a
legitimately added member, and two clients doing so can fight each other. If
your threat model does not tolerate insider membership changes at all, do not
rely on group membership alone for authorization — gate sensitive actions on
an admin-signed application-layer check.

### Message Format

Encrypted messages are transported using the existing `Message` structure.
For auto-decrypted messages surfaced to the app, the SDK sets `metadata.encrypted = "true"`:

```json
{
  "content": "<base64-encoded MLS ciphertext>",
  "metadata": {
    "encrypted": "true"
  }
}
```

MLS control payloads (key package / welcome / ciphertext envelopes) are encoded as internal message content and handled by the SDK before app delivery.

### Capability Negotiation & Payload Formats

The signed key package doubles as a capability advertisement. Alongside the MLS
material, a peer's key package carries version lists that the SDK uses to pick
per-recipient payload formats — all negotiated automatically, nothing for apps
to configure:

| Capability | Advertises | Gates |
|------------|-----------|-------|
| `wire_versions` | Compact binary mesh framing (hop-local; in-memory, re-exchanged on connect) | `binaryWireEnabled` |
| `env_versions` | Compact MLS envelope (base64 binary instead of JSON, ~2.7× smaller) | `encryption.compactEnvelopeEnabled` |
| `rich_versions` | Sealed rich payload (`__RICH_V1__` body inside the MLS plaintext) | `encryption.richPayloadEnabled` |

**Sealed rich payload**: quoted-reply context, rich media metadata (including
cloud-media `encryption_key`/`iv` secrets), and forward attribution are wrapped
around the message text *before* encryption, so the relay and mesh hops never
see them. Toward a recipient without the capability, rich extras are silently
dropped — never sent cleartext. Inbound parsing of every format is always on,
independent of the kill switches.

**Groups**: the same sealed body goes into the group MLS plaintext, but only
when *every* other member is known rich-capable — either from a direct
key-package exchange or attested by their inviter (Add commits and Welcomes
carry the capability map, so members added by someone else stay sealable).
If any member's capability is unknown, the extras drop (the text still sends),
a `group_rich_extras_dropped` event fires with the unknown members, and the SDK
probes them once so a later retry can seal. Apps can pre-check with
`group_rich_readiness(groupId)` (RN: `meshGroupRichReadiness`).

Per-peer `env_versions`/`rich_versions` capabilities persist across restarts;
`wire_versions` is deliberately in-memory (hop-local, re-exchanged on connect).
See [Wire Format Kill Switches](configuration.md#wire-format-kill-switches)
for runtime disable semantics.

### Crypto-Failure Recovery

Distinct from the pending queue above, which covers a session that is *not yet
ready*. This covers an **established** 1:1 session on which a decrypt fails —
most importantly one that has fallen out of epoch sync with the peer (the two
sides disagree on the MLS epoch, e.g. after a fork). Previously such a failure
was delivery-ACKed and the message dropped: silent loss behind an ACK that
claimed delivery.

Gated by `encryption.cryptoRecoveryEnabled` (default `true`), the SDK now
recovers in two tiers.

**Tier 1 — honest failure, then heal.** A decrypt failure withholds the delivery
ACK and un-marks the message id, so the sender keeps retrying rather than
marking it delivered. It does *not* queue the ciphertext — a frame sealed to a
dead epoch, or one whose ratchet generation the failed attempt already consumed,
could never be decrypted however long it waited. Recovery is the sender's
*resend*, not this frame.

When — and only when — the failure is a proven **epoch mismatch**, the receiver
additionally heals the channel: it tears down its own stale session and
advertises a `session_reset` key package; the peer drops its stale session,
rebuilds from that key package, and Welcomes the receiver back. Deleting the
local session is what makes convergence work for both user-id orderings. Re-keys
are rate-limited to one per peer per 30 s.

**Tier 2 — no loss.** The sender keeps per-outbox-entry re-seal provenance
(memory-only; it holds plaintext and is never persisted), so each resend is
re-sealed against the peer's *current* session while preserving the message id
for dedup and ACK correlation. Tier 1 makes the failure honest; Tier 2 is what
makes the message actually arrive.

What is deliberately **excluded from the re-key**: failures that are *not* an
epoch mismatch — an AEAD/authentication failure, a discarded past ratchet
generation, a malformed frame — withhold the ACK exactly like an epoch mismatch
(Tier 1 and Tier 2 both apply, so the sender's re-sealed resend still recovers
the message), but they never trigger a re-key. Widening the *re-key* trigger to
cover them would turn every malformed frame into a session teardown, which is an
unbounded churn vector; withholding an ACK carries no such cost, because the
recovery it enables is the sender's own retry.

**Frames that never reach MLS.** An envelope that fails to *parse* — an
`__MLS_ENC__` payload in no recognized envelope form, or a media envelope whose
encoding does not decode past its magic byte — takes the same disposition under
the same switch: no ACK, and the sender's resend is the recovery path. The frame
is not queued (an unparseable frame can never become parseable), so recovery is
the resend and nothing else. This matters for a sender with an *encoding* bug
rather than a transient corruption: its frames ride the full ACK retry ladder and
then settle as an honest `message_failed`, where previously they were
acknowledged as delivered and silently dropped.

What stays acknowledged is everything that fails *after* a successful decrypt —
an empty or non-UTF-8 plaintext, or a media chunk whose decrypted body does not
parse. Those are terminal: the ratchet generation is spent and a resend would
re-seal the same malformed plaintext, so retrying could never help.

**Event semantics to plan for.** Because these failures no longer settle the
message, `message_decryption_failed` is **advisory** and fires once per failed
*attempt* rather than once per message — bounded by the sender's ACK retry
budget. Treat it as "this attempt did not decrypt". The terminal signals stay
`message_failed` for a DM and `file_receive_failed` for media.

**Security-rejected frames are silent, on both paths.** Two conditions are
refused as illegitimate rather than undeliverable: an envelope naming a session
slot that is not the claimed sender's, and an MLS credential authenticating
someone other than the wire sender. Neither is ACKed and neither is gated by
`cryptoRecoveryEnabled` — an ACK would confirm to whoever injected the frame that
the target is online and processing their traffic. Media behaves exactly like
text here; before this parity fix a media chunk ACKed both conditions, which
leaked precisely what the text path's silence protects.

> **The re-key trigger is unauthenticated, and cannot be made otherwise.** Do not
> read the paragraph above as "only a genuine peer can cause a re-key". An MLS
> epoch is checked during *framing* validation, which OpenMLS performs before any
> AEAD, sender-data or signature check; `__MLS_ENC__` is a data-plane prefix,
> deliberately exempt from the signed control-message gate; and a 1:1 slot id is
> `session:<a>:<b>` over two public user ids. Those three compose: a hand-built
> frame carrying a wrong epoch reaches the recoverable classification with its
> sender still entirely unverified. **Anyone who can inject a frame can drive a
> rate-limited re-key** — no key material, no captured ciphertext, no session, no
> replay. The SDK's own
> `test_forged_frame_reaches_session_desync_without_any_key_material` builds such
> a frame from scratch to keep this statement honest. It is inherent to MLS
> framing, not an OpenMLS defect, so no upgrade changes it, and a sender check
> structurally cannot help — the MLS credential it would compare against only
> exists once decrypt *succeeds*.

The mitigation is therefore that acting on the trigger is **harmless**, not that
it is trusted:

- the re-key is confined to the claimed sender's own session slot, so one
  derivable session id cannot be aimed at other peers;
- the 30 s per-peer floor bounds the churn, and is never reset early by a
  successful decrypt;
- the heal destroys nothing — queued outbound plaintext survives a reset and
  seals against the rebuilt session, and Tier 2 re-seals in-flight resends;
- every re-key raises a `SESSION_REKEY_TRIGGERED` security warning, so a
  sustained rate (injection rather than a real fork) is visible to the app.

**Residual, stated plainly:** an injector can hold one pair in bounded re-key
churn — delivery is delayed, never lost. Closing it outright needs a signed
epoch-corroboration exchange before teardown (a liveness-only probe does not
work: a healthy peer answers and the session is torn down anyway). That is future
work. Apps that want the old behaviour instead can set
`cryptoRecoveryEnabled: false`, which trades this residual back for the silent
drop it replaced.

Media has no Tier 2 — chunks are re-encoded rather than replayed, so an
interrupted transfer recovers through the descriptor-based
`media_resend_required` path instead.

Disabling the switch reverts to the legacy drop-and-ACK behaviour.

> **A missing delivery ACK is not proof of non-delivery.** Both this path and the
> pending queue withhold ACKs while recovering, and a sender that exhausts its
> retry budget before an ACK lands may mark a message undeliverable that the
> recipient already holds. Do not treat ACK absence as loss.

---

## API Reference

> The method names in these tables follow the **React Native / JS wrapper** API — including the
> `mesh*` group helpers and `initializeMlsWithSecureStorage()`. The native UniFFI bindings expose
> the same operations under their generated names (e.g. `createGroup`, `inviteToGroup`,
> `sendGroupMessage`, `initializeMls(secureStorage, protocolStateStorage)`), as shown in the
> Swift/Kotlin snippets above.

### Initialization

| Method | Description |
|--------|-------------|
| `initializeMlsWithSecureStorage()` | Initialize MLS with built-in secure and app-container state storage (recommended) |
| `initializeMls(secureStorage, protocolStateStorage)` | Initialize MLS with custom lifecycle-separated providers |
| `isMlsInitialized()` | Check if MLS is initialized |

### Key Packages

| Method | Description |
|--------|-------------|
| `mlsGenerateKeyPackage()` | Generate a new key package |
| `mlsGetOrCreateKeyPackage()` | Get existing or generate new package |
| `mlsImportKeyPackage(userId, data)` | Import a contact's key package |
| `mlsGetPendingKeyPackages()` | Get packages to upload |
| `mlsMarkKeyPackageSynced(packageId)` | Mark package as uploaded |

### 1:1 Sessions

| Method | Description |
|--------|-------------|
| `mlsHasSession(otherUserId)` | Check if session exists |
| `mlsCreateSession(otherUserId)` | Create session (returns Welcome) |
| `mlsJoinSession(welcome)` | Join session from Welcome |
| `mlsEncryptForUser(userId, plaintext)` | Encrypt for user |
| `mlsDecryptFromUser(encrypted)` | Decrypt from user |
| `mlsListSessions()` | List all sessions |
| `mlsDeleteSession(otherUserId)` | Delete a session |

### Groups (High-Level Mesh API)

| Method | Description |
|--------|-------------|
| `meshCreateGroup(groupName)` | Create a new group (creator is admin) |
| `meshInviteToGroup(groupId, inviteeUserId)` | Invite member (admin only, handles Welcome + Commit) |
| `meshRemoveFromGroup(groupId, memberId)` | Remove member (admin only) |
| `meshLeaveGroup(groupId)` | Leave a group with notification |
| `meshSendGroupMessage(groupId, content)` | Send encrypted message to all members |
| `meshListGroups()` | List all groups (excluding 1:1 sessions) |
| `meshGetGroupInfo(groupId)` | Get group information |
| `meshRenameGroup(groupId, newName)` | Rename a group (admin only) |

### Group Roles (High-Level Mesh API)

| Method | Description |
|--------|-------------|
| `meshSetMemberRole(groupId, userId, role)` | Set a member's role (admin only) |
| `meshGetMemberRole(groupId, userId)` | Get a member's role |
| `meshGetGroupRoles(groupId)` | Get all member roles |

### Generic

| Method | Description |
|--------|-------------|
| `mlsDecrypt(encrypted)` | Decrypt any message |
| `mlsProcessWelcome(welcome)` | Process any Welcome message |
