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
    // Not optional for Nostr, and not merely for encryption's sake.
    //
    // Disabling it skips MLS initialization, which is what derives this
    // device's address — and the Nostr routing tag is derived from that
    // address. With no identity there is no tag, so the SDK installs no Nostr
    // transport and `enableTransport('nostr')` is refused.
    //
    // This used to "work": the transport fell back to the profile, so the
    // subscription filter published a label anyone could recompute from the
    // username to three public relays. It doesn't do that any more.
    enabled: true,
  },
  network: {initialTtl: 8},
};

export const MAX_LOG_ENTRIES = 100;

export const STALE_NEIGHBOR_MS = 60 * 1000;

export const STALE_NEIGHBOR_CLEANUP_INTERVAL_MS = 30 * 1000;
