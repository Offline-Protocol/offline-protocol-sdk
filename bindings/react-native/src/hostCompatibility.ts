/**
 * An extra line for the "not linked" error when the app is running in Expo Go,
 * the one host that can never load this SDK.
 */

export const HOST_DOCTOR_URL =
  'https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/react-native-host-doctor.md';

const EXPO_GO_HINT =
  '\nDetected Expo Go: it cannot load native modules such as this SDK. Use a development ' +
  `build (expo-dev-client) or npx expo run:ios / run:android. See ${HOST_DOCTOR_URL}\n`;

/**
 * Whether expo-constants says this is Expo Go. Newer Expo SDKs report it as
 * `executionEnvironment === 'storeClient'` and deprecate `appOwnership`, so
 * both are read. Returns false when expo-constants is absent (a bare app).
 */
export function isRunningInExpoGo(): boolean {
  try {
    // Optional: only present in Expo apps. Metro bundles a guarded require of
    // a missing module as a runtime throw (allowOptionalDependencies).
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const mod = require('expo-constants');
    const Constants = mod?.default ?? mod;
    return Constants?.executionEnvironment === 'storeClient' || Constants?.appOwnership === 'expo';
  } catch {
    return false;
  }
}

/**
 * The full message thrown when OfflineProtocolModule is not linked.
 */
export function formatNativeLinkingError(baseMessage: string): string {
  return isRunningInExpoGo() ? `${baseMessage}${EXPO_GO_HINT}` : baseMessage;
}
