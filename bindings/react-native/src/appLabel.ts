/**
 * `appLabel`, and `appId` as its deprecated old name.
 *
 * Before 0.35 the mesh value was called `appId`, and developers mixed it up
 * with the App ID (`app_…`) the developer portal issues for telemetry. It is
 * now `appLabel`. Only the name changed: the value still selects the storage
 * namespace, is stamped on every frame and derives the BLE app tag, so an app
 * that shipped must keep passing the same string, under either name.
 *
 * Internal to the package: `index.ts` imports these and re-exports none.
 */

declare const __DEV__: boolean | undefined;

export const APP_ID_DEPRECATION_MESSAGE =
  '[OfflineProtocol] appId on the mesh is now appLabel. Your App ID (app_…) ' +
  'from the dashboard is what turns on telemetry.';

let warned = false;

function warnDeprecatedAppIdOnce(): void {
  if (warned || typeof __DEV__ === 'undefined' || __DEV__ !== true) {
    return;
  }
  warned = true;
  console.warn(APP_ID_DEPRECATION_MESSAGE);
}

/** For tests: forget that the warning was shown. */
export function resetAppLabelWarningForTests(): void {
  warned = false;
}

/**
 * The label from `appLabel` or its old name `appId`.
 *
 * Both set to different values is refused rather than one silently winning,
 * since either could be the one the developer meant, and a wrong guess moves
 * the app to another storage namespace. `appId` alone warns once in
 * development builds. `null` under either name counts as not set, as the
 * native parsers and Flutter read it.
 */
export function resolveAppLabel(
  rawAppLabel: string | null | undefined,
  rawAppId: string | null | undefined,
  context: string
): string | undefined {
  const appLabel = rawAppLabel ?? undefined;
  const appId = rawAppId ?? undefined;
  if (appLabel !== undefined && appId !== undefined && appLabel !== appId) {
    throw new Error(
      `[OfflineProtocol] ${context}: appLabel "${appLabel}" and appId "${appId}" disagree. ` +
        'appId is the old name of appLabel; set only appLabel.'
    );
  }
  if (appLabel === undefined && appId !== undefined) {
    warnDeprecatedAppIdOnce();
    return appId;
  }
  return appLabel;
}
