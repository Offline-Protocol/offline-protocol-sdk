# Android Integration Guide

This guide shows how to integrate the Offline Protocol SDK into a native Android app (Kotlin/Java).

## Prerequisites

- Android Studio
- Rust toolchain with Android targets
- NDK installed

## Setup

### 1. Install Rust Android Targets

```bash
rustup target add aarch64-linux-android
rustup target add armv7-linux-androideabi
rustup target add x86_64-linux-android
```

### 2. Build Rust Library

```bash
cd crates/offline-protocol-uniffi

# Build for all Android architectures
cargo build --release --target aarch64-linux-android
cargo build --release --target armv7-linux-androideabi
cargo build --release --target x86_64-linux-android
```

### 3. Copy Native Libraries

Each build produces `liboffline_protocol_uniffi.so`. UniFFI's Kotlin loader looks for
`libuniffi_offline_protocol.so`, so copy each ABI's output under **that** name:

```
android/app/src/main/jniLibs/
├── arm64-v8a/libuniffi_offline_protocol.so
├── armeabi-v7a/libuniffi_offline_protocol.so
└── x86_64/libuniffi_offline_protocol.so
```

The `bindings/react-native/scripts/build-uniffi-android.sh` helper builds every ABI and
renames automatically (and regenerates the Kotlin bindings).

### 4. Use in Kotlin

The generated Kotlin bindings live in the `uniffi.offline_protocol` package, so import
that (not `com.offlineprotocol.*`, which is the React Native wrapper).

```kotlin
import uniffi.offline_protocol.*
import org.json.JSONObject

class MainActivity : AppCompatActivity() {
    private lateinit var protocol: OfflineProtocol

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // ProtocolConfig has 16 required fields (+ 4 with defaults). See
        // docs/configuration.md for what each one controls.
        val config = ProtocolConfig(
            appId = "my-android-app",
            userId = "user123",
            bleEnabled = true,
            wifiDirectEnabled = true,
            internetEnabled = true,
            reticulumEnabled = false,
            nostrEnabled = false,
            preferOnline = false,
            initialTtl = 8.toUByte(),
            encryptionEnabled = true,
            autoKeyExchange = true,
            storePending = true,
            maxPendingPerPeer = 100.toULong(),
            maxPendingGlobal = 1_000.toULong(),
            pendingTtlMs = 604_800_000.toULong(),
            overflowPolicy = OverflowPolicy.DROP_OLDEST,
            // requireEncryption (true), maxGroupMembers (256u), groupRelayEnabled (true),
            // and requireTransportIdentity (false) use their defaults.
        )

        protocol = OfflineProtocol(config)

        // Events are delivered as JSON strings via the EventCallback interface.
        protocol.setEventCallback(MeshEventHandler())
        protocol.start()

        // Send a message (priority is required; replyToMsg is optional)
        val messageId = protocol.sendMessage(
            recipient = "user456",
            content = "Hello from Android!",
            priority = MessagePriority.HIGH,
            replyToMsg = null,
        )
    }

    override fun onDestroy() {
        super.onDestroy()
        protocol.stop()
    }
}

class MeshEventHandler : EventCallback {
    override fun onEvent(eventJson: String) {
        val obj = JSONObject(eventJson)
        when (obj.optString("type")) {
            "message_received" ->
                Log.d("Protocol", "Received from ${obj.optString("sender")}: ${obj.optString("content")}")
            "transport_switched" ->
                Log.d("Protocol", "Transport switched: $eventJson")
        }
    }
}
```

## Permissions

Add to `AndroidManifest.xml`:

```xml
<!-- Bluetooth permissions -->
<uses-permission android:name="android.permission.BLUETOOTH" />
<uses-permission android:name="android.permission.BLUETOOTH_ADMIN" />
<uses-permission android:name="android.permission.BLUETOOTH_SCAN" />
<uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />

<!-- Location (required for BLE scanning on Android) -->
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />

<!-- Wi-Fi Direct permissions -->
<uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
<uses-permission android:name="android.permission.CHANGE_WIFI_STATE" />
<uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES" />

<!-- Internet -->
<uses-permission android:name="android.permission.INTERNET" />
```

## Group Messaging (MLS-Encrypted Mesh)

Create and manage encrypted groups over the mesh. The group creator is automatically an admin.

```kotlin
// Create a group
val group = protocol.createGroup("Project Team")

// Invite a member (admin only)
protocol.inviteToGroup(group.groupId, "bob")

// Send an encrypted group message
val messageIds = protocol.sendGroupMessage(
    groupId = group.groupId,
    content = "Hello team!",
    priority = null,
    replyToMsg = null,
)

// Remove a member (admin only)
protocol.removeFromGroup(group.groupId, "bob")

// Get group info (members, epoch, etc.)
val info = protocol.getGroupInfo(group.groupId)
info?.let { println("Members: ${it.members}") }

// Rename a group (admin only)
protocol.renameGroup(group.groupId, "New Team Name")

// Leave a group
protocol.leaveGroup(group.groupId)
```

### Group Role Management

Groups use role-based access control: **Admin** and **Member**.

```kotlin
// Promote a member to admin (admin only)
protocol.setMemberRole(groupId, "bob", "admin")

// Check a member's role
val role = protocol.getMemberRole(groupId, "bob") // "admin" or "member"

// Get all roles
val roles = protocol.getGroupRoles(groupId)
// mapOf("alice" to "admin", "bob" to "admin", "charlie" to "member")
```

Role changes and renames arrive through the same `EventCallback` as every other event —
JSON strings whose `type` is `group_role_changed` or `group_renamed`. Extend your
`onEvent(eventJson:)` to handle them:

```kotlin
override fun onEvent(eventJson: String) {
    val obj = JSONObject(eventJson)
    when (obj.optString("type")) {
        "group_role_changed" ->
            Log.d("Protocol", "${obj.optString("user_id")} is now ${obj.optString("new_role")} (by ${obj.optString("changed_by")})")
        "group_renamed" ->
            Log.d("Protocol", "Group ${obj.optString("group_id")} renamed to ${obj.optString("new_name")} by ${obj.optString("renamed_by")}")
    }
}
```

**Security invariants:**
- Only admins can invite, remove members, change roles, or rename groups
- The last admin cannot be demoted, removed, or leave (prevents orphaned groups)
- If the last admin disconnects unexpectedly, a deterministic election promotes the next admin

## TOFU Trust Management

Reset a peer's TOFU-pinned public key when you need to re-establish trust (e.g., the peer reinstalled the app):

```kotlin
// Reset trust pin for a peer
val removed = protocol.resetTofuForPeer("bob")
// removed == true if an entry was cleared, false if none existed
```

After reset, the next message from that peer will establish a new trust pin.

## Architecture

```
Android App (Kotlin)
    ↓
UniFFI Generated Bindings (Kotlin)
    ↓
Rust Core (100% safe)
```

## Performance

- Message sending: <1ms overhead
- Memory safe: Zero buffer overflows or memory leaks
- Battery efficient: Optimized BLE and relay logic
