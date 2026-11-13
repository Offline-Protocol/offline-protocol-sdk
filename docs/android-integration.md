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
cargo build --profile mobile --target aarch64-linux-android
cargo build --profile mobile --target armv7-linux-androideabi
cargo build --profile mobile --target x86_64-linux-android
```

### 3. Copy Native Libraries

Each build produces `liboffline_protocol_uniffi.so`. UniFFI's Kotlin loader looks for
`libuniffi_offline_protocol.so`, so copy each ABI's output under **that** name.
The shipped library exposes **`OfflineProtocolJniLibrary.LOADED_BASENAME`** and
**`CARGO_OUTPUT_BASENAME`** so the two names stay obvious in the IDE. Full tables
and import rules: [android-jni-and-imports.md](android-jni-and-imports.md).

```
android/app/src/main/jniLibs/
├── arm64-v8a/libuniffi_offline_protocol.so
├── armeabi-v7a/libuniffi_offline_protocol.so
└── x86_64/libuniffi_offline_protocol.so
```

The `bindings/react-native/scripts/build-uniffi-android.sh` helper builds every ABI and
renames automatically. It also regenerates the UniFFI bindings, all five languages, not
just Kotlin: they are one artifact set off one UDL, so the Swift, Python, Ruby and Java
bindings are rewritten too and all five must be committed together (see
`scripts/generate-bindings.sh`).

### 4. Use in Kotlin

The generated Kotlin bindings live in the `uniffi.offline_protocol` package. The
library's hand-written classes, such as `OfflineProtocolReferenceStorage` and the
transport managers, live in `com.offlineprotocol`; import those by name, since
the React Native module in that package is not part of the Android library. Do
not use `OfflineProtocolModule` on the native path (React Native only). See
[android-jni-and-imports.md](android-jni-and-imports.md) for a package table and
the canonical consumer-check imports.

```kotlin
import android.util.Log
import com.offlineprotocol.OfflineProtocolReferenceStorage
import uniffi.offline_protocol.*
import org.json.JSONObject

class MainActivity : AppCompatActivity() {
    private lateinit var protocol: OfflineProtocol

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // ProtocolConfig has 16 required fields (+ 19 with defaults). See
        // docs/configuration.md for what each one controls.
        val config = ProtocolConfig(
            appId = "my-android-app",
            profile = "user123",
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
            pendingTtlMs = 86_400_000.toULong(),  // 24 h (the SDK default)
            overflowPolicy = OverflowPolicy.DROP_OLDEST,
            // Among those left at their defaults: requireEncryption (true),
            // maxGroupMembers (256u), groupRelayEnabled (true),
            // groupRelayBroadcastEnabled (true — capability-gated, and it
            // falls back to per-member fan-out against any relay that did not
            // advertise group_delivery_v3; see
            // docs/configuration.md#group-configuration),
            // requireTransportIdentity (false), binaryWireEnabled (true),
            // compactEnvelopeEnabled (true), richPayloadEnabled (true),
            // cryptoRecoveryEnabled (true), encryptServiceMessages (true).
        )

        protocol = OfflineProtocol(config)

        // Events are delivered as JSON strings via the EventCallback interface.
        // Install the callback BEFORE start(): restore settlements from the
        // previous run are parked and drained on start(), so anything emitted
        // before the callback exists is lost.
        protocol.setEventCallback(MeshEventHandler())

        // REQUIRED before you can send anything. Encryption is fail-closed by
        // default, so with MLS uninitialized every send fails with
        // EncryptFailed. Use OfflineProtocolEncryptFailedDiagnostics for a
        // one-shot remediation log, or OfflineProtocolDevelopmentCleartextConfig
        // for lab-only cleartext (docs/native-development-cleartext.md).
        // Unlike React Native nothing opens storage for you on
        // the native path: this opens the SDK's built-in stores for the account.
        // A warning means this account started from a fresh MLS identity
        // although an earlier one was on the device; report it.
        val adoption = OfflineProtocolReferenceStorage.initializeMls(
            protocol,
            this,
            config.appId,
            config.profile,
        )
        adoption.warning?.let { Log.e("Mesh", it) }

