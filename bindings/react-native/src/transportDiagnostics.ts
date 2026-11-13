/**
 * Development-only integrator hints for transport diagnostics.
 *
 * Maps known native `diagnostic` events to plain-language notes about
 * behaviour that looks like a hang but is expected. Never blocks; mirrors
 * docs/troubleshooting.md.
 *
 * Every message below is matched by exact string, so each is pinned against
 * its native emit sites by the Rust guard
 * `transport_expectation_diagnostics_are_pinned_for_rn_hints`. So are the
 * context keys read here.
 */

import type { DiagnosticEvent } from './types';

declare const __DEV__: boolean | undefined;

/** Last `ProtocolConfig.devTransportDiagnostics` from an `OfflineProtocol` ctor. */
let moduleDevTransportDiagnostics: boolean | undefined;

const TROUBLESHOOTING =
  'https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/troubleshooting.md';
const WIFI_DIRECT_DOC = `${TROUBLESHOOTING}#wifi-direct-is-slow-not-stuck`;
const MULTI_APP_DOC = `${TROUBLESHOOTING}#several-sdk-apps-on-one-phone`;

/** Native diagnostic messages this layer recognizes. */
export const TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_STARTED =
  'Wi-Fi Direct group formation started';
export const TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_RESUMED =
  'Wi-Fi Direct group formation resumed';
/**
 * Android only. Sent at most three times per formation attempt, with
 * `elapsedMs` (time since the attempt began) and `report` (1 to 3) in context.
 */
export const TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS =
  'transport_expectation_wifi_direct_formation_progress';
/** Android sends the peer as `context.address`, iOS as `context.peripheral`. */
export const TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES =
  'BLE peer serves several service instances; choosing one';

/**
 * Keys already warned about. Bounded by construction: one per kind of hint
 * and one per progress report number (1 to 3), never one per peer or attempt,
 * because BLE addresses rotate and attempts recur for the life of the process.
 */
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

/** Test seam: forget which hints were shown. */
export function resetTransportDiagnosticHintsForTests(): void {
  warnedKeys.clear();
}

function warnOnce(key: string, message: string): void {
  if (warnedKeys.has(key)) {
    return;
  }
  warnedKeys.add(key);
  console.warn(message);
}

function progressReportNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isInteger(value) && value >= 1 && value <= 3
    ? value
    : undefined;
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
      '[OfflineProtocol] Wi-Fi Direct group formation is in progress. Once a peer is ' +
        'nearby, a group often takes one to two minutes (BLE may still carry traffic ' +
        'meanwhile). See ' +
        WIFI_DIRECT_DOC
    );
    return;
  }

  if (message === TRANSPORT_DIAGNOSTIC_WIFI_DIRECT_FORMATION_PROGRESS) {
    const elapsedMs =
      typeof context?.elapsedMs === 'number' && Number.isFinite(context.elapsedMs)
        ? context.elapsedMs
        : undefined;
    const elapsed =
      elapsedMs !== undefined ? ` (${Math.round(elapsedMs / 1000)}s elapsed)` : '';
    const report = progressReportNumber(context?.report);
    warnOnce(
      `wifi-direct-formation-progress-${report ?? 'unnumbered'}`,
      `[OfflineProtocol] Wi-Fi Direct group formation with a nearby peer is still ` +
        `running${elapsed}. Measured groups took up to about four minutes, so this is ` +
        'usually not a hang. See ' +
        WIFI_DIRECT_DOC
    );
    return;
  }

  if (message === TRANSPORT_DIAGNOSTIC_BLE_MULTI_APP_INSTANCES) {
    const peer =
      typeof context?.address === 'string'
        ? context.address
        : typeof context?.peripheral === 'string'
          ? context.peripheral
          : undefined;
    warnOnce(
      'ble-multi-app',
      `[OfflineProtocol] ${peer !== undefined ? `Peer ${peer}` : 'A peer'} is running ` +
        'several SDK apps on one phone; the stack is picking the instance for your ' +
        'appId. Give each product its own appId on shared test phones. See ' +
        MULTI_APP_DOC
    );
  }
}
