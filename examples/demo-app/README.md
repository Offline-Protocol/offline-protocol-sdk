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
needs no team. Optionally, copy `ios/DevelopmentTeam.xcconfig.example` to
`ios/DevelopmentTeam.xcconfig` and set your team ID if you use a local xcconfig
workflow (that file is gitignored).

## iOS development (simulator vs physical device)

The app uses **UIScene** lifecycle (required on iOS 26+). In **Debug**:

- **Simulator** loads JavaScript from **Metro** (`npm start` in this directory).
  Use `npx react-native run-ios` as usual.
- **Physical device** loads the **embedded** `main.jsbundle` that Xcode produces
  during the build. Metro is not required to open the app, but **after you change
  any JS/TS you must rebuild and reinstall** (run `npx react-native run-ios
  --device "Your iPhone"` again) to pick up those changes.

Release builds always use the embedded bundle.

## How to test peer-to-peer

BLE discovery needs two real devices — the iOS Simulator and the Android
emulator have no working Bluetooth stack.

1. Install on two physical devices
2. Complete onboarding on both, using a different display name on each
3. Open **People** on both and wait for the other device to appear
4. Send a connection request, accept it on the other device
5. Open **Chats** and send a message — it is MLS-encrypted automatically

For Android, grant location and nearby-device permissions when prompted so BLE
discovery can run.
