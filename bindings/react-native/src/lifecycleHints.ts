/**
 * Development-only ordering hints for the React Native lifecycle.
 *
 * Never blocks calls; mirrors the integrator map ordering (listeners →
 * interest → start → send).
 *
 * Internal to the package: `index.ts` imports these and re-exports none of
 * them, so the helpers and the module state below can change without a
 * breaking release. A hint that fires on correct code teaches developers to
 * ignore the channel, so every warning here must be one a correctly ordered
 * app never sees.
 */

declare const __DEV__: boolean | undefined;

/** Last `ProtocolConfig.devLifecycleWarnings` from an `OfflineProtocol` ctor. */
let moduleDevLifecycleWarnings: boolean | undefined;

/** Mirrors `OfflineProtocol` engine started state for stateless helpers (`DataStore`, `MeshServices`). */
let moduleEngineStarted = false;

const LIFECYCLE_DOC =
  'bindings/react-native/README.md#protocol-lifecycle and docs/integrator-map.md';

const warnedKeys = new Set<string>();

/** Called from `OfflineProtocol` construction so `DataStore` shares the flag. */
export function bindDevLifecycleWarningsFromConfig(flag: boolean | undefined): void {
  moduleDevLifecycleWarnings = flag;
}

export function setModuleEngineStartedForHints(started: boolean): void {
  moduleEngineStarted = started;
}

export function isModuleEngineStarted(): boolean {
  return moduleEngineStarted;
}

export function devLifecycleWarningsEnabled(
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

export function currentDevLifecycleWarnings(): boolean {
  return devLifecycleWarningsEnabled(moduleDevLifecycleWarnings);
}

/** Logs `message` the first time `key` is seen in this process. */
function warnOnce(key: string, message: string): void {
  if (warnedKeys.has(key)) {
    return;
  }
  warnedKeys.add(key);
  console.warn(message);
}

export function lifecycleNeedsEngineMessage(context: string): string {
  return (
    `[OfflineProtocol] ${context}: call await protocol.start() before this API. ` +
    `Register listeners with on(...) and declare data interest before start(). ` +
    `See ${LIFECYCLE_DOC}`
  );
}

export function lifecycleNoListenersMessage(): string {
  return (
    '[OfflineProtocol] start() with no on(...) listeners registered. Inbound ' +
    'messages are held for the first listener, but status events that arrive ' +
    'before one is registered (neighbor_discovered, network_metrics, …) are ' +
    `dropped. Register listeners before start(). See ${LIFECYCLE_DOC}`
  );
}

export function lifecycleDataBeforeStartMessage(context: string): string {
  return (
    `[OfflineProtocol] ${context}: the data store is not open yet. ` +
    'Call await protocol.start() first; setInterest() is the one data call ' +
    'that may come before start(). See docs/data.md.'
  );
}

export function lifecycleInterestNarrowedAfterStartMessage(
  spaceId: string
): string {
  return (
    `[OfflineProtocol] setInterest("${spaceId}") after start() narrowed the ` +
    'interest. The narrower filter refuses documents from now on, but does not ' +
    'delete what this device already holds, including what the start-up ' +
    'exchange pulled in. Declare interest before start() when you need a tight ' +
    'filter from the first exchange.'
  );
}

export function lifecycleSendWithoutMlsMessage(context: string): string {
  return (
    `[OfflineProtocol] ${context}: encryption is enabled but MLS is not ` +
    'initialized. Secure send and handshake may fail until it is. Fix the MLS ' +
    'init errors logged at start() (or call initializeMlsWithSecureStorage()), ' +
    'or set encryption.enabled: false for cleartext-only tests.'
  );
}

function lifecycleWarningsOn(devFlag?: boolean): boolean {
  if (devFlag !== undefined) {
    return devLifecycleWarningsEnabled(devFlag);
  }
  return currentDevLifecycleWarnings();
}

/**
 * Whether interest pattern `outer` matches every document `inner` matches.
 * A pattern is a document name, or a prefix ending in `*`.
 */
function patternCovers(outer: string, inner: string): boolean {
  if (outer === inner) {
    return true;
  }
  if (!outer.endsWith('*')) {
    return false;
  }
  return inner.startsWith(outer.slice(0, -1));
}

/**
 * Whether replacing interest `previous` with `next` drops any document
 * `previous` matched. Conservative: a pattern counts as kept only when one
 * pattern of `next` covers it whole, so splitting `a*` into `ab*` and `a*x`
 * style unions reads as a narrowing.
 */
export function isInterestNarrowing(
  previous: readonly string[],
  next: readonly string[]
): boolean {
  return previous.some(
    (old) => !next.some((candidate) => patternCovers(candidate, old))
  );
}

export function warnIfCallBeforeEngineStart(
  context: string,
  engineStarted: boolean,
  devFlag?: boolean
): void {
  if (engineStarted || !lifecycleWarningsOn(devFlag)) {
    return;
  }
  warnOnce(`engine:${context}`, lifecycleNeedsEngineMessage(context));
}

export function warnIfNoListenersAtStart(
  listenerCount: number,
  devFlag?: boolean
): void {
  if (listenerCount > 0 || !lifecycleWarningsOn(devFlag)) {
    return;
  }
  warnOnce('start:no-listeners', lifecycleNoListenersMessage());
}

export function warnIfDataCallBeforeStoreReady(
  context: string,
  storeReady: boolean,
  devFlag?: boolean
): void {
  if (storeReady || !lifecycleWarningsOn(devFlag)) {
    return;
  }
  warnOnce(`data:${context}`, lifecycleDataBeforeStartMessage(context));
}

/**
 * Warns when a post-start `setInterest` narrows the space's interest. A
 * widening is the supported post-start use (it asks peers for what it adds)
 * and never warns.
 */
export function warnIfInterestNarrowedAfterStart(
  spaceId: string,
  previous: readonly string[],
  next: readonly string[],
  devFlag?: boolean
): void {
  if (!lifecycleWarningsOn(devFlag) || !isInterestNarrowing(previous, next)) {
    return;
  }
  warnOnce(
    `interest:narrowed:${spaceId}`,
    lifecycleInterestNarrowedAfterStartMessage(spaceId)
  );
}

export function warnIfSendWithoutMls(
  context: string,
  encryptionEnabled: boolean,
  mlsReady: boolean,
  engineStarted: boolean,
  devFlag?: boolean
): void {
  if (
    !engineStarted ||
    !encryptionEnabled ||
    mlsReady ||
    !lifecycleWarningsOn(devFlag)
  ) {
    return;
  }
  warnOnce(`mls:${context}`, lifecycleSendWithoutMlsMessage(context));
}
