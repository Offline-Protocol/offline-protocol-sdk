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

With default encryption, the first `EncryptFailed` on a send means MLS is missing
or not ready. Direct UniFFI callers can log a one-shot remediation:

```swift
do {
    try mesh.sendMessage(recipient: peer, content: "hi", priority: .high, replyToMsg: nil)
} catch {
    OfflineProtocolEncryptFailedDiagnostics.logFirstRemedyIfNeeded(
        context: "sendMessage",
        error: error,
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
    ) { Log.w("Mesh", it) }
    throw error
}
```

Hosts built on **`ProtocolEngineHost`** (Flutter, hand-written native hosts)
receive the same text through the integrator lifecycle hints channel when
`integratorLifecycleWarnings` is enabled in debug builds.

See also [MLS integration](mls-integration.md), [integrator map](integrator-map.md),
and [configuration](configuration.md).
