import React, { useState, useRef } from 'react';
import {
  View,
  Text,
  StyleSheet,
  Dimensions,
  TouchableOpacity,
  TextInput,
  Alert,
  Platform,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
// import Animated, { FadeInDown } from 'react-native-reanimated';
import LinearGradient from 'react-native-linear-gradient';
import { Icon } from '../components/Icon';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { generateUserName } from '../utils/user';

const { width, height } = Dimensions.get('window');

interface OnboardingScreenProps {
  onComplete: () => void;
}

const onboardingSteps = [
  {
    id: 'welcome',
    title: 'Welcome to Offline Messenger',
    subtitle: 'Connect with people nearby without internet',
    icon: 'chatbubbles',
    description: 'Send messages directly to nearby devices using Bluetooth and WiFi Direct. No internet or cellular connection required.',
  },
  {
    id: 'privacy',
    title: 'Privacy First',
    subtitle: 'Your messages stay on your device',
    icon: 'shield-checkmark',
    description: 'All messages are encrypted and transmitted directly between devices. No servers, no tracking, no data collection.',
  },
  {
    id: 'network',
    title: 'Mesh Networking',
    subtitle: 'Messages can hop through multiple devices',
    icon: 'git-network',
    description: 'If someone is out of range, your message can travel through other nearby devices to reach them.',
  },
  {
    id: 'setup',
    title: 'Choose Your Name',
    subtitle: 'How others will see you',
    icon: 'person',
    description: 'Pick a name that others will see when you connect. You can change this later in settings.',
  },
];

export function OnboardingScreen({ onComplete }: OnboardingScreenProps) {
  const { theme } = useTheme();
  const { updateUserName, initialize, start, isInitialized } = useProtocol();
  const [currentStep, setCurrentStep] = useState(0);
  const [userName, setUserName] = useState(generateUserName());
  const [isLoading, setIsLoading] = useState(false);
  
  const nameInputRef = useRef<TextInput>(null);

  const handleNext = () => {
    if (currentStep < onboardingSteps.length - 1) {
      setCurrentStep(currentStep + 1);
    } else {
      handleComplete();
    }
  };

  const handleComplete = async () => {
    if (!userName.trim()) {
      Alert.alert('Name Required', 'Please enter a name to continue.');
      return;
    }

    setIsLoading(true);
    try {
      console.log('🔄 Starting onboarding completion...');
      updateUserName(userName.trim());
      
      let initSucceeded = true;
      if (!isInitialized) {
        console.log('🔄 Initializing protocol...');
        initSucceeded = await initialize();
        console.log('✅ Initialize result:', initSucceeded);
      }

      if (!initSucceeded) {
        console.log('❌ Initialization failed');
        return;
      }

      console.log('🔄 Starting protocol...');
      await start();
      console.log('✅ Protocol started successfully');
      onComplete();
    } catch (error) {
      console.error('❌ Onboarding completion failed:', error);
      
      // Show more detailed error information
      const errorMessage = error instanceof Error ? error.message : String(error);
      const stackTrace = error instanceof Error ? error.stack : '';
      
      Alert.alert(
        'Setup Failed',
        `Error: ${errorMessage}\n\nPlease check the console for more details and try again.`,
        [
          {
            text: 'Show Details',
            onPress: () => {
              Alert.alert('Error Details', `${errorMessage}\n\nStack: ${stackTrace}`);
            }
          },
          {
            text: 'OK',
            style: 'default'
          }
        ]
      );
    } finally {
      setIsLoading(false);
    }
  };

  const generateRandomName = () => {
    const newName = generateUserName();
    setUserName(newName);
  };

  const currentStepData = onboardingSteps[currentStep];
  const isLastStep = currentStep === onboardingSteps.length - 1;

  return (
    <SafeAreaView style={[styles.container, { backgroundColor: theme.colors.background }]}>
      <LinearGradient
        colors={[theme.colors.primary, theme.colors.primaryDark]}
        style={styles.headerGradient}
      />
      
      <View style={styles.content}>
        {/* Progress Indicator */}
        <View style={styles.progressContainer}>
          {onboardingSteps.map((_, index) => (
            <View
              key={index}
              style={[
                styles.progressDot,
                {
                  backgroundColor: index <= currentStep 
                    ? theme.colors.primary 
                    : theme.colors.border,
                },
              ]}
            />
          ))}
        </View>

        {/* Icon */}
        <View style={[styles.iconContainer, { backgroundColor: theme.colors.surface }]}>
          <Icon 
            name={currentStepData.icon} 
            size={48} 
            color={theme.colors.primary} 
          />
        </View>

        {/* Content */}
        <View style={styles.textContent}>
          <Text style={[styles.title, { color: theme.colors.text }]}>
            {currentStepData.title}
          </Text>
          <Text style={[styles.subtitle, { color: theme.colors.textSecondary }]}>
            {currentStepData.subtitle}
          </Text>
          <Text style={[styles.description, { color: theme.colors.textSecondary }]}>
            {currentStepData.description}
          </Text>
        </View>

        {/* Name Input (only on last step) */}
        {isLastStep && (
          <View style={styles.nameInputContainer}>
            <View style={[styles.inputWrapper, { backgroundColor: theme.colors.surface }]}>
              <TextInput
                ref={nameInputRef}
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
              Tap the shuffle icon to generate a random name
            </Text>
          </View>
        )}

        {/* Spacer */}
        <View style={{ flex: 1 }} />

        {/* Actions */}
        <View style={styles.actionContainer}>
          <TouchableOpacity
            style={[
              styles.nextButton,
              { 
                backgroundColor: theme.colors.primary,
                opacity: isLoading ? 0.7 : 1,
              },
            ]}
            onPress={handleNext}
            disabled={isLoading}
            activeOpacity={0.8}
          >
            <Text style={[styles.nextButtonText, { color: theme.colors.textInverse }]}>
              {isLoading ? 'Setting up...' : isLastStep ? 'Get Started' : 'Next'}
            </Text>
            {!isLoading && (
              <Icon 
                name={isLastStep ? 'checkmark' : 'arrow-forward'} 
                size={20} 
                color={theme.colors.textInverse} 
                style={{ marginLeft: 8 }}
              />
            )}
          </TouchableOpacity>

          {!isLastStep && (
            <TouchableOpacity
              style={styles.skipButton}
              onPress={() => setCurrentStep(onboardingSteps.length - 1)}
              activeOpacity={0.7}
            >
              <Text style={[styles.skipButtonText, { color: theme.colors.textSecondary }]}>
                Skip
              </Text>
            </TouchableOpacity>
          )}
        </View>
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
    height: height * 0.3,
    borderBottomLeftRadius: 30,
    borderBottomRightRadius: 30,
  },
  content: {
    flex: 1,
    paddingHorizontal: 24,
    paddingTop: 60,
  },
  progressContainer: {
    flexDirection: 'row',
    justifyContent: 'center',
    marginBottom: 40,
    gap: 8,
  },
  progressDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
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
    marginBottom: 32,
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
  inputWrapper: {
    flexDirection: 'row',
    alignItems: 'center',
    borderRadius: 12,
    paddingHorizontal: 16,
    paddingVertical: 4,
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
    marginTop: 8,
    fontStyle: 'italic',
  },
  actionContainer: {
    paddingBottom: 32,
  },
  nextButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 16,
    paddingHorizontal: 32,
    borderRadius: 25,
    marginBottom: 16,
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
  nextButtonText: {
    fontSize: 18,
    fontWeight: '600',
  },
  skipButton: {
    alignSelf: 'center',
    paddingVertical: 12,
    paddingHorizontal: 24,
  },
  skipButtonText: {
    fontSize: 16,
    fontWeight: '500',
  },
});
