# iOS Integration Guide

This guide shows how to integrate the Offline Protocol SDK into a native iOS app (Swift).

## Prerequisites

- Xcode 15+
- Rust toolchain with iOS targets
- CocoaPods or Swift Package Manager

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

### 3. Create XCFramework

```bash
# Combine into universal library
lipo -create \\
    target/aarch64-apple-ios/release/liboffline_protocol.a \\
    -output liboffline_protocol.a

# Add to Xcode project under Frameworks
```

### 4. Use in Swift

```swift
import OfflineProtocolSDK

class ViewController: UIViewController {
    var protocol: OfflineProtocol?
    
    override func viewDidLoad() {
        super.viewDidLoad()
        
        // Initialize protocol
        let config = ProtocolConfig(
            appId: "my-ios-app",
            userId: "user123"
        )
        
        do {
            protocol = try OfflineProtocol(config: config)
            try protocol?.start()
            
            // Set up event handler
            protocol?.onEvent { event in
                switch event {
                case .messageReceived(let msg):
                    print("Received: \\(msg.content)")
                case .transportSwitched(let evt):
                    print("Switched to \\(evt.to)")
                default:
                    break
                }
            }
            
            // Send a message
            let messageId = try protocol?.sendMessage(
                recipient: "user456",
                content: "Hello from iOS!",
                priority: .high
            )
            
            print("Message sent: \\(messageId ?? "")")
            
        } catch {
            print("Error: \\(error)")
        }
    }
    
    deinit {
        try? protocol?.stop()
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

## CocoaPods

Add to your `Podfile`:

```ruby
pod 'OfflineProtocolSDK', '~> 0.1'
```

## Swift Package Manager

Add to `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/offline-protocol/sdk", from: "0.1.0")
]
```

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
let group = try protocol.meshCreateGroup(groupName: "Project Team")

// Invite a member (admin only)
try protocol.meshInviteToGroup(groupId: group.groupId, inviteeUserId: "bob")

// Send an encrypted group message
let messageIds = try protocol.meshSendGroupMessage(
    groupId: group.groupId,
    content: "Hello team!"
)

// Remove a member (admin only)
try protocol.meshRemoveFromGroup(groupId: group.groupId, memberId: "bob")

// Get group info (members, epoch, etc.)
if let info = try protocol.getGroupInfo(groupId: group.groupId) {
    print("Members: \(info.members)")
}

// Rename a group (admin only)
try protocol.renameGroup(groupId: group.groupId, newName: "New Team Name")

// Leave a group
try protocol.meshLeaveGroup(groupId: group.groupId)
```

### Group Role Management

Groups use role-based access control: **Admin** and **Member**.

```swift
// Promote a member to admin (admin only)
try protocol.setMemberRole(groupId: groupId, userId: "bob", role: "admin")

// Check a member's role
let role = try protocol.getMemberRole(groupId: groupId, userId: "bob") // "admin" or "member"

// Get all roles
let roles = try protocol.getGroupRoles(groupId: groupId)
// ["alice": "admin", "bob": "admin", "charlie": "member"]
```

Listen for role changes and renames:

```swift
protocol.onEvent { event in
    switch event {
    case .groupRoleChanged(let evt):
        print("\(evt.userId) is now \(evt.newRole) (changed by \(evt.changedBy))")
    case .groupRenamed(let evt):
        print("Group \(evt.groupId) renamed to \(evt.newName) by \(evt.renamedBy)")
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
let removed = try protocol.resetTofuForPeer(peerId: "bob")
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

