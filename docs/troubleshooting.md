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
| Wi‑Fi Direct link is up but no `transport_switched` | DORS still prefers BLE | Same section |
| Two SDK apps on one phone; wrong neighbor | Shared BT service; `appId` filter | [Several SDK apps on one phone](#several-sdk-apps-on-one-phone) |
| Extra neighbors while dogfooding another team's app | Relay + shared discovery | Same section |
| Wi‑Fi Direct group never forms between two phones that each run several SDK apps | Neither app holds port 8988, so neither creates a group | [LAN / peer-stream port](#lan--peer-stream-port-android) |
| `neighbor_discovered` empty | Permissions, range, or not started | [No peers](#no-peers-discovered) |

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
   - `neighbor_discovered` whose `transport` names the Wi‑Fi Direct slot: the
     link is up. This is the signal to wait for.
   - `transport_switched` fires only if DORS then prefers the new link. A
     switch needs a clear score improvement and respects a cooldown (see
     [dors-configuration.md](dors-configuration.md)), so it may never fire
     while BLE is healthy. Its absence is not a failure.
3. Grant **NEARBY_WIFI_DEVICES** (Android 13+) or **ACCESS_FINE_LOCATION**
   (Android 12 and lower) **before** `start()` when Wi‑Fi Direct is on. Missing
   grants disable Wi‑Fi Direct for the whole session until you call
   `enableTransport('wifiDirect')` after the grant.
4. Without `autoAccept`, pair once in the system Wi‑Fi Direct settings, then
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
| **Upgrade all SDK apps** on shared test phones to the **same SDK generation** | Older builds could not disambiguate multi-app phones correctly (see [UPGRADING](UPGRADING.md)) |
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
3. Simulators are **not** sufficient for BLE or Wi‑Fi Direct mesh QA.
4. Run `npx mesh-sdk-doctor` for linking and Expo issues.

---

## Related

| Document | Role |
|----------|------|
| [react-native-integration §4](react-native-integration.md#4-enabling-transports) | Transport behaviour after permissions |
| [dors-configuration.md](dors-configuration.md) | When switches happen after links exist |
| [transport-architecture.md](transport-architecture.md) | How carriers fit together |
| [bindings/react-native/README §Troubleshooting](../bindings/react-native/README.md#troubleshooting) | Delivery, battery, linking |
