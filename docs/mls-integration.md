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
| `pendingQueue.pendingTtlMs` | `120000` | TTL for encrypted messages queued before session readiness |
| `pendingQueue.overflowPolicy` | `drop_oldest` | Overflow policy: `drop_oldest` or `drop_newest` |

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

// Send encrypted group message (MLS encryption + mesh fan-out)
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
other store that can outlive the app container. State writes must be atomic.

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
- Only admins can invite, remove members, or change roles — enforced at the protocol level
- The last-admin invariant prevents orphaned groups
- Removed members receive a notification and should clean up local group state
- Rotate group keys periodically
- Consider re-creating groups for maximum security after member removal

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
