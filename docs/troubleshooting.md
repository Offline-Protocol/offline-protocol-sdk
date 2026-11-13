# Transport troubleshooting

For QA engineers and integrators chasing an **"SDK broken"** report. This page
covers behaviour that looks like a hang or a bad peer but is expected,
especially **Wi‑Fi Direct group formation time** and **several SDK apps on one
phone**.

For host setup mistakes (Expo Go, Podfile), see
[react-native-host-doctor.md](react-native-host-doctor.md). For permissions,
see [mobile-permissions.md](mobile-permissions.md).

---

## Symptom index

| What you see | Likely cause | Read |
|--------------|--------------|------|
| Wi‑Fi Direct enabled, no link for 1–2 minutes | Normal group formation budget | [Wi‑Fi Direct is slow, not stuck](#wifi-direct-is-slow-not-stuck) |
| BLE works; Wi‑Fi Direct "never connects" in 30s | Timeout too short for QA | Same section |
| `WiFiDirect` link in seconds, on phones that share a Wi‑Fi network | The LAN, not a group: the slot carries both | Same section |
| First `neighbor_discovered` on `WiFiDirect` and no `transport_switched` to it follows | Not expected; capture logs and report it | Same section |
| `transport_switched` to `wifiDirect` (lower case) with no `WiFiDirect` neighbor | A DORS send decision, not a link | Same section |
| Two SDK apps on one phone; wrong neighbor | Shared BT service; `appId` filter | [Several SDK apps on one phone](#several-sdk-apps-on-one-phone) |
| Extra neighbors while dogfooding another team's app | Relay + shared discovery | Same section |
| Wi‑Fi Direct group never forms between two phones that each run several SDK apps | Your app holds port 8988 on neither phone, so neither creates a group | [LAN / peer-stream port](#lan--peer-stream-port-android) |
| No `neighbor_discovered` events | Permissions, range, or not started | [No peers](#no-peers-discovered) |

---

## Wi‑Fi Direct is slow, not stuck

### What is happening

On **Android 10+**, with `wifiDirect.autoAccept: true`, the SDK forms the Wi‑Fi
Direct group itself. Formation is **not instantaneous**. In measured runs, **two
phones took about 18 to 225 seconds** (median near **one minute**). Treat **one
to two minutes** as a normal QA window, not a failure.

While the group is forming, **BLE remains available** when BLE is enabled.
Traffic and discovery can continue on Bluetooth.

### What to do in QA

1. Enable both BLE and Wi‑Fi Direct for proximity tests, or wait the full time
   budget before failing the case.
2. Watch events instead of staring at a spinner:
   - `neighbor_discovered` with `transport: "WiFiDirect"`: the slot has a
     link to that peer. This is the signal to wait for.
   - `transport_switched` with `from: null` and `to: "WiFiDirect"` fires
     when the slot gains its first link, and `from: "WiFiDirect"` with
     `to: "None"` when it loses its last. The slot's own layer emits these
     whatever DORS prefers. Only the first link switches: a second peer on a
     slot that already has one raises `neighbor_discovered` alone. The switch
     can trail the first `neighbor_discovered`: a stream that connects before
     the platform reports the layer up switches once the layer is up. A first
     `WiFiDirect` neighbor with no switch to follow is a bug worth reporting.
   - DORS decisions raise their own events: `dors_transport_switched` with a
     machine-readable `reason_code`, and a `transport_switched` whose `from`
     and `to` use the configuration spelling (`"ble"`, `"wifiDirect"`) and
     whose `reason` is "DORS selected better transport", when it picks a
     different transport to send on (see
     [dors-configuration.md](dors-configuration.md)). Match the slot's events
     on the exact `"WiFiDirect"` spelling, or a DORS switch reads as a link.
3. Take both phones off any shared Wi‑Fi network when you time group
   formation. On Android the `wifiDirect` slot also carries peer streams on
   the Wi‑Fi network the phone is on, so two phones on one network link over
   the LAN within seconds and raise the same `WiFiDirect` events without any
   group. That run says nothing about the group's time budget.
4. Grant **NEARBY_WIFI_DEVICES** (Android 13+) or **ACCESS_FINE_LOCATION**
   (Android 12 and lower) **before** `start()` when Wi‑Fi Direct is on. Missing
   grants disable Wi‑Fi Direct for the whole session until you call
   `enableTransport('wifiDirect')` after the grant.
5. Without `autoAccept`, pair once in the system Wi‑Fi Direct settings, then
   retry.

### Configuration reference

```typescript
transports: {
  ble: { enabled: true },
  wifiDirect: { enabled: true, autoAccept: true },
}
```

Full transport notes:
[react-native-integration §4.2](react-native-integration.md#42-wifi-direct-android),
[mobile-permissions.md](mobile-permissions.md#verify-before-release).

### iOS note

iOS has no Wi‑Fi Direct group. On iOS the `wifiDirect` slot carries Network
framework TCP streams on the local network or over AWDL, found by browsing
`_offlineprotocol._tcp`
([ADR 0027](adr/0027-ios-peer-streams-ride-network-framework.md)). There is no
group to form, so the one to two minute budget does not apply. The first
`start()` with the slot enabled may show the Local Network prompt (see
[mobile-permissions.md](mobile-permissions.md)); until it is granted the slot
finds nothing. Use physical devices and the same event-based checks.

---

## Several SDK apps on one phone

### What is happening

Every app built on this SDK advertises the **same Bluetooth service** on a
device. A phone running **two SDK apps** therefore exposes **two logical
instances** of that service.

The stack disambiguates using a short tag derived from each app's **`appId`**
(see [identity-for-integrators.md](identity-for-integrators.md)). Peers with the
**same `appId`** recognize each other as one product. Peers with **different
`appId`s** may still appear in discovery because they can **relay** for you.

### Dogfood and test fleets

| Practice | Why |
|----------|-----|
| Use a **stable `appId` per product** across debug, staging, and release builds of **that** app | Otherwise two builds of "the same" app ignore each other |
| Use **distinct `appId`s** for distinct products on the same phone | Avoid assuming every neighbor is your app |
| **Upgrade all SDK apps** on shared test phones to **0.27.0 or later** | An app built on an earlier SDK serves no `appId` tag and cannot find its own counterpart on a phone running several SDK apps (see [changelog 0.27](changelog/0.27.md)) |
| Identify **your** counterpart by **known `off1…` address** or invite, not by discovery alone | Other SDK apps may show up as neighbors |
| After installing or removing an SDK app, **toggle Bluetooth** on the *other* phone if pairing stalls | iOS may cache stale GATT service lists |

### LAN / peer-stream port (Android)

SDK apps also browse peers on the local Wi‑Fi/LAN. From **0.32.0**, if the
peer-stream port **8988** is already taken by another app on the **same
device**, the second app binds a free port and advertises that one instead.

A Wi‑Fi Direct group's clients can dial only **8988**, so an app that could not
take it **never creates a group; it only joins one**. If two phones each run
your app as the second SDK app, neither creates a group and Wi‑Fi Direct never
forms between them, however long you wait. BLE and the LAN still work. Test
Wi‑Fi Direct with one SDK app per phone, or make sure your app started first
on at least one of them.

On a phone with an SDK app older than 0.32.0, a second app could not bind 8988
and still advertised it, so LAN peers that dialled it reached the first app
instead. Upgrade every SDK app on shared test phones.

Details: [changelog 0.32](changelog/0.32.md),
[UPGRADING §30](UPGRADING.md#30-behaviour-that-changes-without-a-compile-error-v0320).

---

## No peers discovered

1. Both sides called `start()` and listeners were registered **before** `start()`
   on React Native (see [integrator map](integrator-map.md#react-native-lifecycle-ordering)).
2. **Physical devices** in range, Bluetooth on, permissions granted per
   [mobile-permissions.md](mobile-permissions.md).
3. For peers on the same Wi‑Fi network: an Android app that targets API 37
   (Android 17) must declare `ACCESS_LOCAL_NETWORK` and request it before
   `start()`, or the LAN carrier stays off with only a warning `diagnostic`
   event. The SDK does not declare it for you (see
   [mobile-permissions.md](mobile-permissions.md#android-17-api-37-local-network)).
4. Simulators are **not** sufficient for BLE or Wi‑Fi Direct mesh QA.
5. Run `npx mesh-sdk-doctor` for linking and Expo issues.

---

## React Native: development console hints

`devTransportDiagnostics` on `ProtocolConfig` turns a few native `diagnostic`
events into one-time `console.warn` messages that link back to this page. It is
**on** in development builds (`__DEV__`) and **off** in production unless set
to `true`. The hints never block calls. Each is shown at most once per
process, the progress hint once for each report number.

| Native `diagnostic` message | Platform | Hint |
|-----------------------------|----------|------|
| `Wi-Fi Direct group formation started` or `... resumed` | Android | Formation is in progress; once a peer is nearby a group often takes one to two minutes |
| `transport_expectation_wifi_direct_formation_progress` | Android | Formation with a nearby peer is still running, with the elapsed time |
| `BLE peer serves several service instances; choosing one` | Android, iOS | The peer phone runs several SDK apps and the stack is picking yours by `appId` |

The progress diagnostic is reported only while an attempt can end in a group:
a peer of the same `appId` has been heard over Wi‑Fi Direct, Wi‑Fi P2P is on,
the transport is not paused, the device is not yet in this app's group, and no
stream links a peer yet (a `WiFiDirect` link over the LAN counts as one). It is
sent at one, two and four minutes into an attempt, never more, with `elapsedMs`
and `report` (1 to 3) in its context. A lone phone never sends it. Unlike the
console hints, the diagnostic reaches your `diagnostic` listeners in every
build.

---

## Related

| Document | Role |
|----------|------|
| [react-native-integration §4](react-native-integration.md#4-enabling-transports) | Transport behaviour after permissions |
| [dors-configuration.md](dors-configuration.md) | DORS decisions (`dors_transport_switched`, and `transport_switched` spelled `"ble"`, `"wifiDirect"`) once links exist |
| [transport-architecture.md](transport-architecture.md) | How carriers fit together |
| [bindings/react-native/README §Troubleshooting](../bindings/react-native/README.md#troubleshooting) | Delivery, battery, linking |
