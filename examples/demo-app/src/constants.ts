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

export const PRESENCE_BROADCAST_INTERVAL_MS = 15 * 1000;

export const TYPING_INDICATOR_TIMEOUT_MS = 10 * 1000;

export const NEARBY_THRESHOLD_MS = 30 * 1000;

export const PROTOCOL_CONFIG = {
  appId: 'offline-demo',
  // Multi-transport REPRO config (mirrors fernweh_v2's shape). BLE + a *dead*
  // Internet transport + WiFi Direct, with DORS preferOnline. The WebSocket
  // points at a dead loopback (port 1 — nothing ever listens) so the Internet
  // transport can never open/authenticate → it stays Unavailable → DORS is
  // forced to fall back to BLE. This exercises SDK-level offline BLE MLS
  // convergence under the same multi-transport shape fernweh uses, but WITHOUT
  // any plaintext-relay fallback to mask a broken mesh (requireEncryption:true
  // below means a non-converged session fails the send outright).
  // To restore the original BLE-only demo, replace `transports` with
  // `{ ble: { enabled: true } }` and delete the `dors` block.
  transports: {
    ble: {enabled: true},
    internet: {
      enabled: true,
      serverAddress: 'wss://127.0.0.1:1/ws', // dead loopback → never connects → guaranteed BLE fallback
      autoReconnect: true,
      reconnectDelay: 3000,
    },
    wifiDirect: {enabled: true},
  },
  dors: {
    preferOnline: true, // matches fernweh; harmless here since Internet never becomes Available
    minSuccessRateBeforeEscalation: 0.3,
    minBleSamplesBeforeSuccessRateEscalation: 5,
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
