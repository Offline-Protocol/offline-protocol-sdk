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
| Lab / simulator smoke test without MLS | `OfflineProtocolDevelopmentCleartextConfig.make(appId:profile:)` |

Never ship a build that uses the cleartext config to end users. Inbound plaintext
from peers that run MLS is still rejected as a downgrade attempt even when
`requireEncryption` is off.

## Swift

```swift
let config = OfflineProtocolDevelopmentCleartextConfig.make(
    appId: "my-ios-app",
    profile: "default"
)
let mesh = try OfflineProtocol(config: config)
```

No `initializeMls` call is required when `encryptionEnabled` is `false`.

## Kotlin

```kotlin
val config = OfflineProtocolDevelopmentCleartextConfig.make(
    appId = "my-android-app",
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
helper logs its one-shot remediation only when MLS is not initialized:

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
receive the same remedy through the integrator lifecycle hints channel when
`integratorLifecycleWarnings` is enabled in debug builds. It covers
`sendMessage` and `sendMessageRich`, fires once per engine, and only when
encryption is on and MLS is not initialized. A send made while the engine runs
already gets the "encryption is enabled but MLS is not initialized" hint before
the call, so the remedy appears for the case that hint does not cover: a send
made before `start()`. Group sends are not wrapped: they need an MLS group, so
they never fail this way for want of MLS.

See also [MLS integration](mls-integration.md), [integrator map](integrator-map.md),
and [configuration](configuration.md).
