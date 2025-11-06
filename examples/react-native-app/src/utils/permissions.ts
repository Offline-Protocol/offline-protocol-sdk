import { Platform, PermissionsAndroid, Alert } from 'react-native';

export interface PermissionResult {
  granted: boolean;
  deniedPermissions: string[];
}

/**
 * Request all Bluetooth and Location permissions required for the Offline Protocol
 * Handles both Android 12+ (API 31+) and legacy Android versions
 */
export async function requestBluetoothPermissions(): Promise<PermissionResult> {
  if (Platform.OS === 'ios') {
    // iOS permissions are handled through Info.plist and requested automatically
    // when Bluetooth is accessed
    return { granted: true, deniedPermissions: [] };
  }

  if (Platform.OS !== 'android') {
    return { granted: true, deniedPermissions: [] };
  }

  try {
    const androidVersion = Platform.Version as number;
    const permissions: string[] = [];

    // Android 12+ (API 31+) uses new Bluetooth permissions
    if (androidVersion >= 31) {
      permissions.push(
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_SCAN,
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT,
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_ADVERTISE,
        PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION
      );
    } else {
      // Legacy Android (API 23-30) requires Location permission for BLE
      permissions.push(
        PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION,
        PermissionsAndroid.PERMISSIONS.ACCESS_COARSE_LOCATION
      );
    }

    console.log('Requesting permissions:', permissions);

    // Request all permissions at once
    const results = await PermissionsAndroid.requestMultiple(permissions);

    // Check which permissions were granted
    const deniedPermissions: string[] = [];
    let allGranted = true;

    for (const permission of permissions) {
      const result = results[permission];
      if (result !== PermissionsAndroid.RESULTS.GRANTED) {
        allGranted = false;
        deniedPermissions.push(permission);
        console.warn(`Permission denied: ${permission} (${result})`);
      } else {
        console.log(`Permission granted: ${permission}`);
      }
    }

    if (!allGranted) {
      console.error('Not all permissions granted:', deniedPermissions);
    }

    return {
      granted: allGranted,
      deniedPermissions,
    };
  } catch (error) {
    console.error('Error requesting permissions:', error);
    return {
      granted: false,
      deniedPermissions: ['UNKNOWN_ERROR'],
    };
  }
}

/**
 * Check if all required Bluetooth permissions are already granted
 */
export async function checkBluetoothPermissions(): Promise<boolean> {
  if (Platform.OS === 'ios') {
    // iOS permissions can't be checked directly
    return true;
  }

  if (Platform.OS !== 'android') {
    return true;
  }

  try {
    const androidVersion = Platform.Version as number;
    const permissions: string[] = [];

    if (androidVersion >= 31) {
      permissions.push(
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_SCAN,
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT,
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_ADVERTISE,
        PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION
      );
    } else {
      permissions.push(
        PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION,
        PermissionsAndroid.PERMISSIONS.ACCESS_COARSE_LOCATION
      );
    }

    // Check all permissions
    for (const permission of permissions) {
      const granted = await PermissionsAndroid.check(permission);
      if (!granted) {
        console.log(`Permission not granted: ${permission}`);
        return false;
      }
    }

    return true;
  } catch (error) {
    console.error('Error checking permissions:', error);
    return false;
  }
}

/**
 * Show an explanation dialog before requesting permissions
 */
export function showPermissionRationale(): Promise<boolean> {
  return new Promise((resolve) => {
    Alert.alert(
      'Bluetooth Permissions Required',
      'This app needs Bluetooth and Location permissions to discover and communicate with nearby devices when offline.\n\n' +
      'Location permission is required by Android for Bluetooth scanning, but your location is never tracked or stored.',
      [
        {
          text: 'Cancel',
          style: 'cancel',
          onPress: () => resolve(false),
        },
        {
          text: 'Grant Permissions',
          onPress: () => resolve(true),
        },
      ]
    );
  });
}

/**
 * Get a user-friendly message for denied permissions
 */
export function getPermissionDeniedMessage(deniedPermissions: string[]): string {
  if (deniedPermissions.length === 0) {
    return '';
  }

  const permissionNames = deniedPermissions.map(permission => {
    if (permission.includes('BLUETOOTH')) {
      return 'Bluetooth';
    }
    if (permission.includes('LOCATION')) {
      return 'Location';
    }
    return permission;
  });

  const uniqueNames = Array.from(new Set(permissionNames));

  return `The following permissions were denied: ${uniqueNames.join(', ')}.\n\n` +
    'Please grant these permissions in Settings to use offline messaging features.';
}



