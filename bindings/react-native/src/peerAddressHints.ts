/**
 * Shape checks for mesh addresses (`off1…`, 44 lowercase characters).
 *
 * Mirrors `Username::looks_like_address` in offline-protocol-core: a shape test
 * only, not a checksum parse. Used for integrator hints in development.
 */

/** Canonical encoded length; pinned with `Address::ENCODED_LEN` in core. */
export const MESH_ADDRESS_ENCODED_LEN = 44;

/** Versioned bech32m HRP + data prefix for every address in this spec. */
export const MESH_ADDRESS_PREFIX = 'off1';

declare const __DEV__: boolean | undefined;

/**
 * Whether `candidate` has the shape of a mesh address (not whether checksum validates).
 */
export function looksLikeMeshAddress(candidate: string): boolean {
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
  return typeof __DEV__ !== 'undefined' && __DEV__;
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

const warnedKeys = new Set<string>();

/**
 * Logs once per `(context, peerId)` when warnings are enabled. Never throws.
 */
export function warnPeerIdIfMisshapen(
  peerId: string,
  context: string,
  devPeerAddressWarnings?: boolean
): void {
  if (!devPeerAddressWarningsEnabled(devPeerAddressWarnings)) {
    return;
  }
  if (!peerId || looksLikeMeshAddress(peerId)) {
    return;
  }
  const key = `${context}\0${peerId}`;
  if (warnedKeys.has(key)) {
    return;
  }
  warnedKeys.add(key);
  console.warn(peerAddressMisshapeMessage(peerId, context));
}
