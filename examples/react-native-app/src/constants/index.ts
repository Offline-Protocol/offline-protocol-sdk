/**
 * Application-wide constants for the React Native example app.
 */

/**
 * Default WebSocket relay server URL for internet transport.
 */
export const DEFAULT_RELAY_SERVER_URL = 'ws://192.168.1.7:3000/ws';

// Hardcoded token for aditi user
export const HARDCODED_TOKEN =
  'eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJlbWFpbCI6ImFkaXRpcG9sa2FtQGdtYWlsLmNvbSIsImlkIjoxMDcwMiwidXNlcm5hbWUiOiJhZGl0aSIsImRldklkIjpmYWxzZSwiaWF0IjoxNzY4OTk4OTI5LCJleHAiOjE4MDg1OTg5Mjl9.Pe7rrZt5KcpkYiZ_pk0hzIViUDX0CfW_7fZCovPkxi28TuBfmfbBvAP03SBLwfu89sydMsmjDuhXLM80O8gUd8CGkhUuAN0fYmfGTgsYPSUrNzdtC1w-BLtQPRr8IErQHVX8Pl_SqZxTG6ZfxgehqeooOCy3cIAWczpToQvcwJ5S2DFOAXGYQoA5chQfsPC1uauJSe62q5obDlWLK1c3F2k4xPDjwOkRCTtNCq28MPvEta9dgxwFNrTmUyS6pMQG67tL-_1987ciVmZA5IX8kCm7uIrYEwnaEY98db6xbzjM0ffPfbiX9H3V73zUKyM3jq9NVCIATYaR2bhJDiOGlA';

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

