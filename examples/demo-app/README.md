# Offline Demo

The reference React Native app for the Offline Protocol SDK. It exercises most
of the public surface — discovery, 1:1 chat, groups, mesh services, and runtime
telemetry — in one place, and is the example to read first.

## What it does

Six tabs, each backed by a screen in `src/screens/`:

- **Onboarding** — picks a display name and user ID, then starts the protocol
- **People** — live peer discovery, presence, and connection requests
- **Chats** — 1:1 messaging with automatic MLS end-to-end encryption
- **Groups** — group creation, invites, and encrypted group messaging
- **Services** — mesh service registry, discovery, and request/response
- **Diagnostics** — transport state, DORS selection, and a live telemetry feed
  (`src/components/TelemetryViz.tsx`)

Protocol wiring lives in `src/context/ProtocolContext.tsx`; that is the file to
copy into a real app.

## Setup

```bash
# 0. One-time prerequisite. The build regenerates the UniFFI bindings alongside
#    the native libraries, so it needs uniffi-bindgen at the crate's pinned
#    version (it refuses to run on a mismatch rather than emit bindings whose
#    checksums fail at the app's first call).
cargo install uniffi --version 0.30.0 --features cli --locked

# 1. Build the SDK's native libraries first (required before the first run —
#    the app cannot load the native module otherwise).
cd bindings/react-native
npm run build:all
cd ../..

# 2. Install JS dependencies
cd examples/demo-app
npm install
npx pod-install  # iOS only

# 3. Run it
npx react-native run-ios
# or
npx react-native run-android
```

This example ships both an `ios/` and an `android/` project.

## Signing (iOS)

`DEVELOPMENT_TEAM` is intentionally blank in the committed Xcode project. Open
`ios/OfflineDemo.xcodeproj`, select the target, and set your own team under
**Signing & Capabilities** before running on a physical device. The Simulator
needs no team.

## How to test peer-to-peer

BLE discovery needs two real devices — the iOS Simulator and the Android
emulator have no working Bluetooth stack.

1. Install on two physical devices
2. Complete onboarding on both, using a different display name on each
3. Open **People** on both and wait for the other device to appear
4. Send a connection request, accept it on the other device
5. Open **Chats** and send a message — it is MLS-encrypted automatically

Grant Bluetooth and (on Android) nearby-devices/location permissions when
prompted, or discovery silently returns nothing.

The address under your name in the header is this device's identity. It is
the same on every launch, so contacts and encrypted sessions survive a restart.

### Wi-Fi Direct (Android)

Wi-Fi Direct is on by default on Android and works with Bluetooth off. On
Android 10 and later the SDK forms the group itself (`autoAccept: true` in
`src/constants.ts`): start the app on both phones and they find each other
within about a minute, with no dialog to accept. On older phones, pair them
once in the system settings (**Wi-Fi > Wi-Fi Direct**, or **Wi-Fi > Advanced >
Wi-Fi Direct** on some phones). Allow **Nearby devices** (Android 13+) or
**Location** (Android 12 and lower) when asked, or the transport stays off.

If a connection request stays pending, check the date and time on both phones.
Phones that have not been online often have the wrong date, and two devices
whose clocks are far enough apart cannot set up encryption. The app shows a
"Check date and time" alert when it sees this.
