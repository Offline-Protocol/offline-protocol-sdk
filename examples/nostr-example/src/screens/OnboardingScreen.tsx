import React, {useState} from 'react';
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  Alert,
  ActivityIndicator,
  SafeAreaView,
} from 'react-native';
import {generateUserId, generateUserName} from '../utils';
import {useProtocol} from '../context/ProtocolContext';
import {DEFAULT_RELAYS} from '../constants';

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
      const userId = generateUserId();
      await initialize(userId, name.trim());
      onComplete();
    } catch (error: any) {
      Alert.alert('Error', `Failed to start protocol: ${error.message}`);
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
        <Text style={styles.title}>Nostr Transport</Text>
        <Text style={styles.subtitle}>
          Offline mesh messaging{'\n'}over Nostr relays
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
              <Text style={styles.randomizeText}>Rand</Text>
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
            <Text style={styles.startButtonText}>Start Nostr Transport</Text>
          )}
        </TouchableOpacity>

        <View style={styles.relaySection}>
          <Text style={styles.relayTitle}>Relays</Text>
          {DEFAULT_RELAYS.map(relay => (
            <Text key={relay} style={styles.relayUrl}>{relay}</Text>
          ))}
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
    paddingHorizontal: 14,
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: '#E5E5E5',
  },
  randomizeText: {
    fontSize: 14,
    fontWeight: '600',
    color: '#7B1FA2',
  },
  startButton: {
    width: '100%',
    backgroundColor: '#7B1FA2',
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
  relaySection: {
    gap: 4,
    alignItems: 'center',
  },
  relayTitle: {
    fontSize: 14,
    fontWeight: '600',
    color: '#3C3C43',
    marginBottom: 4,
  },
  relayUrl: {
    fontSize: 12,
    color: '#8E8E93',
    fontFamily: 'monospace',
  },
});
