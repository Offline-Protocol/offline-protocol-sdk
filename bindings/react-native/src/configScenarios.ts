/**
 * Opinionated {@link ProtocolConfig} presets for common integration paths.
 *
 * Presets only set fields that differ by scenario. Unset fields stay absent
 * so the native bridge and Rust core keep owning their defaults (see js-ci-harness
 * relay-config and data-config tests).
 */

import type { ProtocolConfig } from './types';

/** Named integration scenarios covered by docs and tests. */
export type ConfigScenario =
  | 'localBle'
  | 'proximityMesh'
  | 'internetRelay'
  | 'nostrMesh'
  /**
   * Cleartext-only local testing. Never ship to production users; encryption
   * defaults remain fail-closed when you omit this scenario.
   */
  | 'developmentCleartext';

export const CONFIG_SCENARIO_IDS: readonly ConfigScenario[] = [
  'localBle',
  'proximityMesh',
  'internetRelay',
  'nostrMesh',
  'developmentCleartext',
] as const;

export interface ForScenarioParams {
  appId: string;
  profile: string;
  /** Applied last; use for relay URL, internet serverAddress, and app-specific tuning. */
  overrides?: Partial<ProtocolConfig>;
}

type PlainRecord = Record<string, unknown>;

function isPlainRecord(value: unknown): value is PlainRecord {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Shallow merge for top-level keys; one extra level for nested objects (e.g. `transports`). */
function mergePartial(
  base: Partial<ProtocolConfig>,
  layer: Partial<ProtocolConfig>
): Partial<ProtocolConfig> {
  const out: Partial<ProtocolConfig> = { ...base };
  for (const key of Object.keys(layer) as (keyof ProtocolConfig)[]) {
    if (key === 'appId' || key === 'profile') {
      continue;
    }
    const value = layer[key];
    if (value === undefined) {
      continue;
    }
    const existing = out[key];
    if (isPlainRecord(value)) {
      const merged: PlainRecord = isPlainRecord(existing) ? { ...existing } : {};
      for (const [nestedKey, nestedValue] of Object.entries(value)) {
        if (nestedValue === undefined) {
          continue;
        }
        const nestedExisting = merged[nestedKey];
        if (isPlainRecord(nestedValue) && isPlainRecord(nestedExisting)) {
          merged[nestedKey] = {
            ...(nestedExisting as PlainRecord),
            ...(nestedValue as PlainRecord),
          };
        } else {
          merged[nestedKey] = nestedValue;
        }
      }
      (out as PlainRecord)[key as string] = merged;
    } else {
      (out as PlainRecord)[key as string] = value;
    }
  }
  return out;
}

/**
 * Returns the preset slice for a scenario (no `appId` / `profile`).
 */
export function scenarioPreset(scenario: ConfigScenario): Partial<ProtocolConfig> {
  switch (scenario) {
    case 'localBle':
      return {
        transports: {
          ble: { enabled: true },
          wifiDirect: { enabled: false },
          internet: { enabled: false },
          nostr: { enabled: false },
          reticulum: { enabled: false },
        },
      };
    case 'proximityMesh':
      return {
        transports: {
          ble: { enabled: true },
          wifiDirect: { enabled: true },
          internet: { enabled: false },
          nostr: { enabled: false },
        },
      };
    case 'internetRelay':
      return {
        transports: {
          ble: { enabled: true },
          internet: { enabled: true },
        },
        dors: {
          preferOnline: true,
        },
      };
    case 'nostrMesh':
      return {
        transports: {
          ble: { enabled: true },
          nostr: { enabled: true },
        },
      };
    case 'developmentCleartext':
      return {
        transports: {
          ble: { enabled: true },
        },
        encryption: {
          enabled: false,
        },
      };
    default: {
      const _exhaustive: never = scenario;
      return _exhaustive;
    }
  }
}

/**
 * Builds a full {@link ProtocolConfig} for `appId`, `profile`, the scenario
 * preset, and optional `overrides` (merged last).
 */
export function forScenario(
  scenario: ConfigScenario,
  params: ForScenarioParams
): ProtocolConfig {
  const merged = mergePartial(
    scenarioPreset(scenario),
    params.overrides ?? {}
  );
  return {
    appId: params.appId,
    profile: params.profile,
    ...merged,
  };
}
