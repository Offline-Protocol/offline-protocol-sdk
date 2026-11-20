# Native lab cleartext configuration

Production mesh traffic must stay **fail-closed**: `requireEncryption` defaults to
`true` and outbound sends fail with `EncryptFailed` until MLS can encrypt. That
surprises early prototyping when storage is not wired yet.

React Native exposes an explicit **lab-only** path:
[`developmentCleartext`](react-native-config-presets.md) in `forScenario`. Native
Swift and Android ship the same posture through
**`OfflineProtocolDevelopmentCleartextConfig`**.

## When to use it

| Goal | Path |
|------|------|
| Production encrypted mesh | Default `ProtocolConfig`, then `OfflineProtocolReferenceStorage.initializeMls`, then `start()` |
| Lab / simulator smoke test without MLS | `OfflineProtocolDevelopmentCleartextConfig.make(appLabel:profile:)` |

Never ship a build that uses the cleartext config to end users. Inbound plaintext
from peers that run MLS is still rejected as a downgrade attempt even when
`requireEncryption` is off.

## Swift

```swift
let config = OfflineProtocolDevelopmentCleartextConfig.make(
    appLabel: "my-ios-app",
    profile: "default"
)
let mesh = try OfflineProtocol(config: config)
```

No `initializeMls` call is required when `encryptionEnabled` is `false`.

## Kotlin

```kotlin
val config = OfflineProtocolDevelopmentCleartextConfig.make(
    appLabel = "my-android-app",
    profile = "default",
)
val protocol = OfflineProtocol(config)
```

## When sends fail with `EncryptFailed`

With default encryption and MLS not initialized, every send fails with
`EncryptFailed`. The same error also comes back with MLS initialized, for a
different reason (an encrypt that failed on a confirmed session, or a
document-layer transfer that cannot leave unsealed), so the error alone does not
say MLS is missing. Direct UniFFI callers pass the engine's own answer, and the
helper logs its one-shot remediation only when MLS is not initialized. Use it
only on the default encrypted config: with the lab config an `EncryptFailed`
comes from a feature that is sealed or not sent at all (a document-layer
transfer, a service subscription), and the remedy would name the wrong fix.

```swift
do {
    try mesh.sendMessage(recipient: peer, content: "hi", priority: .high, replyToMsg: nil)
} catch {
    OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded(
        context: "sendMessage",
        error: error,
        mlsInitialized: mesh.isMlsInitialized(),
        sink: { print($0) }
    )
    throw error
}
```

```kotlin
try {
    protocol.sendMessage(peer, "hi", MessagePriority.HIGH, null)
} catch (error: ProtocolException.EncryptFailed) {
    OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded(
        "sendMessage",
        error,
        protocol.isMlsInitialized(),
    ) { Log.w("Mesh", it) }
    throw error
}
```

Hosts built on **`ProtocolEngineHost`** (Flutter, hand-written native hosts)
need no catch. With `integratorLifecycleWarnings` on (the default in debug
builds), a `sendMessage` or `sendMessageRich` on a running engine with
encryption on and MLS not initialized gets the "encryption is enabled but MLS
is not initialized" hint before the call, once per method, and that hint names
both fixes: `initializeMlsWithSecureStorage()` for production and this config
for lab-only cleartext. A send before `start()` fails with `NotStarted`, never
`EncryptFailed`, so there is nothing further to diagnose there.

See also [MLS integration](mls-integration.md), [integrator map](integrator-map.md),
and [configuration](configuration.md).
