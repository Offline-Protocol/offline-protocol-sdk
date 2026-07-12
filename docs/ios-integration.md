# iOS Integration Guide

This guide shows how to integrate the Offline Protocol SDK into a native iOS app (Swift).

## Prerequisites

- Xcode 15+
- Rust toolchain with iOS targets

## Setup

### 1. Install Rust iOS Targets

```bash
rustup target add aarch64-apple-ios
rustup target add x86_64-apple-ios    # For simulator
rustup target add aarch64-apple-ios-sim  # For M1 simulator
```

### 2. Build Rust Library

```bash
cd crates/offline-protocol-uniffi

# Build for iOS devices
cargo build --release --target aarch64-apple-ios

# Build for simulator
cargo build --release --target aarch64-apple-ios-sim
```

Each build produces `liboffline_protocol_uniffi.a` under `target/<triple>/release/`
(the crate's `[lib] name` is `offline_protocol_uniffi`).

### 3. Package the Library and Generate Bindings

The recommended path is the helper script, which builds the device/simulator slices **and**
regenerates the UniFFI Swift bindings:

```bash
# From bindings/react-native
./scripts/build-uniffi-ios.sh
```

It produces, under `bindings/react-native/ios/`:

- `liboffline_protocol_uniffi_device.a` (device)
- `liboffline_protocol_uniffi_sim.a` (simulator, fat)
- `Generated/offline_protocol.swift` and `Generated/offline_protocolFFI.modulemap`

Add the `.a` for your build target plus `offline_protocol.swift` to your Xcode project. To
do it by hand instead, `lipo`-combine the per-arch `liboffline_protocol_uniffi.a` outputs
and run `uniffi-bindgen generate --language swift` to produce the Swift bindings.

### 4. Use in Swift

The generated `offline_protocol.swift` declares the public types (`OfflineProtocol`,
`ProtocolConfig`, `EventCallback`, `MessagePriority`, …). When it compiles into your app
target no extra `import` is needed — the low-level FFI is exposed to it as the
`offline_protocolFFI` clang module referenced from the generated file.

```swift
import Foundation

// Events are delivered as JSON strings — implement `EventCallback` to receive them.
final class MeshEventHandler: EventCallback {
    func onEvent(eventJson: String) {
        guard let data = eventJson.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = obj["type"] as? String else { return }

        switch type {
        case "message_received":
            let sender = obj["sender"] as? String ?? "?"
            let content = obj["content"] as? String ?? ""
            print("Received from \(sender): \(content)")
        case "transport_switched":
            print("Transport switched: \(eventJson)")
        default:
            break
        }
    }
}

final class MeshController {
    private var offlineProtocol: OfflineProtocol?
    private let eventHandler = MeshEventHandler()

    func startMesh() {
        // ProtocolConfig has 16 required fields (+ 4 with defaults). See
        // docs/configuration.md for what each one controls.
        let config = ProtocolConfig(
            appId: "my-ios-app",
            userId: "user123",
            bleEnabled: true,
            wifiDirectEnabled: false,   // iOS does not support Wi-Fi Direct
            internetEnabled: true,
            reticulumEnabled: false,
            nostrEnabled: false,
            preferOnline: false,
            initialTtl: 8,
            encryptionEnabled: true,
            autoKeyExchange: true,
            storePending: true,
            maxPendingPerPeer: 100,
            maxPendingGlobal: 1000,
            pendingTtlMs: 604_800_000,   // 7 days
            overflowPolicy: .dropOldest
            // requireEncryption (true), maxGroupMembers (256), groupRelayEnabled (true),
            // and requireTransportIdentity (false) use their defaults.
        )

        do {
            let mesh = try OfflineProtocol(config: config)
            mesh.setEventCallback(callback: eventHandler)
            try mesh.start()

            // Send a message (priority is required; replyToMsg is optional)
            let messageId = try mesh.sendMessage(
                recipient: "user456",
                content: "Hello from iOS!",
                priority: .high,
                replyToMsg: nil
            )
            print("Message sent: \(messageId)")

            offlineProtocol = mesh
        } catch {
            print("Error: \(error)")
        }
    }

    func stopMesh() {
        try? offlineProtocol?.stop()
    }
}
```

## Permissions

Add to `Info.plist`:

```xml
<!-- Bluetooth permissions -->
<key>NSBluetoothAlwaysUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices</string>

<key>NSBluetoothPeripheralUsageDescription</key>
<string>This app uses Bluetooth to communicate with nearby devices</string>

<!-- Background modes (REQUIRED for reliable BLE operation) -->
<!-- Without these, iOS will throttle/stop BLE scanning and advertising -->
<!-- causing missed discoveries and false "peer lost" events -->
<key>UIBackgroundModes</key>
<array>
    <string>bluetooth-central</string>
    <string>bluetooth-peripheral</string>
</array>
```

## Distribution

The SDK ships as an npm package for React Native apps:

```bash
npm install @offline-protocol/mesh-sdk
```

For a **native** iOS app (no React Native), integrate the Rust core directly via the
manual XCFramework build in steps 1–4 above: build the `offline-protocol-uniffi` static
libraries for the device and simulator targets, package them into an XCFramework, and add
it to your Xcode project alongside the UniFFI-generated Swift bindings.

> A standalone CocoaPods pod and a Swift Package Manager manifest are not currently
> published — use the npm module (React Native) or the manual XCFramework path (native).

## Architecture

```
iOS App (Swift)
    ↓
UniFFI Generated Bindings (Swift)
    ↓
Rust Core (100% safe)
```

## Group Messaging (MLS-Encrypted Mesh)

Create and manage encrypted groups over the mesh. The group creator is automatically an admin.

```swift
// Create a group
let group = try offlineProtocol.createGroup(groupName: "Project Team")

// Invite a member (admin only)
try offlineProtocol.inviteToGroup(groupId: group.groupId, inviteeUserId: "bob")

// Send an encrypted group message
let messageIds = try offlineProtocol.sendGroupMessage(
    groupId: group.groupId,
    content: "Hello team!",
    priority: nil,
    replyToMsg: nil
)

// Remove a member (admin only)
try offlineProtocol.removeFromGroup(groupId: group.groupId, memberId: "bob")

// Get group info (members, epoch, etc.)
if let info = try offlineProtocol.getGroupInfo(groupId: group.groupId) {
    print("Members: \(info.members)")
}

// Rename a group (admin only)
try offlineProtocol.renameGroup(groupId: group.groupId, newName: "New Team Name")

// Leave a group
try offlineProtocol.leaveGroup(groupId: group.groupId)
```

### Group Role Management

Groups use role-based access control: **Admin** and **Member**.

```swift
// Promote a member to admin (admin only)
try offlineProtocol.setMemberRole(groupId: groupId, userId: "bob", role: "admin")

// Check a member's role
let role = try offlineProtocol.getMemberRole(groupId: groupId, userId: "bob") // "admin" or "member"

// Get all roles
let roles = try offlineProtocol.getGroupRoles(groupId: groupId)
// ["alice": "admin", "bob": "admin", "charlie": "member"]
```

Role changes and renames arrive through the same `EventCallback` as every other event —
JSON strings whose `type` is `group_role_changed` or `group_renamed`. Handle them inside
your `onEvent(eventJson:)`:

```swift
func onEvent(eventJson: String) {
    guard let data = eventJson.data(using: .utf8),
          let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let type = obj["type"] as? String else { return }

    switch type {
    case "group_role_changed":
        print("\(obj["user_id"] ?? "?") is now \(obj["new_role"] ?? "?") (by \(obj["changed_by"] ?? "?"))")
    case "group_renamed":
        print("Group \(obj["group_id"] ?? "?") renamed to \(obj["new_name"] ?? "?") by \(obj["renamed_by"] ?? "?")")
    default:
        break
    }
}
```

**Security invariants:**
- Only admins can invite, remove members, change roles, or rename groups
- The last admin cannot be demoted, removed, or leave (prevents orphaned groups)
- If the last admin disconnects unexpectedly, a deterministic election promotes the next admin

## TOFU Trust Management

Reset a peer's TOFU-pinned public key when you need to re-establish trust (e.g., the peer reinstalled the app):

```swift
// Reset trust pin for a peer
let removed = try offlineProtocol.resetTofuForPeer(peerId: "bob")
// removed == true if an entry was cleared, false if none existed
```

After reset, the next message from that peer will establish a new trust pin.

## Platform Limitations

iOS does **not** support Wi-Fi Direct. Available transports:
- Bluetooth Low Energy
- Internet
- Wi-Fi Direct (Android only) - Not available

## Performance

- Message sending: <1ms overhead
- Memory safe: Zero buffer overflows or memory leaks
- Battery efficient: Optimized for iOS power management
