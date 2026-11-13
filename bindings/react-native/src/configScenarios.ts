/**
 * Opinionated {@link ProtocolConfig} presets for common integration paths.
 *
 * Presets only set fields that differ by scenario. Unset fields stay absent
 * so the native bridge and Rust core keep owning their defaults (see js-ci-harness
 * relay-config and data-config tests).
 *
 * `forScenario` refuses a config that would start without the transport it
 * names. `start()` brings internet up only with a `serverAddress` and Nostr
 * only with a relay URL, and says nothing when either is missing, so a preset
 * that returned such a config would build an app that is silently offline on
 * the one transport it was chosen for.
 */

import type { ProtocolConfig, TransportsConfig } from './types';

/**
 * Every scenario id, in documentation order. `developmentCleartext` turns
 * end-to-end encryption off: never ship it to production users. Encryption
 * stays on and fail-closed in every other scenario.
 */
export const CONFIG_SCENARIO_IDS = [
  'localBle',
  'proximityMesh',
  'internetRelay',
  'nostrMesh',
  'developmentCleartext',
] as const;

/** Named integration scenarios covered by docs and tests. */
export type ConfigScenario = (typeof CONFIG_SCENARIO_IDS)[number];

/**
 * Recursively optional. Arrays and functions are taken whole: an override
 * replaces them rather than merging element by element.
 */
export type DeepPartial<T> = T extends (...args: never[]) => unknown
  ? T
  : T extends readonly unknown[]
    ? T
    : T extends object
      ? { [K in keyof T]?: DeepPartial<T[K]> }
      : T;

/**
 * Fields layered over a preset. Any nested field may be given alone, for
 * example `{ transports: { internet: { serverAddress } } }`; the preset
 * supplies the rest. `appId` and `profile` are parameters of
 * {@link forScenario}, not overrides.
 */
export type ScenarioOverrides = DeepPartial<Omit<ProtocolConfig, 'appId' | 'profile'>>;

export interface ForScenarioParams {
  appId: string;
  profile: string;
  /** Merged over the preset last; use for relay URLs, the internet server and app tuning. */
  overrides?: ScenarioOverrides;
}

type PlainRecord = Record<string, unknown>;

/**
 * Keys a merge must never copy. A `JSON.parse`d override carries `__proto__`
 * as an own key, and assigning it would replace the prototype of the merged
 * object rather than set a field.
 */
const FORBIDDEN_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

/** An object literal: only these are merged; anything else is replaced whole. */
function isPlainRecord(value: unknown): value is PlainRecord {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return false;
  }
  const proto = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

/**
 * Deep merge of `layer` over `base`. Plain objects merge at every depth;
 * arrays, scalars and other values replace; `undefined` leaves the base value.
 * Neither input is modified.
 */
function deepMerge(base: PlainRecord, layer: PlainRecord): PlainRecord {
  const out: PlainRecord = {};
  for (const [key, value] of Object.entries(base)) {
    if (!FORBIDDEN_KEYS.has(key)) {
      out[key] = isPlainRecord(value) ? deepMerge({}, value) : value;
    }
  }
  for (const [key, value] of Object.entries(layer)) {
    if (FORBIDDEN_KEYS.has(key) || value === undefined) {
      continue;
    }
    const existing = out[key];
    out[key] = isPlainRecord(value)
      ? deepMerge(isPlainRecord(existing) ? existing : {}, value)
      : value;
  }
  return out;
}

/**
 * Returns the preset slice for a scenario (no `appId` / `profile`). A new
 * object on every call, so the caller may modify it.
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
          reticulum: { enabled: false },
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

const TRANSPORT_KEYS: readonly (keyof TransportsConfig)[] = [
  'ble',
  'wifiDirect',
  'internet',
  'nostr',
  'reticulum',
];

/**
 * Throws on a merged config that would start without what it asked for. Each
 * of these is otherwise silent: the engine starts and the transport, or the
 * whole config, is not what the app chose.
 */
function assertStartable(scenario: ConfigScenario, config: ProtocolConfig): void {
  const fail = (detail: string): never => {
    throw new Error(`[OfflineProtocol] forScenario('${scenario}'): ${detail}`);
  };
  const transports = config.transports ?? {};
  for (const key of TRANSPORT_KEYS) {
    const entry = transports[key];
    if (entry !== undefined && typeof entry.enabled !== 'boolean') {
      fail(
        `transports.${key} has no 'enabled' and no preset supplies one. ` +
          `Set transports.${key}.enabled in overrides.`
      );
    }
  }
  const internet = transports.internet;
  if (internet?.enabled && !internet.serverAddress) {
    fail(
      'the internet transport is enabled without a server, so start() would ' +
        'never connect it. Set overrides.transports.internet.serverAddress.'
    );
  }
  const nostr = transports.nostr;
  if (nostr?.enabled) {
    if (!nostr.relayUrls?.length) {
      fail(
        'the Nostr transport is enabled without relays, so start() would never ' +
          'connect it. Set overrides.transports.nostr.relayUrls; the SDK names none.'
      );
    }
    if (config.encryption?.enabled === false) {
      fail(
        'Nostr needs the identity MLS initialization creates, and ' +
          'encryption.enabled: false skips it. Enable encryption or disable Nostr.'
      );
    }
  }
}

/**
 * Builds a full {@link ProtocolConfig} for `appId`, `profile`, the scenario
 * preset, and optional `overrides` (deep-merged last).
 *
 * @throws Error when the result would start without the transport it enables
 *   (internet without `serverAddress`, Nostr without `relayUrls` or with
 *   encryption off), when a transport entry has no `enabled`, or when
 *   `overrides` names `appId` or `profile`.
 */
export function forScenario(
  scenario: ConfigScenario,
  params: ForScenarioParams
): ProtocolConfig {
  const overrides = (params.overrides ?? {}) as PlainRecord;
  for (const key of ['appId', 'profile']) {
    if (Object.prototype.hasOwnProperty.call(overrides, key)) {
      throw new Error(
        `[OfflineProtocol] forScenario('${scenario}'): pass ${key} as a parameter, ` +
          'not inside overrides.'
      );
    }
  }
  const merged = deepMerge(scenarioPreset(scenario) as PlainRecord, overrides);
  const config = {
    ...merged,
    appId: params.appId,
    profile: params.profile,
  } as ProtocolConfig;
  assertStartable(scenario, config);
  if (config.encryption?.enabled === false) {
    console.warn(
      `[OfflineProtocol] forScenario('${scenario}') builds a config with end-to-end ` +
        'encryption OFF: messages travel in cleartext. Lab use only; never ship it.'
    );
  }
  return config;
}
