# MLS End-to-End Encryption Integration

This guide explains how to integrate MLS (Message Layer Security) end-to-end encryption into your app using the Offline Protocol SDK.

## Overview

The SDK provides end-to-end encryption via the MLS protocol (RFC 9420). MLS provides:

- **Forward secrecy**: Past messages remain secure even if keys are compromised
- **Post-compromise security**: Future messages become secure after key updates
- **Efficient group messaging**: Scalable encryption for groups of any size
- **1:1 messaging**: Direct encrypted conversations using 2-person groups

## Architecture

```
┌─────────────────────────────────────────────┐
│              Your App                       │
│  ┌────────────────────────────────────────┐ │
│  │   MlsStorageProvider Implementation    │ │
│  │   (iOS: Keychain, Android: Keystore)   │ │
│  └────────────────────────────────────────┘ │
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
│  └────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

## Quick Start

### 1. Initialize MLS with Storage

Before using encryption, you must initialize MLS with a storage provider:

```swift
// iOS
let storage = KeychainMlsStorage()
try protocol.initializeMls(storage: storage)
```

```kotlin
// Android
val storage = KeystoreMlsStorage(context)
protocol.initializeMls(storage)
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

## Implementing MlsStorageProvider

You must implement the `MlsStorageProvider` protocol to provide secure key storage.

### iOS Implementation (Keychain)

```swift
import Security
import Foundation

class KeychainMlsStorage: MlsStorageProvider {
    private let service = "com.yourapp.mls"
    
    func store(keyType: String, keyId: String, data: Data) throws {
        let key = "\(keyType):\(keyId)"
        
        // Delete any existing item
        let deleteQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key
        ]
        SecItemDelete(deleteQuery as CFDictionary)
        
        // Add new item
        let addQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        
        let status = SecItemAdd(addQuery as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw MlsStorageError.storeFailed
        }
    }
    
    func load(keyType: String, keyId: String) throws -> Data? {
        let key = "\(keyType):\(keyId)"
        
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        
        switch status {
        case errSecSuccess:
            return result as? Data
        case errSecItemNotFound:
            return nil
        default:
            throw MlsStorageError.loadFailed
        }
    }
    
    func delete(keyType: String, keyId: String) throws {
        let key = "\(keyType):\(keyId)"
        
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key
        ]
        
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw MlsStorageError.deleteFailed
        }
    }
    
    func listKeys(keyType: String) throws -> [String] {
        let prefix = "\(keyType):"
        
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecReturnAttributes as String: true,
            kSecMatchLimit as String: kSecMatchLimitAll
        ]
        
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        
        switch status {
        case errSecSuccess:
            guard let items = result as? [[String: Any]] else {
                return []
            }
            return items.compactMap { item -> String? in
                guard let account = item[kSecAttrAccount as String] as? String,
                      account.hasPrefix(prefix) else {
                    return nil
                }
                return String(account.dropFirst(prefix.count))
            }
        case errSecItemNotFound:
            return []
        default:
            throw MlsStorageError.loadFailed
        }
    }
}
```

### Android Implementation (EncryptedSharedPreferences)

```kotlin
import android.content.Context
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import android.util.Base64

class KeystoreMlsStorage(context: Context) : MlsStorageProvider {
    
    private val masterKey = MasterKey.Builder(context)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()
    
    private val sharedPreferences = EncryptedSharedPreferences.create(
        context,
        "mls_secure_storage",
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
    )
    
    private fun makeKey(keyType: String, keyId: String): String = "$keyType:$keyId"
    
    @Throws(MlsStorageError::class)
    override fun store(keyType: String, keyId: String, data: ByteArray) {
        try {
            val key = makeKey(keyType, keyId)
            val encoded = Base64.encodeToString(data, Base64.NO_WRAP)
            sharedPreferences.edit().putString(key, encoded).apply()
            
            // Also track the key in the index
            val indexKey = "index:$keyType"
            val existingKeys = sharedPreferences.getStringSet(indexKey, mutableSetOf()) ?: mutableSetOf()
            val updatedKeys = existingKeys.toMutableSet().apply { add(keyId) }
            sharedPreferences.edit().putStringSet(indexKey, updatedKeys).apply()
        } catch (e: Exception) {
            throw MlsStorageError.StoreFailed()
        }
    }
    
    @Throws(MlsStorageError::class)
    override fun load(keyType: String, keyId: String): ByteArray? {
        return try {
            val key = makeKey(keyType, keyId)
            val encoded = sharedPreferences.getString(key, null) ?: return null
            Base64.decode(encoded, Base64.NO_WRAP)
        } catch (e: Exception) {
            throw MlsStorageError.LoadFailed()
        }
    }
    
    @Throws(MlsStorageError::class)
    override fun delete(keyType: String, keyId: String) {
        try {
            val key = makeKey(keyType, keyId)
            sharedPreferences.edit().remove(key).apply()
            
            // Remove from index
            val indexKey = "index:$keyType"
            val existingKeys = sharedPreferences.getStringSet(indexKey, mutableSetOf()) ?: mutableSetOf()
            val updatedKeys = existingKeys.toMutableSet().apply { remove(keyId) }
            sharedPreferences.edit().putStringSet(indexKey, updatedKeys).apply()
        } catch (e: Exception) {
            throw MlsStorageError.DeleteFailed()
        }
    }
    
    @Throws(MlsStorageError::class)
    override fun listKeys(keyType: String): List<String> {
        return try {
            val indexKey = "index:$keyType"
            sharedPreferences.getStringSet(indexKey, emptySet())?.toList() ?: emptyList()
        } catch (e: Exception) {
            throw MlsStorageError.LoadFailed()
        }
    }
}
```

### React Native Implementation

For React Native, implement the storage in native code and expose it via the native module:

```typescript
// TypeScript types
interface MlsStorageProvider {
  store(keyType: string, keyId: string, data: Uint8Array): Promise<void>;
  load(keyType: string, keyId: string): Promise<Uint8Array | null>;
  delete(keyType: string, keyId: string): Promise<void>;
  listKeys(keyType: string): Promise<string[]>;
}

// Usage in React Native
import { NativeModules } from 'react-native';

const { OfflineProtocolModule } = NativeModules;

// The native module handles storage internally using platform-native secure storage
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
| `initializeMls(storage)` | Initialize MLS with storage provider |
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
