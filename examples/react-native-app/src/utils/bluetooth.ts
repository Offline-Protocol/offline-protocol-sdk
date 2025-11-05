import { Platform, NativeModules, Alert, Linking } from 'react-native';

/**
 * Check if Bluetooth is enabled on the device
 * Note: On Android 12+, requires BLUETOOTH_CONNECT permission
 */
export async function isBluetoothEnabled(): Promise<boolean> {
  if (Platform.OS === 'ios') {
    // iOS doesn't allow checking Bluetooth state directly
    // The system will prompt automatically when trying to use Bluetooth
    return true;
  }

  if (Platform.OS === 'android') {
    try {
      // Try to use the BluetoothAdapter to check if Bluetooth is enabled
      // This is a simple approach that works without additional dependencies
      const { RNBluetoothManager } = NativeModules;
      
      // If we don't have a native module, assume Bluetooth is available
      // The protocol will fail gracefully if it's not
      if (!RNBluetoothManager) {
        console.log('Native Bluetooth module not available, assuming Bluetooth is enabled');
        return true;
      }

      const enabled = await RNBluetoothManager.isEnabled();
      return enabled;
    } catch (error) {
      console.warn('Could not check Bluetooth state:', error);
      // If we can't check, assume it's enabled and let the protocol handle it
      return true;
    }
  }

  return true;
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
      const { RNBluetoothManager } = NativeModules;
      
      if (!RNBluetoothManager) {
        // No native module available, show a generic alert
        return showBluetoothAlert();
      }

      // Request to enable Bluetooth (shows system dialog on Android)
      const enabled = await RNBluetoothManager.enable();
      return enabled;
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

