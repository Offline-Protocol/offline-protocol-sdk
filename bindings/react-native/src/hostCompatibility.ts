/**
 * Optional hints when the native module is missing (Expo Go, stale Podfile, etc.).
 */

const EXPO_GO_HINT =
  '\nDetected Expo Go: this host cannot load native modules. Use a development build ' +
  '(expo-dev-client) or bare React Native. See docs/react-native-host-doctor.md.\n';

/**
 * When expo-constants is installed, returns a extra hint for Expo Go only.
 */
export function expoGoLinkingHint(): string {
  try {
    // Optional peer: only present in Expo apps.
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const mod = require('expo-constants');
    const Constants = mod?.default ?? mod;
    if (Constants?.appOwnership === 'expo') {
      return EXPO_GO_HINT;
    }
  } catch {
    /* not an Expo app */
  }
  return '';
}

/**
 * Full message when OfflineProtocolModule is not linked.
 */
export function formatNativeLinkingError(baseMessage: string): string {
  const hint = expoGoLinkingHint();
  return hint ? `${baseMessage}${hint}` : baseMessage;
}
