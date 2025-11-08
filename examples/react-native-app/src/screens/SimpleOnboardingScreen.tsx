import React, { useState } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  TextInput,
  Alert,
  Platform,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import LinearGradient from 'react-native-linear-gradient';
import { Icon } from '../components/Icon';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { generateUserName } from '../utils/user';

interface SimpleOnboardingScreenProps {
  onComplete: () => void;
}

export function SimpleOnboardingScreen({ onComplete }: SimpleOnboardingScreenProps) {
  const { theme } = useTheme();
  const { updateUserName, initialize, start } = useProtocol();
  const [userName, setUserName] = useState(generateUserName());
  const [isLoading, setIsLoading] = useState(false);

  const handleComplete = async () => {
    if (!userName.trim()) {
      Alert.alert('Name Required', 'Please enter a name to continue.');
      return;
    }

    setIsLoading(true);
    try {
      updateUserName(userName.trim());
      await initialize();
      await start();
      onComplete();
    } catch (error) {
      console.error('Onboarding completion failed:', error);
      Alert.alert(
        'Setup Failed',
        'Failed to initialize the app. Please check your permissions and try again.'
      );
    } finally {
      setIsLoading(false);
    }
  };

  const generateRandomName = () => {
    const newName = generateUserName();
    setUserName(newName);
  };

  return (
    <SafeAreaView style={[styles.container, { backgroundColor: theme.colors.background }]}>
      <LinearGradient
        colors={[theme.colors.primary, theme.colors.primaryDark]}
        style={styles.headerGradient}
      />
      
      <View style={styles.content}>
        {/* Icon */}
        <View style={[styles.iconContainer, { backgroundColor: theme.colors.surface }]}>
          <Icon name="chatbubbles" size={48} color={theme.colors.primary} />
        </View>

        {/* Content */}
        <View style={styles.textContent}>
          <Text style={[styles.title, { color: theme.colors.text }]}>
            Welcome to Offline Messenger
          </Text>
          <Text style={[styles.subtitle, { color: theme.colors.textSecondary }]}>
            Connect with people nearby without internet
          </Text>
          <Text style={[styles.description, { color: theme.colors.textSecondary }]}>
            Send messages directly to nearby devices using Bluetooth. No internet required.
          </Text>
        </View>

        {/* Name Input */}
        <View style={styles.nameInputContainer}>
          <Text style={[styles.inputLabel, { color: theme.colors.text }]}>
            Choose Your Name
          </Text>
          <View style={[styles.inputWrapper, { backgroundColor: theme.colors.surface }]}>
            <TextInput
              style={[styles.nameInput, { color: theme.colors.text }]}
              value={userName}
              onChangeText={setUserName}
              placeholder="Enter your name"
              placeholderTextColor={theme.colors.textSecondary}
              maxLength={20}
              autoCapitalize="words"
              returnKeyType="done"
              onSubmitEditing={handleComplete}
            />
            <TouchableOpacity 
              style={styles.randomButton}
              onPress={generateRandomName}
              activeOpacity={0.7}
            >
              <Icon name="shuffle" size={20} color={theme.colors.primary} />
            </TouchableOpacity>
          </View>
          <Text style={[styles.inputHint, { color: theme.colors.textSecondary }]}>
            This is how others will see you
          </Text>
        </View>

        {/* Spacer */}
        <View style={{ flex: 1 }} />

        {/* Get Started Button */}
        <TouchableOpacity
          style={[
            styles.getStartedButton,
            { 
              backgroundColor: theme.colors.primary,
              opacity: isLoading ? 0.7 : 1,
            },
          ]}
          onPress={handleComplete}
          disabled={isLoading}
          activeOpacity={0.8}
        >
          <Text style={[styles.getStartedButtonText, { color: theme.colors.textInverse }]}>
            {isLoading ? 'Setting up...' : 'Get Started'}
          </Text>
          {!isLoading && (
            <Icon 
              name="checkmark" 
              size={20} 
              color={theme.colors.textInverse} 
              style={{ marginLeft: 8 }}
            />
          )}
        </TouchableOpacity>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  headerGradient: {
    position: 'absolute',
    top: 0,
    left: 0,
    right: 0,
    height: 300,
    borderBottomLeftRadius: 30,
    borderBottomRightRadius: 30,
  },
  content: {
    flex: 1,
    paddingHorizontal: 24,
    paddingTop: 80,
  },
  iconContainer: {
    width: 96,
    height: 96,
    borderRadius: 48,
    alignSelf: 'center',
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 32,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 4 },
        shadowOpacity: 0.1,
        shadowRadius: 8,
      },
      android: {
        elevation: 4,
      },
    }),
  },
  textContent: {
    alignItems: 'center',
    marginBottom: 40,
  },
  title: {
    fontSize: 28,
    fontWeight: '700',
    textAlign: 'center',
    marginBottom: 8,
    lineHeight: 36,
  },
  subtitle: {
    fontSize: 18,
    fontWeight: '500',
    textAlign: 'center',
    marginBottom: 16,
    lineHeight: 24,
  },
  description: {
    fontSize: 16,
    textAlign: 'center',
    lineHeight: 24,
    paddingHorizontal: 16,
  },
  nameInputContainer: {
    marginBottom: 32,
  },
  inputLabel: {
    fontSize: 18,
    fontWeight: '600',
    marginBottom: 12,
    textAlign: 'center',
  },
  inputWrapper: {
    flexDirection: 'row',
    alignItems: 'center',
    borderRadius: 12,
    paddingHorizontal: 16,
    paddingVertical: 4,
    marginBottom: 8,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 2 },
        shadowOpacity: 0.1,
        shadowRadius: 4,
      },
      android: {
        elevation: 2,
      },
    }),
  },
  nameInput: {
    flex: 1,
    fontSize: 18,
    fontWeight: '500',
    paddingVertical: 16,
    textAlign: 'center',
  },
  randomButton: {
    padding: 8,
    borderRadius: 20,
  },
  inputHint: {
    fontSize: 14,
    textAlign: 'center',
    fontStyle: 'italic',
  },
  getStartedButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 16,
    paddingHorizontal: 32,
    borderRadius: 25,
    marginBottom: 32,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 4 },
        shadowOpacity: 0.2,
        shadowRadius: 8,
      },
      android: {
        elevation: 4,
      },
    }),
  },
  getStartedButtonText: {
    fontSize: 18,
    fontWeight: '600',
  },
});
