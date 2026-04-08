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
# From the repo root
cd examples/nostr-example
npm install
npx pod-install  # iOS only
npx react-native run-ios    # or run-android
```

## Default Relays

- `wss://relay.damus.io`
- `wss://nos.lol`
- `wss://relay.nostr.band`

These are public relays. For production use, configure your own relay.
