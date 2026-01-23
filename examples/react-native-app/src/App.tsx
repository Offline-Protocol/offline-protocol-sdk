import React, { useEffect, useState } from 'react';
import { StatusBar, Platform, AppState, AppStateStatus } from 'react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';

// Screens
import { SimpleOnboardingScreen } from './screens/SimpleOnboardingScreen';
import { MainAppScreen } from './screens/MainAppScreen';

// Providers
import { ThemeProvider } from './providers/ThemeProvider';
import { ProtocolProvider } from './providers/ProtocolProvider';
import { WebSocketRelayProvider } from './providers/WebSocketRelayProvider';

// Hooks
import { useTheme } from './hooks/useTheme';
import { useProtocol } from './hooks/useProtocol';

function AppContent() {
  const [isFirstLaunch, setIsFirstLaunch] = useState<boolean | null>(null);
  const { theme } = useTheme();
  const { initialize, isInitialized } = useProtocol();

  useEffect(() => {
    // Check if this is the first launch
    // In a real app, you'd use AsyncStorage to persist this
    const checkFirstLaunch = async () => {
      // For demo purposes, we'll show onboarding every time
      // In production, check AsyncStorage for a flag
      setIsFirstLaunch(true);
    };

    checkFirstLaunch();
  }, []);

  useEffect(() => {
    // Initialize protocol when app starts
    if (!isInitialized) {
      initialize();
    }
  }, [initialize, isInitialized]);

  useEffect(() => {
    const handleAppStateChange = (nextAppState: AppStateStatus) => {
      // Handle app state changes for better UX
      if (nextAppState === 'active') {
        // App came to foreground
        console.log('App is active');
      } else if (nextAppState === 'background') {
        // App went to background
        console.log('App is in background');
      }
    };

    const subscription = AppState.addEventListener('change', handleAppStateChange);
    return () => subscription?.remove();
  }, []);

  if (isFirstLaunch === null) {
    // Loading state while checking first launch
    return null;
  }

  if (isFirstLaunch) {
    return (
      <SimpleOnboardingScreen 
        onComplete={() => setIsFirstLaunch(false)}
      />
    );
  }

  return <MainAppScreen />;
}

export default function App() {
  return (
    <SafeAreaProvider>
      <ThemeProvider>
        <ProtocolProvider>
          <WebSocketRelayProvider>
            <StatusBar
              barStyle="light-content"
              backgroundColor="#1a1a1a"
              translucent
            />
            <AppContent />
          </WebSocketRelayProvider>
        </ProtocolProvider>
      </ThemeProvider>
    </SafeAreaProvider>
  );
}