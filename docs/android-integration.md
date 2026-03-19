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

Copy the compiled `.so` files to your Android project:

```
android/app/src/main/jniLibs/
├── arm64-v8a/liboffline_protocol.so
├── armeabi-v7a/liboffline_protocol.so
└── x86_64/liboffline_protocol.so
```

### 4. Use in Kotlin

```kotlin
import com.offlineprotocol.OfflineProtocol

class MainActivity : AppCompatActivity() {
    private lateinit var protocol: OfflineProtocol

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Initialize protocol
        val config = ProtocolConfig(
            appId = "my-android-app",
            userId = "user123"
        )

        protocol = OfflineProtocol(config)
        protocol.start()

        // Set up event listener
        protocol.setEventListener { event ->
            when (event) {
                is Event.MessageReceived -> {
                    Log.d("Protocol", "Received: ${event.content}")
                }
                is Event.TransportSwitched -> {
                    Log.d("Protocol", "Switched to ${event.to}")
                }
                else -> {}
            }
        }

        // Send a message
        val messageId = protocol.sendMessage(
            recipient = "user456",
            content = "Hello from Android!",
            priority = MessagePriority.HIGH
        )
    }

    override fun onDestroy() {
        super.onDestroy()
        protocol.stop()
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
val group = protocol.meshCreateGroup("Project Team")

// Invite a member (admin only)
protocol.meshInviteToGroup(group.groupId, "bob")

// Send an encrypted group message
val messageIds = protocol.meshSendGroupMessage(
    groupId = group.groupId,
    content = "Hello team!"
)

// Remove a member (admin only)
protocol.meshRemoveFromGroup(group.groupId, "bob")

// Get group info (members, epoch, etc.)
val info = protocol.getGroupInfo(group.groupId)
info?.let { println("Members: ${it.members}") }

// Rename a group (admin only)
protocol.renameGroup(group.groupId, "New Team Name")

// Leave a group
protocol.meshLeaveGroup(group.groupId)
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

Listen for role changes and renames:

```kotlin
protocol.setEventListener { event ->
    when (event) {
        is Event.GroupRoleChanged -> {
            Log.d("Protocol", "${event.userId} is now ${event.newRole} (by ${event.changedBy})")
        }
        is Event.GroupRenamed -> {
            Log.d("Protocol", "Group ${event.groupId} renamed to ${event.newName} by ${event.renamedBy}")
        }
        else -> {}
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

