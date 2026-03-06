/**
 * Constants used throughout the Offline Protocol React Native bindings.
 */

/**
 * Error message shown when the native module is not properly linked.
 */
export const LINKING_ERROR =
  `The package '@offline-protocol/mesh-sdk' doesn't seem to be linked. Make sure: \n\n` +
  '- You have run pod install (iOS)\n' +
  '- You rebuilt the app after installing the package\n' +
  '- You are not using Expo Go\n';

/**
 * Default delay in milliseconds before refreshing runtime state after protocol start.
 * This gives the protocol time to fully initialize before querying state.
 */
export const PROTOCOL_START_DELAY_MS = 100;

/**
 * Maximum number of events to keep in memory for the event history.
 * Older events are automatically removed to prevent unbounded memory growth.
 */
export const MAX_EVENT_HISTORY = 200;