        protocol.start()

        // Keep this order: event callback, MLS, start(), then send. Hosts
        // built on ProtocolEngineHost (the Flutter plugin) get development
        // hints when it is broken; see docs/integrator-map.md. A direct
        // UniFFI caller like this one gets none.

        val peerAddress =
            "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn" // from neighbor_discovered

        // Send a message (priority is required; replyToMsg is optional)
        val messageId = protocol.sendMessage(
            recipient = peerAddress,
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

> **`message_received` is the only copy.** The core persists outbound and session
> state; it never stores inbound content. Your callback either durably records
> the message or it is gone. This is why `start()` must never run ahead of a
> callback that can actually keep what it is handed — see below.

#### Never start the protocol without a live consumer

If you run the protocol from a `Service` — a foreground keep-alive, a
`START_STICKY` restart, a boot receiver — resist the obvious shape of
"reconstruct the config and call `start()`". Starting without a consumer that
durably stores `message_received` does not degrade delivery, it destroys
messages **and tells their senders they arrived**:

1. The receive path sends the delivery ACK *before* it emits the event.
2. That ACK makes the sender drop its outbox entry, retiring the retry ladder.
3. Your callback drops the event (or there is no callback yet), so nothing keeps it.
4. The MLS ratchet generation is spent, so a resend cannot reconstruct it either.

Not starting is strictly better: the sender's outbox holds for up to seven days,
retries, parks, and pushes, and delivers once the device is genuinely running
again. Order it so the consumer exists first — `setEventCallback` before
`start()`, as above — and if a restart path cannot guarantee one, let the mesh
stay down until the app is running. The React Native bindings make the same
call in `MeshForegroundService.handleStickyRestart`, which stops the keep-alive
rather than rebuild a protocol JavaScript is not there to receive from.

Note what the sound version of "bring it back" looks like, since you own the
equivalent path here: those bindings can optionally start *JavaScript* on a
sticky restart and let the app run its own `start()`, so the consumer exists
before the protocol does. The native analogue is the same shape — restore your
durable `EventCallback` first, then `start()` — and the ordering, not the
service, is what makes it safe.

### 5. Storage: Two Providers, Two Lifecycles

`initializeMls` takes two providers because key material and restartable
delivery state have different lifetimes. The shipped
**`OfflineProtocolReferenceStorage`** factory opens the same Keystore-backed MLS
store and app-container state store the React Native module uses, for the
account `(appId, profile)` names. You may substitute your own providers; they
must meet the obligations below.

To sign a user out for good, stop the engine, release it, then call
`OfflineProtocolReferenceStorage.wipe(context, appId, profile)`. It erases that
account's identity, sessions, outbox and protocol state, and nothing of any other
account. A running engine keeps writing, and releasing an engine flushes its
state, so an engine released after the wipe writes that state back. Call
`protocol.close()` first.

| | `MlsStorageProvider` | `ProtocolStateStorageProvider` |
|---|---|---|
| Holds | MLS identity, sessions, groups, peer trust records, install secrets, the record-sealing key | Outbox, pending messages, session/Welcome lifecycles, peer snapshots, media descriptors, block list, Lamport clock |
| Back it with | `EncryptedSharedPreferences` (Keystore-backed) | `noBackupFilesDir` — **must** be removed when the app is uninstalled |
| Value type | `List<UByte>` (`sequence<u8>`) | `ByteArray` (`bytes`) |

A credential store can outlive an app container, which is why delivery state must
not live in one: uninstalling would otherwise leave queued message plaintext and
cloud-media `encryption_key`/`iv` values in the Keystore with nothing that ever
reads or deletes them. Sensitive state-record *values* are sealed with a
per-install AEAD key held in the secure provider, so the state provider only ever
sees ciphertext.

The React Native module's `ProtocolStateStorage.kt` and `StorageNamespace.kt` are
working reference implementations — atomic durable writes, digest-based
filenames, a process-wide lock, per-account namespacing, and stale-temporary
sweeping. Read the
[custom-provider contract](UPGRADING.md#15-the-custom-provider-contract)
before writing your own; every obligation there exists because something breaks
on a device without it.

`initializeMls` is transactional — a failed call rolls back and leaves no partial
state, so surface the error and retry rather than proceeding. Do not treat a
failure as "start clean": a `blocked_users` listing failure deliberately fails
initialization rather than coming up with every peer unblocked.

See [MLS Integration](mls-integration.md#custom-storage-advanced) for the full
provider interfaces.

#### Verify a custom protocol-state backend (C11)

Green on the conformance suite is the definition of a **supported** backend.
`StorageConformance.run` runs it against your `ProtocolStateStorageProvider`
and decodes the report; no engine is needed, but the Rust library must be
loaded, so it runs on a device or emulator rather than in a JVM unit test. The
suite blocks on writes of several megabytes, so call it off the main thread,
and on a scratch instance of a new backend until it is green:

```kotlin
val report = StorageConformance.run(myScratchBackend)
check(report.isGreen) { report.failures }
```

Every check runs against one open instance, so green is not a persistence test:
a backend that forgets everything on reopen can pass, and proves reopen with a
test of its own. Only the protocol-state suite crosses the FFI; the MLS storage
suite runs in Rust only. Details:
[C11](bridges/README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified).

## Permissions

Complete Android rules (SDK merge vs app manifest, runtime order, Wi‑Fi Direct,
API 37 `ACCESS_LOCAL_NETWORK`, foreground service):
[mobile-permissions.md](mobile-permissions.md). The snippet below is a common
app manifest starting point for native integration.

**Identity:** `recipient` and group member ids are peer `off1…` addresses, not
`profile`. See [identity-for-integrators.md](identity-for-integrators.md).

Add to `AndroidManifest.xml`:

```xml
<!-- Bluetooth permissions -->
<uses-permission android:name="android.permission.BLUETOOTH" android:maxSdkVersion="30" />
<uses-permission android:name="android.permission.BLUETOOTH_ADMIN" android:maxSdkVersion="30" />
<uses-permission android:name="android.permission.BLUETOOTH_SCAN"
    android:usesPermissionFlags="neverForLocation" />
<uses-permission android:name="android.permission.BLUETOOTH_ADVERTISE" />
<uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />

<!-- Location: BLE scanning on API 30 and lower, Wi-Fi Direct discovery on
     API 32 and lower. Not needed for BLE from Android 12 (API 31). -->
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION"
    android:maxSdkVersion="32" />

<!-- Wi-Fi Direct permissions -->
<uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
<uses-permission android:name="android.permission.CHANGE_WIFI_STATE" />
<uses-permission android:name="android.permission.NEARBY_WIFI_DEVICES"
    android:usesPermissionFlags="neverForLocation" />
<!-- The same slot on the Wi-Fi network (LAN peers) -->
<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
<uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />

<!-- Internet -->
<uses-permission android:name="android.permission.INTERNET" />
```

## Group Messaging (MLS-Encrypted Mesh)

Create and manage encrypted groups over the mesh. The group creator is automatically an admin.

```kotlin
// Create a group
val group = protocol.createGroup("Project Team")

val inviteeAddress = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
protocol.inviteToGroup(group.groupId, inviteeAddress)

// Send an encrypted group message
val messageIds = protocol.sendGroupMessage(
    groupId = group.groupId,
    content = "Hello team!",
    priority = null,
    replyToMsg = null,
)

// Remove a member (admin only)
protocol.removeFromGroup(group.groupId, inviteeAddress)

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
val inviteeAddress = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn" // a member's address
protocol.setMemberRole(groupId, inviteeAddress, "admin")

val role = protocol.getMemberRole(groupId, inviteeAddress)

val roles = protocol.getGroupRoles(groupId)
// map keys are member off1… addresses
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
- Only admins can *call* invite, remove, change-role, or rename — these are checked before sending
- Role changes and renames are additionally enforced on receive: a non-admin's frame is rejected by every peer
- Membership changes (invite/remove) are **not** enforced on receive. MLS authenticates the committer as a group member, but a member running a modified client can add or remove anyone; the change applies and is reported via `group_unauthorized_membership_change`. See [Group authorization model](./mls-integration.md#group-authorization-model)
- The last admin cannot be demoted, removed, or leave (prevents orphaned groups)
- If the last admin disconnects unexpectedly, a deterministic election promotes the next admin

## Replicated Documents

Shared state any member of a space can edit while disconnected, merging
deterministically when the replicas meet again. The model, how concurrent edits
resolve, and the limits are in the [Replicated Documents guide](data.md); this
section is the Kotlin shape of it.

`DataStore` wraps a live protocol instance. It needs `initializeMls` to have
run, because documents are sealed at rest with the record key that call mints.
`data.enabled` defaults to `true`, so nothing switches the layer on.

```kotlin
val store = DataStore(protocol)

// A space is an MLS scope: a peer's address for a 1:1 space, a group id for a
// group. Values cross as JSON: {"kind":"text","value":"..."}.
val space = peerAddress

store.createDoc(space, "trip")
store.mapSet(space, "trip", "meta", "title", """{"kind":"text","value":"Coast road"}""")
store.textInsert(space, "trip", "notes", 0u, "Meet at the bridge")
store.counterIncrement(space, "trip", "opened", 1.0)

// Edits batch. This is what makes them durable and sends them to peers.
store.flush(space, "trip")

val json = store.docJson(space, "trip")
```

Every call above blocks while it reaches storage, so keep them off the main
looper like every other protocol call
([K2](bridges/kotlin.md#k2-nothing-blocking-on-the-main-looper)).

To put documents in a store of your own rather than the one protocol state
already uses, construct it with a provider instead. Sealing sits above that
seam, so the adapter is handed sealed bytes and never sees document content,
and an app that does this owes `wipeAll()` on logout because
`wipePersistedState` cannot reach a backend it does not know about:

```kotlin
val store = DataStore.withStorage(protocol, myBackend)
```

A reference implementation and the conformance suite that gates one are in
[`examples/storage-adapters/kotlin/`](../examples/storage-adapters/README.md).

Six events arrive through the same `EventCallback` as everything else. Handle
`data_changed` to re-render (it fires after the change is durable), and
`data_attachment_requested` if your app writes attachment references: the SDK
never kept the blob bytes, so only your app can answer, and a request nobody
answers leaves the asking side showing a spinner forever.

```kotlin
"data_changed" ->
    reload(obj.optString("space_id"), obj.optString("doc_id"))
"data_attachment_requested" ->
    // Answer with provideAttachment(...), or declineAttachment(...) if the
    // bytes are gone. Both are real answers; silence is not.
    respondToAttachment(obj)
```

## Peer Identity

There is no trust pin to manage. A peer's address **is** the hash of their
identity key, so every control message they send is checked by re-deriving the
address from the key that signed it — on first contact as much as on the
thousandth. An impersonator has to find a 160-bit second preimage (~2^160), not
win a race to a name. (That is the cost of targeting an address that already
exists. The birthday bound on the same truncation is ~2^80, which yields two
keys sharing one address rather than a chosen peer's — a deliberate trade
against BLE frame budget, documented on `Address::HASH_LEN`.)

That also removes the reinstall problem the old `resetTofuForPeer` existed for:
a peer who reinstalls generates a new identity key and therefore has a **new
address**. They reach you as a new contact, not as the old one behaving oddly,
and there is nothing to reset. If you see
`SENDER_ADDRESS_MISMATCH`, treat it as an impersonation attempt — it has no
benign reading.

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
