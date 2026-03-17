import {Platform, PermissionsAndroid, Alert, Linking, NativeModules} from 'react-native';
import type {Permission} from 'react-native';

const {OfflineProtocolModule} = NativeModules;

// ─── User ID & Name ──────────────────────────────────────────

let cachedUserId: string | null = null;

export function generateUserId(): string {
  if (cachedUserId) {
    return cachedUserId;
  }
  const timestamp = Date.now().toString(36);
  const random = Math.random().toString(36).substring(2, 8);
  cachedUserId = `user_${timestamp}_${random}`;
  return cachedUserId;
}

export function generateUserName(): string {
  const adjectives = [
    'Swift', 'Bright', 'Clever', 'Quiet', 'Bold', 'Gentle', 'Wise', 'Kind',
    'Brave', 'Cool', 'Smart', 'Quick', 'Calm', 'Sharp', 'Keen', 'Alert',
  ];
  const nouns = [
    'Fox', 'Eagle', 'Wolf', 'Bear', 'Lion', 'Tiger', 'Hawk', 'Owl',
    'Dolphin', 'Whale', 'Falcon', 'Raven', 'Lynx', 'Puma', 'Shark', 'Phoenix',
  ];
  const adjective = adjectives[Math.floor(Math.random() * adjectives.length)];
  const noun = nouns[Math.floor(Math.random() * nouns.length)];
  const number = Math.floor(Math.random() * 999) + 1;
  return `${adjective}${noun}${number}`;
}

export function formatUserId(userId: string): string {
  return userId.length > 6 ? `...${userId.slice(-6)}` : userId;
}

export function getUserInitials(name: string): string {
  return name
    .split(/(?=[A-Z])/)
    .map(word => word.charAt(0).toUpperCase())
    .join('')
    .substring(0, 2);
}

export function generateAvatarColor(userId: string): string {
  const colors = [
    '#FF6B6B', '#4ECDC4', '#45B7D1', '#96CEB4', '#FECA57',
    '#FF9FF3', '#54A0FF', '#5F27CD', '#00D2D3', '#FF9F43',
    '#10AC84', '#EE5A24', '#0984E3', '#A29BFE', '#FD79A8',
    '#FDCB6E', '#6C5CE7', '#74B9FF', '#00B894', '#E17055',
  ];
  let hash = 0;
  for (let i = 0; i < userId.length; i++) {
    hash = userId.charCodeAt(i) + ((hash << 5) - hash);
  }
  return colors[Math.abs(hash) % colors.length];
}

// ─── Time Formatting ─────────────────────────────────────────

export function formatRelativeTime(timestamp: number): string {
  const now = Date.now();
  const diffMs = now - timestamp;
  const diffSec = Math.floor(diffMs / 1000);
  const diffMin = Math.floor(diffSec / 60);
  const diffHour = Math.floor(diffMin / 60);
  const diffDay = Math.floor(diffHour / 24);

  if (diffSec < 30) {return 'just now';}
  if (diffMin < 1) {return `${diffSec}s ago`;}
  if (diffMin < 60) {return `${diffMin}m ago`;}
  if (diffHour < 24) {return `${diffHour}h ago`;}
  return `${diffDay}d ago`;
}

export function formatMessageTime(timestamp: number): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});
}

// ─── Bluetooth ───────────────────────────────────────────────

export async function isBluetoothEnabled(): Promise<boolean> {
  try {
    if (OfflineProtocolModule?.isBluetoothEnabled) {
      return await OfflineProtocolModule.isBluetoothEnabled();
    }
    return true;
  } catch {
    return true;
  }
}

export async function ensureBluetoothEnabled(): Promise<boolean> {
  const enabled = await isBluetoothEnabled();
  if (!enabled) {
    if (Platform.OS === 'ios') {
      return new Promise(resolve => {
        Alert.alert(
          'Bluetooth Required',
          'Please enable Bluetooth in Settings to use offline messaging.',
          [
            {text: 'Cancel', style: 'cancel', onPress: () => resolve(false)},
            {
              text: 'Open Settings',
              onPress: () => {
                Linking.openURL('App-Prefs:Bluetooth');
                resolve(false);
              },
            },
          ],
        );
      });
    }
    if (Platform.OS === 'android' && OfflineProtocolModule?.requestEnableBluetooth) {
      try {
        return await OfflineProtocolModule.requestEnableBluetooth();
      } catch {
        return false;
      }
    }
    return false;
  }
  return true;
}

// ─── Permissions ─────────────────────────────────────────────

export interface PermissionResult {
  granted: boolean;
  deniedPermissions: string[];
}

export async function requestBluetoothPermissions(): Promise<PermissionResult> {
  if (Platform.OS === 'ios') {
    return {granted: true, deniedPermissions: []};
  }
  if (Platform.OS !== 'android') {
    return {granted: true, deniedPermissions: []};
  }

  try {
    const androidVersion = Platform.Version as number;
    const permissions: Permission[] = [];

    if (androidVersion >= 31) {
      permissions.push(
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_SCAN,
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT,
        PermissionsAndroid.PERMISSIONS.BLUETOOTH_ADVERTISE,
        PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION,
      );
    } else {
      permissions.push(
        PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION,
        PermissionsAndroid.PERMISSIONS.ACCESS_COARSE_LOCATION,
      );
    }

    const results = await PermissionsAndroid.requestMultiple(permissions);
    const deniedPermissions: string[] = [];
    let allGranted = true;

    for (const permission of permissions) {
      if (results[permission] !== PermissionsAndroid.RESULTS.GRANTED) {
        allGranted = false;
        deniedPermissions.push(permission);
      }
    }

    return {granted: allGranted, deniedPermissions};
  } catch {
    return {granted: false, deniedPermissions: ['UNKNOWN_ERROR']};
  }
}
