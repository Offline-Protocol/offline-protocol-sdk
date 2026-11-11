/**
 * Development-only integrator hints for transport diagnostics (G6 PR2).
 *
 * Maps known native `diagnostic` events to plain-language progress notes. Never
 * blocks; mirrors docs/troubleshooting.md.
 */

import type { DiagnosticEvent } from './types';

declare const __DEV__: boolean | undefined;

/** Last `ProtocolConfig.devTransportDiagnostics` from an `OfflineProtocol` ctor. */
let moduleDevTransportDiagnostics: boolean | undefined;

const TRANSPORT_DOC = 'docs/troubleshooting.md';

/** Native diagnostic messages this layer recognizes (keep in sync with bridges). */
export const TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED =
  'Wi-Fi Direct group formation started';
export const TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_RESUMED =
  'Wi-Fi Direct group formation resumed';
export const TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS =
  'transport_expectation_wifi_direct_formation_progress';
export const TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES =
  'BLE peer serves several service instances; choosing one';

const warnedKeys = new Set<string>();

export function bindDevTransportDiagnosticsFromConfig(flag: boolean | undefined): void {
  moduleDevTransportDiagnostics = flag;
}

export function devTransportDiagnosticsEnabled(flag: boolean | undefined): boolean {
  if (flag === false) {
    return false;
  }
  if (flag === true) {
    return true;
  }
  return typeof __DEV__ !== 'undefined' && __DEV__ === true;
}

export function currentDevTransportDiagnostics(): boolean {
  return devTransportDiagnosticsEnabled(moduleDevTransportDiagnostics);
}

function warnOnce(key: string, message: string): void {
  if (warnedKeys.has(key)) {
    return;
  }
  warnedKeys.add(key);
  console.warn(message);
}

/**
 * If `event` is a known transport-expectation diagnostic, log a one-time hint.
 */
export function maybeExplainTransportDiagnostic(event: DiagnosticEvent): void {
  if (!currentDevTransportDiagnostics()) {
    return;
  }

  const { message, context } = event;

  if (
    message === TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED ||
    message === TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_RESUMED
  ) {
    warnOnce(
      'wifi-direct-formation-started',
      '[OfflineProtocol] Wi‑Fi Direct group formation is in progress. This often takes ' +
        '1–2 minutes (BLE may still carry traffic meanwhile). See ' +
        TRANSPORT_DOC
    );
    return;
  }

  if (message === TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS) {
    const elapsedMs =
      typeof context?.elapsedMs === 'number' ? context.elapsedMs : undefined;
    const elapsed =
      elapsedMs !== undefined ? ` (${Math.round(elapsedMs / 1000)}s elapsed)` : '';
    warnOnce(
      `wifi-direct-formation-progress-${Math.floor((elapsedMs ?? 0) / 60_000)}`,
      `[OfflineProtocol] Wi‑Fi Direct group formation still running${elapsed}. ` +
        'This is usually not a hang. See ' +
        TRANSPORT_DOC
    );
    return;
  }

  if (message === TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES) {
    const address =
      typeof context?.address === 'string' ? context.address : 'peer';
    warnOnce(
      `ble-multi-app-${address}`,
      `[OfflineProtocol] Peer ${address} is running several SDK apps on one phone; ` +
        'the stack is picking the instance for your appId. Use distinct appIds per ' +
        'product in dogfood. See ' +
        TRANSPORT_DOC
    );
  }
}
