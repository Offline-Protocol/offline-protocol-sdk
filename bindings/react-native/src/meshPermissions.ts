/**
 * React Native mesh runtime permission helpers (Android).
 *
 * Contract: docs/mobile-permissions.md. Info.plist and manifest declarations
 * remain the app's responsibility; this module only requests Android dangerous
 * permissions that mesh transports check before `start()`.
 */

import { PermissionsAndroid, Platform, type Permission } from 'react-native';
import type {
  MeshPermissionRequestOptions,
  MeshPermissionRequestResult,
  MeshPermissionStatus,
  ProtocolConfig,
} from './types';

/** First Android API level that uses runtime Bluetooth permissions (API 31). */
export const ANDROID_API_BLUETOOTH_RUNTIME = 31;

/** Android 13 (API 33): Wi‑Fi Direct uses NEARBY_WIFI_DEVICES. */
export const ANDROID_API_NEARBY_WIFI = 33;

/** Android 17 (API 37): LAN discovery needs ACCESS_LOCAL_NETWORK when targeted. */
export const ANDROID_API_LOCAL_NETWORK = 37;

const IOS_PREFLIGHT_MESSAGE =
  'iOS permissions are prompted when mesh starts. Add the Info.plist keys in docs/mobile-permissions.md before calling start().';

/**
 * Builds the Android dangerous-permission list to request for the given options.
 * Exported for unit tests; matches BleTransportFacade and WifiDirectManager checks.
 */
export function buildAndroidMeshPermissionList(
  options: MeshPermissionRequestOptions,
  apiLevel: number,
): Permission[] {
  const ble = options.ble ?? true;
  const wifiDirect = options.wifiDirect ?? false;
  const localNetwork = options.localNetwork ?? false;

  const permissions: Permission[] = [];
  const P = PermissionsAndroid.PERMISSIONS;

  if (ble) {
    if (apiLevel >= ANDROID_API_BLUETOOTH_RUNTIME) {
      permissions.push(P.BLUETOOTH_SCAN, P.BLUETOOTH_CONNECT, P.BLUETOOTH_ADVERTISE);
    } else if (apiLevel >= 23) {
      permissions.push(P.ACCESS_FINE_LOCATION);
    }
  }

  if (wifiDirect) {
    if (apiLevel >= ANDROID_API_NEARBY_WIFI) {
      permissions.push(P.NEARBY_WIFI_DEVICES);
    } else if (apiLevel >= 23) {
      permissions.push(P.ACCESS_FINE_LOCATION);
    }
  }

  if (localNetwork && apiLevel >= ANDROID_API_LOCAL_NETWORK) {
    permissions.push(P.ACCESS_LOCAL_NETWORK);
  }

  return [...new Set(permissions)];
}

function mapAndroidResult(
  raw: Record<string, string>,
): Record<string, MeshPermissionStatus> {
  const out: Record<string, MeshPermissionStatus> = {};
  for (const [permission, status] of Object.entries(raw)) {
    if (status === PermissionsAndroid.RESULTS.GRANTED) {
      out[permission] = 'granted';
    } else if (status === PermissionsAndroid.RESULTS.NEVER_ASK_AGAIN) {
      out[permission] = 'never_ask_again';
    } else {
      out[permission] = 'denied';
    }
  }
  return out;
}

/**
 * Derives permission options from transport toggles in a {@link ProtocolConfig}.
 */
export function meshPermissionOptionsFromConfig(
  config: Pick<ProtocolConfig, 'transports'>,
): MeshPermissionRequestOptions {
  const ble = config.transports?.ble?.enabled ?? true;
  const wifiDirect =
    Platform.OS === 'android' && (config.transports?.wifiDirect?.enabled ?? false);

  return {
    ble,
    wifiDirect,
    // LAN rides the same peer-stream slot; API 37+ apps need the grant when that slot is on.
    localNetwork: wifiDirect,
  };
}

/**
 * Requests mesh-related Android runtime permissions before {@link OfflineProtocol.start}.
 *
 * On iOS, returns immediately with `granted: true` and a message: Bluetooth and
 * local-network prompts are system-driven when the native transport starts.
 *
 * On Android, requests only permissions not already granted. Does not modify
 * `ProtocolConfig` or start transports.
 */
export async function requestMeshPermissions(
  options: MeshPermissionRequestOptions = {},
): Promise<MeshPermissionRequestResult> {
  if (Platform.OS === 'ios') {
    return {
      granted: true,
      platform: 'ios',
      message: IOS_PREFLIGHT_MESSAGE,
    };
  }

  if (Platform.OS !== 'android') {
    return {
      granted: true,
      platform: 'other',
      message: 'requestMeshPermissions applies to Android and iOS only.',
    };
  }

  const apiLevel =
    typeof Platform.Version === 'number'
      ? Platform.Version
      : parseInt(String(Platform.Version), 10);

  const toRequest = buildAndroidMeshPermissionList(options, apiLevel);

  const needPrompt: Permission[] = [];
  for (const permission of toRequest) {
    const already = await PermissionsAndroid.check(permission);
    if (!already) {
      needPrompt.push(permission);
    }
  }

  if (needPrompt.length === 0) {
    return {
      granted: true,
      platform: 'android',
      android: {},
      message: 'All requested mesh permissions were already granted.',
    };
  }

  const raw = await PermissionsAndroid.requestMultiple(needPrompt);
  const rawByPermission = raw as Record<string, string>;
  const android = mapAndroidResult(rawByPermission);
  const granted = needPrompt.every(
    (p) => rawByPermission[p] === PermissionsAndroid.RESULTS.GRANTED,
  );

  return {
    granted,
    platform: 'android',
    android,
  };
}

/**
 * Convenience wrapper: {@link meshPermissionOptionsFromConfig} then
 * {@link requestMeshPermissions}.
 */
export async function requestMeshPermissionsForConfig(
  config: Pick<ProtocolConfig, 'transports'>,
): Promise<MeshPermissionRequestResult> {
  return requestMeshPermissions(meshPermissionOptionsFromConfig(config));
}
