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
import { OfflineProtocol } from '@anthropic/offline-protocol';

const protocol = new OfflineProtocol({
  appId: 'my-app',
  userId: 'alice',
  // Encryption is enabled by default!
  // To disable: encryption: { enabled: false }
});

await protocol.start();

// Initialize MLS with secure storage (required once)
await protocol.initializeMlsWithSecureStorage();

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
  }
});
```

| Option | Default | Description |
|--------|---------|-------------|
| `enabled` | `true` | Enable automatic encryption/decryption |
| `autoKeyExchange` | `true` | Automatically exchange key packages on peer discovery |
| `storePending` | `true` | Queue messages when no session exists (sent after session established) |

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
│  └────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

## Quick Start

### 1. Initialize MLS

The SDK includes built-in secure storage using platform-native APIs (iOS Keychain, Android EncryptedSharedPreferences). Just call initialize:

```swift
// iOS - uses Keychain automatically
try protocol.initializeMlsWithSecureStorage()
```

```kotlin
// Android - uses EncryptedSharedPreferences automatically
protocol.initializeMlsWithSecureStorage()
```

### 2. Generate and Share Key Packages

Key packages allow others to initiate encrypted sessions with you:

```swift
// Generate a key package
let keyPackage = try protocol.mlsGenerateKeyPackage()

// Upload to your server for distribution
uploadKeyPackage(keyPackage.keyPackageData, userId: keyPackage.userId)
```

### 3. Send Encrypted Messages

#### 1:1 Messaging

```swift
// First, import the recipient's key package
try protocol.mlsImportKeyPackage(
    userId: "bob",
    keyPackageData: bobsKeyPackage
)

// Send an encrypted message
let encrypted = try protocol.mlsEncryptForUser(
    otherUserId: "bob",
    plaintext: "Hello, Bob!".data(using: .utf8)!
)

// Send the encrypted message using existing transport
protocol.sendMessage(
    recipient: "bob",
    content: encryptedToJson(encrypted),
    priority: .medium
)
```

#### Group Messaging

```swift
// Create a group
let group = try protocol.mlsCreateGroup(groupName: "Project Team")

// Add members (need their key packages)
let welcome = try protocol.mlsAddGroupMember(
    groupId: group.groupId,
    memberKeyPackage: aliceKeyPackage
)

// Send the welcome to the new member
sendWelcome(welcome, to: "alice")

// Send encrypted group message
let encrypted = try protocol.mlsEncryptForGroup(
    groupId: group.groupId,
    plaintext: "Hello team!".data(using: .utf8)!
)
```

### 4. Receive and Decrypt Messages

```swift
// When receiving a message, check if it's encrypted
if message.metadata["mls_encrypted"] == "true" {
    let encrypted = parseEncryptedMessage(message.content)
    if let plaintext = try protocol.mlsDecrypt(encrypted: encrypted) {
        let text = String(data: plaintext, encoding: .utf8)
        // Handle decrypted message
    }
}

// When receiving a Welcome message (invited to group)
if let welcomeData = message.metadata["mls_welcome"] {
    let welcome = parseWelcomeMessage(welcomeData)
    let groupInfo = try protocol.mlsProcessWelcome(welcome: welcome)
    // Now you're part of the group
}
```

---

## Custom Storage (Advanced)

The SDK includes built-in secure storage, but you can provide a custom implementation if needed (e.g., for custom backup strategies, additional encryption layers, or testing).

### Using Custom Storage

```swift
// iOS - custom storage
let customStorage = MyCustomMlsStorage()
try protocol.initializeMls(storage: customStorage)
```

```kotlin
// Android - custom storage
val customStorage = MyCustomMlsStorage()
protocol.initializeMls(customStorage)
```

### Implementing MlsStorageProvider

To create a custom storage provider, implement the `MlsStorageProvider` protocol:

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
let pending = protocol.mlsGetPendingKeyPackages()

for pkg in pending {
    // Upload to server
    try await uploadKeyPackage(pkg)
    
    // Mark as synced
    try protocol.mlsMarkKeyPackageSynced(packageId: pkg.packageId)
}

// Fetch a contact's key package before messaging
let keyPackageData = try await fetchKeyPackage(userId: "bob")
try protocol.mlsImportKeyPackage(userId: "bob", keyPackageData: keyPackageData)
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

### Group Security

- Use MLS's built-in member removal to ensure forward secrecy
- Rotate group keys periodically
- Consider re-creating groups for maximum security after member removal

### Message Format

Encrypted messages use the existing `Message` structure with metadata:

```json
{
  "content": "<base64-encoded MLS ciphertext>",
  "metadata": {
    "mls_encrypted": "true",
    "mls_group_id": "group:abc123",
    "mls_epoch": "5",
    "mls_message_type": "Application"
  }
}
```

---

## API Reference

### Initialization

| Method | Description |
|--------|-------------|
| `initializeMlsWithSecureStorage()` | Initialize MLS with built-in platform secure storage (recommended) |
| `initializeMls(storage)` | Initialize MLS with custom storage provider |
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

### Groups

| Method | Description |
|--------|-------------|
| `mlsCreateGroup(name)` | Create a new group |
| `mlsAddGroupMember(groupId, keyPackage)` | Add member (returns Welcome) |
| `mlsRemoveGroupMember(groupId, memberId)` | Remove member |
| `mlsLeaveGroup(groupId)` | Leave a group |
| `mlsEncryptForGroup(groupId, plaintext)` | Encrypt for group |
| `mlsDecryptFromGroup(encrypted)` | Decrypt from group |
| `mlsJoinGroup(welcome)` | Join group from Welcome |
| `mlsListGroups()` | List all groups |
| `mlsGetGroupInfo(groupId)` | Get group information |

### Generic

| Method | Description |
|--------|-------------|
| `mlsDecrypt(encrypted)` | Decrypt any message |
| `mlsProcessWelcome(welcome)` | Process any Welcome message |
