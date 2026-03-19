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
  transports: {
    ble: {enabled: true},
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
