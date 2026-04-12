import React, {useState} from 'react';
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  Alert,
  ActivityIndicator,
} from 'react-native';
import {SafeAreaView} from 'react-native-safe-area-context';
import {generateUserId, generateUserName, requestBluetoothPermissions, ensureBluetoothEnabled, showPermissionDeniedAlert} from '../utils';
import {useProtocol} from '../context/ProtocolContext';

interface OnboardingScreenProps {
  onComplete: () => void;
}

export function OnboardingScreen({onComplete}: OnboardingScreenProps) {
  const [name, setName] = useState(generateUserName);
  const [isLoading, setIsLoading] = useState(false);
  const {initialize} = useProtocol();

  const handleStart = async () => {
    if (!name.trim()) {
      Alert.alert('Name Required', 'Please enter a display name.');
      return;
    }

    setIsLoading(true);
    try {
      // Request Bluetooth permissions
      const permResult = await requestBluetoothPermissions();
      if (!permResult.granted) {
        showPermissionDeniedAlert(permResult);
        setIsLoading(false);
        return;
      }

      // Ensure Bluetooth is enabled
      const btEnabled = await ensureBluetoothEnabled();
      if (!btEnabled) {
        Alert.alert(
          'Bluetooth Required',
          'Please enable Bluetooth to use offline messaging.',
        );
        setIsLoading(false);
        return;
      }

      // Initialize protocol
      const userId = generateUserId();
      await initialize(userId, name.trim());
      onComplete();
    } catch (error) {
      console.error('Failed to start protocol:', error);
      Alert.alert('Error', 'Failed to start the protocol. Please try again.');
    } finally {
      setIsLoading(false);
    }
  };

  const handleRandomize = () => {
    setName(generateUserName());
  };

  return (
    <SafeAreaView style={styles.container}>
      <View style={styles.content}>
        <Text style={styles.emoji}>📡</Text>
        <Text style={styles.title}>Offline Demo</Text>
        <Text style={styles.subtitle}>
          Encrypted mesh messaging{'\n'}No internet required
        </Text>

        <View style={styles.inputSection}>
          <Text style={styles.label}>Display Name</Text>
          <View style={styles.inputRow}>
            <TextInput
              style={styles.input}
              value={name}
              onChangeText={setName}
              placeholder="Enter your name"
              maxLength={20}
              autoCapitalize="words"
              editable={!isLoading}
            />
            <TouchableOpacity
              style={styles.randomizeButton}
              onPress={handleRandomize}
              disabled={isLoading}>
              <Text style={styles.randomizeText}>🎲</Text>
            </TouchableOpacity>
          </View>
        </View>

        <TouchableOpacity
          style={[styles.startButton, isLoading && styles.startButtonDisabled]}
          onPress={handleStart}
          disabled={isLoading}
          activeOpacity={0.8}>
          {isLoading ? (
            <ActivityIndicator color="#FFFFFF" />
          ) : (
            <Text style={styles.startButtonText}>Start Mesh Network</Text>
          )}
        </TouchableOpacity>

        <View style={styles.features}>
          <Text style={styles.featureItem}>🔒 End-to-end encrypted (MLS)</Text>
          <Text style={styles.featureItem}>📶 Bluetooth Low Energy mesh</Text>
          <Text style={styles.featureItem}>👥 Peer discovery & groups</Text>
          <Text style={styles.featureItem}>⚡ Service registry & discovery</Text>
        </View>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#F2F2F7',
  },
  content: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
  },
  emoji: {
    fontSize: 64,
    marginBottom: 16,
  },
  title: {
    fontSize: 32,
    fontWeight: '800',
    color: '#1C1C1E',
    marginBottom: 8,
  },
  subtitle: {
    fontSize: 16,
    color: '#8E8E93',
    textAlign: 'center',
    lineHeight: 22,
    marginBottom: 40,
  },
  inputSection: {
    width: '100%',
    marginBottom: 24,
  },
  label: {
    fontSize: 14,
    fontWeight: '600',
    color: '#3C3C43',
    marginBottom: 8,
  },
  inputRow: {
    flexDirection: 'row',
    gap: 8,
  },
  input: {
    flex: 1,
    backgroundColor: '#FFFFFF',
    borderRadius: 12,
    paddingHorizontal: 16,
    paddingVertical: 14,
    fontSize: 17,
    color: '#1C1C1E',
    borderWidth: 1,
    borderColor: '#E5E5E5',
  },
  randomizeButton: {
    backgroundColor: '#FFFFFF',
    borderRadius: 12,
    width: 48,
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: '#E5E5E5',
  },
  randomizeText: {
    fontSize: 22,
  },
  startButton: {
    width: '100%',
    backgroundColor: '#007AFF',
    borderRadius: 14,
    paddingVertical: 16,
    alignItems: 'center',
    marginBottom: 32,
  },
  startButtonDisabled: {
    opacity: 0.6,
  },
  startButtonText: {
    color: '#FFFFFF',
    fontSize: 17,
    fontWeight: '700',
  },
  features: {
    gap: 8,
  },
  featureItem: {
    fontSize: 14,
    color: '#636366',
    lineHeight: 20,
  },
});
