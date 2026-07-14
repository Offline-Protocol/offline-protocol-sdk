# Nostr Transport Example

Minimal React Native app demonstrating the Nostr relay transport for the Offline Protocol SDK.

## What it does

- Connects to public Nostr relays (`relay.damus.io`, `nos.lol`, `relay.nostr.band`)
- Sends and receives messages between two devices via Nostr relays
- Shows connection status and transport logs
- Allows toggling the Nostr transport on/off

## How to test

1. Install the app on two devices (or one device + one simulator)
2. Tap **Start** on both devices
3. Note each device's User ID shown in the header
4. Enter the other device's User ID in the "Peer's User ID" field
5. Send a message - it will be routed through the Nostr relays

## Setup

```bash
# 1. Build the SDK's native libraries first (required before the first run —
#    the app cannot load the native module otherwise).
cd bindings/react-native
npm run build:all
cd ../..

# 2. Install JS dependencies
cd examples/nostr-example
npm install
npx pod-install  # iOS only

# 3. Run it (this example ships an ios/ project but no android/, so iOS only)
npx react-native run-ios
```

> To run on Android, scaffold an `android/` project for this directory first
> (`npx react-native init` or an Expo prebuild).

## Default Relays

- `wss://relay.damus.io`
- `wss://nos.lol`
- `wss://relay.nostr.band`

These are public relays. For production use, configure your own relay.
