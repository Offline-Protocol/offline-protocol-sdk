/**
 * Application-wide constants for the React Native example app.
 */

import {
  RELAY_SERVER_URL as ENV_RELAY_SERVER_URL,
  RELAY_AUTH_TOKEN as ENV_RELAY_AUTH_TOKEN,
} from '@env';

/**
 * Default WebSocket relay server URL for internet transport.
 * Can be overridden via RELAY_SERVER_URL environment variable.
 */
export const DEFAULT_RELAY_SERVER_URL =
  ENV_RELAY_SERVER_URL || 'ws://localhost:3000/ws';

/**
 * Authentication token for WebSocket relay server.
 * Must be set via RELAY_AUTH_TOKEN environment variable in .env file.
 */
export const HARDCODED_TOKEN = ENV_RELAY_AUTH_TOKEN || '';

if (__DEV__) {
  console.log('[Constants] ENV_RELAY_SERVER_URL:', ENV_RELAY_SERVER_URL);
  console.log(
    '[Constants] ENV_RELAY_AUTH_TOKEN length:',
    ENV_RELAY_AUTH_TOKEN?.length || 0,
  );
  console.log(
    '[Constants] DEFAULT_RELAY_SERVER_URL:',
    DEFAULT_RELAY_SERVER_URL,
  );
  console.log(
    '[Constants] HARDCODED_TOKEN length:',
    HARDCODED_TOKEN?.length || 0,
  );
  console.log('[Constants] HARDCODED_TOKEN:', HARDCODED_TOKEN || 0);
}

/**
 * Presence message prefix used to identify presence/status messages.
 */
export const PRESENCE_MESSAGE_PREFIX = '__presence__::';

/**
 * Key package message prefix used for MLS key exchange.
 */
export const KEY_PACKAGE_MESSAGE_PREFIX = '__keypackage__::';

/**
 * MLS welcome message prefix for session establishment.
 */
export const MLS_WELCOME_MESSAGE_PREFIX = '__mlswelcome__::';

/**
 * Encrypted message prefix indicating MLS-encrypted content.
 */
export const ENCRYPTED_MESSAGE_PREFIX = '__encrypted__::';

/**
 * Interval in milliseconds for rebroadcasting presence messages.
 * This ensures other devices know this device is still online.
 */
export const PRESENCE_REBROADCAST_INTERVAL_MS = 15 * 1000; // 15 seconds

/**
 * Maximum number of presence messages to send per broadcast tick.
 * Caps BLE connection attempts to avoid flooding the transport layer.
 */
export const MAX_PRESENCE_SENDS_PER_TICK = 3;

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

