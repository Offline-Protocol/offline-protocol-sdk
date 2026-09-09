# Privacy disclosures for SDK telemetry

What the SDK's telemetry collects, and what that means for the two app-store
privacy declarations. Everything here applies only once an app calls
`enableTelemetry`; before that the SDK collects nothing and sends nothing.
[docs/telemetry.md](telemetry.md) is the complete data inventory this page
summarises.

The React Native package ships an Apple privacy manifest
(`bindings/react-native/ios/PrivacyInfo.xcprivacy`) in a resource bundle of
the `MeshSdk` pod, so on iOS the SDK's own declaration travels with it.

**You still have work to do**, for two reasons:

1. Apple does not reliably aggregate the manifests that libraries ship in
   their resource bundles, especially under static linking. Fold the entries
   below into your app-level `PrivacyInfo.xcprivacy`, and answer the matching
   questions in App Store Connect under App Privacy: the manifest does not
   fill those in for you.
2. Google Play has no manifest equivalent. Fill in the Data safety form by
   hand; the table below is the starting point.

> What the SDK collects is constrained by design: no personal information, no
> precise location, no advertising identifiers, no message content. The
> device sends typed counts, durations and the engine's own fixed reason
> tokens; the ingest derives a coarse country from the request address and
> then discards the address. The one exception is opt-in and yours to switch
> on: the stable per-install id, below.

## If you set `includeDeviceId`

Everything else on this page describes telemetry as it behaves by default,
where it collects no device identifier. One option changes that, and it is
the only one that changes your store answers:

```typescript
await protocol.enableTelemetry({ apiKey, appId, includeDeviceId: true });
```

It stamps a stable per-install identifier on every batch, so the service
counts distinct devices instead of distinct sessions. The id is opaque: a hash
the SDK derives from its per-install scrub secret, which the ingest hashes
again with a rotating secret before storing. It identifies an install of your
app on one device and nothing more: it is not a hardware id, not an
advertising id, not shared with any other app, and cannot be reversed to the
secret it came from.

It is still a persistent per-install identifier, which is what the two stores
ask about:

| | Without `includeDeviceId` (default) | With `includeDeviceId` |
|---|---|---|
| Google Play, "Device or other IDs" | Not collected. The per-session UUID is random and rotates on every background. | **Collected.** Google's definition names Firebase's installation id as an example, and the same reasoning applies. Purpose Analytics, not linked to identity, not used to track. |
| Apple, `NSPrivacyCollectedDataTypeDeviceID` | Not declared, and the shipped manifest omits it. | Declare it in your app-level manifest and in App Store Connect: not linked to your identity, not used to track you, purpose Analytics. |

The manifest the SDK ships is unchanged either way: it declares what the SDK
collects on its own, and on its own it collects no device id. Because the
option is yours to set, the disclosure is yours to make. The safe default is
to leave it unset and stay at session grain, which costs only precision in
distinct-device counts.

## Apple: `PrivacyInfo.xcprivacy`

The manifest the pod ships declares three collected data types, each not
linked to identity, not used for tracking, purpose analytics:

- `NSPrivacyCollectedDataTypeProductInteraction` (messages sent, delivered,
  relayed; routing decisions; session summaries)
- `NSPrivacyCollectedDataTypePerformanceData` (delivery latency, transport
  dwell, queue depths, handshake latency)
- `NSPrivacyCollectedDataTypeOtherDiagnosticData` (failure reasons,
  decryption failures, transport status changes)

It also declares the required-reason APIs the SDK's iOS code already uses
(`UserDefaults` under CA92.1, file timestamps under C617.1), and declares no
tracking and no tracking domains. The ingest endpoint is not a tracking
domain: nothing sent to it is used for cross-app tracking.

App Store Connect answers that correspond to the above: declare Product
Interaction (Usage Data), Performance Data and Other Diagnostic Data; for
each, not used to track you and not linked to your identity; purpose
Analytics.

**Location.** The manifest declares no location data, because the SDK
accesses no location API. The ingest derives a coarse country code from the
request address and then discards the address. App Store Connect answers are
app-scoped, not SDK-scoped; confirm with counsel whether a server-derived
country counts as "coarse location" for your app.

## Google Play: Data safety

| Data type | Collected? | Shared? | Purpose | Notes |
|---|---|---|---|---|
| App activity, App interactions | Yes | No | Analytics | Message and routing events, session summaries |
| App info and performance, Other app performance data | Yes | No | Analytics | Latencies, dwell, queue depth, handshake timing |
| App info and performance, Diagnostics | Yes | No | Analytics | Failure reasons, decryption failures, transport status |
| Device or other IDs | Only with `includeDeviceId` | No | Analytics | See above |
| Location | No | No | | The ingest derives a country from the address; declare per your counsel's reading |
| Personal info, Messages, Photos, Contacts, Files | No | No | | Never collected by the SDK |

"Shared" is No because the data goes to Offline Protocol's ingest as your
processor, not to a third party; confirm how your own agreements describe
that relationship. Data is encrypted in transit (TLS through the SDK's bundled
roots). Users can request deletion through you, and you can stop collection
at any time with `setTelemetryEnabled(false)` or `disableTelemetry()`; there
is no in-SDK deletion request because the SDK holds nothing to delete beyond
the pending queue, which it drops with the key.

## What is never collected

Message content, attachments, file names, usernames, peer addresses, group
ids, message ids, precise or coarse location from the device, advertising
identifiers, hardware identifiers, contacts. The pipe's event projections
have no field for any of them; a test in the SDK pins the field lists, and
the golden fixture in `crates/offline-protocol/tests/fixtures/` pins the
bytes.
