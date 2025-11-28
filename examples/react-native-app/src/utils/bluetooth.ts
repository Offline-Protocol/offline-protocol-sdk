import { Platform, NativeModules, Alert, Linking } from 'react-native';

const { OfflineProtocolModule } = NativeModules;

/**
 * Check if Bluetooth is enabled on the device
 * Note: On Android 12+, requires BLUETOOTH_CONNECT permission
 */
export async function isBluetoothEnabled(): Promise<boolean> {
  try {
    if (OfflineProtocolModule?.isBluetoothEnabled) {
      return await OfflineProtocolModule.isBluetoothEnabled();
    }
    // Fallback: assume enabled if module not available
    console.log('OfflineProtocolModule not available, assuming Bluetooth is enabled');
    return true;
  } catch (error) {
    console.warn('Could not check Bluetooth state:', error);
    return true;
  }
}

/**
 * Request the user to enable Bluetooth
 * On Android, shows a system dialog to enable Bluetooth
 * On iOS, directs user to Settings
 */
export async function requestEnableBluetooth(): Promise<boolean> {
  if (Platform.OS === 'ios') {
    // iOS doesn't allow programmatic Bluetooth enabling
    // Show alert directing user to Settings
    return new Promise((resolve) => {
      Alert.alert(
        'Bluetooth Required',
        'Please enable Bluetooth in Settings to use offline messaging features.',
        [
          {
            text: 'Cancel',
            style: 'cancel',
            onPress: () => resolve(false),
          },
          {
            text: 'Open Settings',
            onPress: () => {
              Linking.openURL('App-Prefs:Bluetooth');
              resolve(false); // User needs to come back after enabling
            },
          },
        ]
      );
    });
  }

  if (Platform.OS === 'android') {
    try {
      if (OfflineProtocolModule?.requestEnableBluetooth) {
        const enabled = await OfflineProtocolModule.requestEnableBluetooth();
        if (!enabled) {
          // System dialog was shown, show additional alert
          return showBluetoothAlert();
        }
        return enabled;
      }
      return showBluetoothAlert();
    } catch (error) {
      console.warn('Could not request Bluetooth enable:', error);
      return showBluetoothAlert();
    }
  }

  return true;
}

/**
 * Show a generic Bluetooth alert when we can't programmatically enable it
 */
function showBluetoothAlert(): Promise<boolean> {
  return new Promise((resolve) => {
    Alert.alert(
      'Bluetooth Required',
      'Please enable Bluetooth to use offline messaging features.',
      [
        {
          text: 'OK',
          onPress: () => resolve(false),
        },
      ]
    );
  });
}

/**
 * Check Bluetooth and prompt user if it's disabled
 * Returns true if Bluetooth is enabled or user enabled it
 */
export async function ensureBluetoothEnabled(): Promise<boolean> {
  const enabled = await isBluetoothEnabled();
  
  if (!enabled) {
    console.log('Bluetooth is disabled, requesting user to enable it');
    return await requestEnableBluetooth();
  }
  
  return true;
}




