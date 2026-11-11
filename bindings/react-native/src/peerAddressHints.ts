/**
 * Shape checks for mesh addresses (`off1…`, 44 lowercase characters).
 *
 * Mirrors `Username::looks_like_address` in offline-protocol-core: a shape test
 * only, not a checksum parse. Used for integrator hints in development.
 */

/**
 * Canonical encoded length. Hand-mirrored from `Address::ENCODED_LEN`;
 * `the_react_native_address_hint_mirrors_the_core_constants` in
 * offline-protocol-core reads this file and fails if the two disagree.
 */
export const MESH_ADDRESS_ENCODED_LEN = 44;

/**
 * `Address::HRP_STR` plus the bech32 separator, pinned by the same guard.
 */
export const MESH_ADDRESS_PREFIX = 'off1';

declare const __DEV__: boolean | undefined;

/**
 * Whether `candidate` has the shape of a mesh address (not whether checksum validates).
 */
export function looksLikeMeshAddress(candidate: string): boolean {
  if (typeof candidate !== 'string') {
    return false;
  }
  if (candidate.length !== MESH_ADDRESS_ENCODED_LEN) {
    return false;
  }
  if (!candidate.startsWith(MESH_ADDRESS_PREFIX)) {
    return false;
  }
  for (let i = 0; i < candidate.length; i++) {
    const code = candidate.charCodeAt(i);
    const lower = code >= 0x61 && code <= 0x7a;
    const digit = code >= 0x30 && code <= 0x39;
    if (!lower && !digit) {
      return false;
    }
  }
  return true;
}

/** Default: warn in React Native `__DEV__` builds only. */
export function devPeerAddressWarningsEnabled(
  flag: boolean | undefined
): boolean {
  if (flag === false) {
    return false;
  }
  if (flag === true) {
    return true;
  }
  return typeof __DEV__ !== 'undefined' && __DEV__ === true;
}

/**
 * The `ProtocolConfig.devPeerAddressWarnings` in force.
 *
 * Module state, not a per-call argument, because `DataStore` and
 * `MeshServices` are constructed without a config and still take peer ids.
 * A per-call flag left those call sites on the `__DEV__` default, so `false`
 * did not silence them and `true` did not force them on. They already share
 * the one native module with `OfflineProtocol`, so they share this setting
 * too: the most recently constructed `OfflineProtocol` sets it.
 */
let configuredFlag: boolean | undefined;

/** Called by the `OfflineProtocol` constructor with its config value. */
export function setDevPeerAddressWarnings(flag: boolean | undefined): void {
  configuredFlag = flag;
}

const WARN_DOC =
  'docs/identity-for-integrators.md (use neighbor_discovered.peer_id or localAddress()).';

export function peerAddressMisshapeMessage(
  peerId: string,
  context: string
): string {
  const shown =
    peerId.length > 64 ? `${peerId.slice(0, 32)}…` : peerId;
  return (
    `[OfflineProtocol] ${context}: peer id "${shown}" does not look like a ` +
    `mesh address (expected ${MESH_ADDRESS_ENCODED_LEN}-char ${MESH_ADDRESS_PREFIX}…). ` +
    `See ${WARN_DOC}`
  );
}

/**
 * Bound on remembered `(context, peerId)` pairs. Forced on in a release
 * build, an app passing many distinct bad ids would otherwise grow the set
 * for the life of the process. Past the bound, warnings for new pairs still
 * print; they are just no longer deduplicated.
 */
export const MAX_WARNED_KEYS = 256;

const warnedKeys = new Set<string>();

/**
 * Logs once per `(context, peerId)` when warnings are enabled. Never throws.
 */
export function warnPeerIdIfMisshapen(peerId: string, context: string): void {
  if (!devPeerAddressWarningsEnabled(configuredFlag)) {
    return;
  }
  if (typeof peerId !== 'string' || !peerId || looksLikeMeshAddress(peerId)) {
    return;
  }
  const key = `${context}\0${peerId}`;
  if (warnedKeys.has(key)) {
    return;
  }
  if (warnedKeys.size < MAX_WARNED_KEYS) {
    warnedKeys.add(key);
  }
  console.warn(peerAddressMisshapeMessage(peerId, context));
}

/** Test seam: forget every remembered pair. */
export function resetPeerAddressWarningsForTests(): void {
  warnedKeys.clear();
}
