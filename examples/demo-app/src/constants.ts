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

export const PRESENCE_MESSAGE_PREFIX = '__presence__::';

export const PRESENCE_REBROADCAST_INTERVAL_MS = 15 * 1000;

export const MAX_PRESENCE_SENDS_PER_TICK = 3;

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
};
