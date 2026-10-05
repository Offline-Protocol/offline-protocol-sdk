export const QUICK_MESSAGES = [
  {text: 'Hey there!', emoji: '👋', priority: 'medium' as const},
  {text: 'On my way', emoji: '🚶', priority: 'medium' as const},
  {text: 'Got it, thanks', emoji: '👍', priority: 'medium' as const},
  {text: 'Where are you?', emoji: '📍', priority: 'medium' as const},
  {text: 'Be right back', emoji: '⏳', priority: 'medium' as const},
  {text: 'Yes', emoji: '✅', priority: 'medium' as const},
  {text: 'No', emoji: '❌', priority: 'medium' as const},
  {text: 'Call me later', emoji: '📞', priority: 'medium' as const},
  {text: 'See you soon', emoji: '👀', priority: 'medium' as const},
  {text: 'Emergency - need help!', emoji: '🚨', priority: 'critical' as const},
];

import {Platform} from 'react-native';

// One identity per install. The SDK keeps a profile's keys in the platform
// keystore, so the same profile is the same address on every launch. The demo
// used to mint a new profile at each start, which made every launch a new
// person to its peers (contacts and sessions gone) and left another keystore
// namespace behind each time.
export const PROFILE = 'offline-demo';

export const PRESENCE_BROADCAST_INTERVAL_MS = 15 * 1000;

export const TYPING_INDICATOR_TIMEOUT_MS = 10 * 1000;

export const NEARBY_THRESHOLD_MS = 30 * 1000;

export const PROTOCOL_CONFIG = {
  appId: 'offline-demo',
  // Offline BLE mesh demo: nearby devices discover each other over Bluetooth LE
  // and converge an MLS session directly on the mesh — no internet or relay
  // involved. `requireEncryption: true` keeps the demo fail-closed, so a message
  // only sends once the end-to-end encrypted session has been established.
  transports: {
    ble: {enabled: true},
    // Android only. `autoAccept` lets the SDK form the Wi-Fi Direct group
    // itself (Android 10+); below that, pair the phones in the system's
    // Wi-Fi Direct settings (see README).
    wifiDirect: {enabled: Platform.OS === 'android', autoAccept: true},
  },
  encryption: {
    enabled: true,
    autoKeyExchange: true,
    storePending: true,
    requireEncryption: true,
  },
  network: {initialTtl: 8},
  reliability: {
    ack: {defaultTimeoutMs: 10000},
    retry: {maxRetries: 10},
  },
};

// Telemetry. Paste the key and app id the developer portal issued; with the
// placeholders left in place the demo runs with telemetry off and says so
// once in the console. See docs/telemetry.md for what leaves the device.
export const TELEMETRY_API_KEY = 'PASTE_API_KEY_HERE';
export const TELEMETRY_APP_ID = 'PASTE_APP_ID_HERE';
export const APP_VERSION = '0.0.1';
