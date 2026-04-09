export const DEFAULT_RELAYS = [
  'wss://relay.damus.io',
  'wss://nos.lol',
  'wss://relay.nostr.band',
];

export const PROTOCOL_CONFIG = {
  appId: 'nostr-example',
  transports: {
    ble: {enabled: false},
    nostr: {
      enabled: true,
      relayUrls: DEFAULT_RELAYS,
      autoReconnect: true,
      maxReconnectAttempts: 0, // Infinite retries
    },
  },
  encryption: {
    enabled: false, // Disabled for simple transport testing
  },
  network: {initialTtl: 8},
};

export const MAX_LOG_ENTRIES = 100;

export const STALE_NEIGHBOR_MS = 60 * 1000;

export const STALE_NEIGHBOR_CLEANUP_INTERVAL_MS = 30 * 1000;
