# Mesh Wiki

React Native app demonstrating **mesh services** — the request/response layer of
the Offline Protocol SDK — by sharing offline reference material between devices
with no internet connection.

Where the other examples focus on messaging, this one shows a device acting as a
*server*: it registers a knowledge pack as a discoverable mesh service, and
nearby peers query it over BLE.

## What it does

- Registers built-in knowledge packs (for example `first-aid.v1`) as mesh
  services via `MeshServices`
- Discovers packs published by nearby peers through service-discovery gossip
- Answers questions by issuing a service request and rendering the response
- Lets each pack be toggled on and off, so you can watch a service appear and
  disappear from the other device's list

All of it lives in `src/App.tsx`, including the knowledge-pack data, so the
service registration and request/response flow can be read top to bottom in one
file.

## Setup

```bash
# 1. Build the SDK's native libraries first (required before the first run —
#    the app cannot load the native module otherwise).
cd bindings/react-native
npm run build:all
cd ../..

# 2. Install JS dependencies
cd examples/mesh-wiki
npm install
npx pod-install  # iOS only

# 3. Run it (this example ships an ios/ project but no android/, so iOS only)
npx react-native run-ios
```

> To run on Android, scaffold an `android/` project for this directory first
> (`npx react-native init` or an Expo prebuild).

## Signing (iOS)

`DEVELOPMENT_TEAM` is intentionally blank in the committed Xcode project. Open
`ios/MeshWiki.xcodeproj`, select the target, and set your own team under
**Signing & Capabilities** before running on a physical device.

## How to test

Service discovery runs over BLE, so it needs two real devices.

1. Install the app on two physical devices
2. Start the protocol on both
3. On device A, enable a knowledge pack — it is now published as a service
4. On device B, wait for that pack to appear under discovered services
5. Ask one of the pack's questions on device B — the request is routed to
   device A over the mesh and the answer comes back over the same path

Turning the pack off on device A should make it disappear from device B.
