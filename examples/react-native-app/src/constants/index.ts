/**
 * Application-wide constants for the React Native example app.
 */

/**
 * Default WebSocket relay server URL for internet transport.
 */
export const DEFAULT_RELAY_SERVER_URL = 'wss://relay-server-production-31c7.up.railway.app/ws';

/**
 * Presence message prefix used to identify presence/status messages.
 */
export const PRESENCE_MESSAGE_PREFIX = '__presence__::';

/**
 * Interval in milliseconds for rebroadcasting presence messages.
 * This ensures other devices know this device is still online.
 */
export const PRESENCE_REBROADCAST_INTERVAL_MS = 60 * 1000; // 1 minute

/**
 * Time in milliseconds to retain processed messages before cleanup.
 * This prevents memory growth from message history.
 */
export const PROCESSED_MESSAGE_RETENTION_MS = 10 * 60 * 1000; // 10 minutes

/**
 * Step configuration for multi-step forms or wizards.
 */
export const STEP_CONFIG = {
  min: 0,
  max: 100,
  step: 1,
} as const;

