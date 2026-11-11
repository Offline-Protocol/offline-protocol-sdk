/**
 * Development-only ordering hints for the React Native lifecycle.
 *
 * Never blocks calls; mirrors the integrator map ordering (listeners →
 * interest → start → send).
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
  return typeof __DEV__ !== 'undefined' && __DEV__;
}

export function currentDevLifecycleWarnings(): boolean {
  return devLifecycleWarningsEnabled(moduleDevLifecycleWarnings);
}

function warnOnce(key: string, message: string, enabled: boolean): void {
  if (!enabled) {
    return;
  }
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
    '[OfflineProtocol] start() with no on(...) listeners registered. ' +
    'Startup events (neighbor_discovered, message_received, …) may be dropped. ' +
    `Register listeners before start(). See ${LIFECYCLE_DOC}`
  );
}

export function lifecycleDataBeforeStartMessage(context: string): string {
  return (
    `[OfflineProtocol] ${context}: the data store is not open yet. ` +
    'Call await protocol.start() first, or use setInterest() before start() for ' +
    'replication policy. See data-interest-order in the RN package js-ci-harness.'
  );
}

export function lifecycleInterestAfterStartMessage(): string {
  return (
    '[OfflineProtocol] setInterest after start() can only widen what is replicated; ' +
    'narrowing does not delete documents already pulled during start-up. ' +
    'Declare interest before start() when you need a tight filter from the first exchange.'
  );
}

export function lifecycleSendWithoutMlsMessage(context: string): string {
  return (
    `[OfflineProtocol] ${context}: encryption is enabled but MLS did not initialize during ` +
    'start(). Secure send and handshake may fail until MLS is ready. ' +
    'Fix MLS init errors logged at start(), or set encryption.enabled: false for cleartext-only tests.'
  );
}

function lifecycleWarningsOn(devFlag?: boolean): boolean {
  if (devFlag !== undefined) {
    return devLifecycleWarningsEnabled(devFlag);
  }
  return currentDevLifecycleWarnings();
}

export function warnIfCallBeforeEngineStart(
  context: string,
  engineStarted: boolean,
  devFlag?: boolean
): void {
  const enabled = lifecycleWarningsOn(devFlag);
  if (engineStarted || !enabled) {
    return;
  }
  warnOnce(
    `engine:${context}`,
    lifecycleNeedsEngineMessage(context),
    true
  );
}

export function warnIfNoListenersAtStart(
  listenerCount: number,
  devFlag?: boolean
): void {
  const enabled = lifecycleWarningsOn(devFlag);
  if (listenerCount > 0 || !enabled) {
    return;
  }
  warnOnce('start:no-listeners', lifecycleNoListenersMessage(), true);
}

export function warnIfDataMutationBeforeStoreReady(
  context: string,
  storeReady: boolean,
  devFlag?: boolean
): void {
  const enabled = lifecycleWarningsOn(devFlag);
  if (storeReady || !enabled) {
    return;
  }
  warnOnce(
    `data:${context}`,
    lifecycleDataBeforeStartMessage(context),
    true
  );
}

export function warnIfInterestAfterStart(devFlag?: boolean): void {
  const enabled = lifecycleWarningsOn(devFlag);
  if (!enabled) {
    return;
  }
  warnOnce('interest:after-start', lifecycleInterestAfterStartMessage(), true);
}

export function warnIfSendWithoutMls(
  context: string,
  encryptionEnabled: boolean,
  mlsReadyAfterStart: boolean,
  engineStarted: boolean,
  devFlag?: boolean
): void {
  const enabled = lifecycleWarningsOn(devFlag);
  if (
    !enabled ||
    !engineStarted ||
    !encryptionEnabled ||
    mlsReadyAfterStart
  ) {
    return;
  }
  warnOnce(`mls:${context}`, lifecycleSendWithoutMlsMessage(context), true);
}
