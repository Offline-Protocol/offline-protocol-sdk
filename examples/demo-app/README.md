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
