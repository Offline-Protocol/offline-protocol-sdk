import React from 'react';
import { View, StyleSheet } from 'react-native';
// import Animated, {
//   useSharedValue,
//   useAnimatedStyle,
//   withRepeat,
//   withTiming,
//   interpolate,
// } from 'react-native-reanimated';
import { useTheme } from '../hooks/useTheme';

interface LoadingSpinnerProps {
  size?: number;
  color?: string;
}

export function LoadingSpinner({ size = 24, color }: LoadingSpinnerProps) {
  const { theme } = useTheme();
  const spinnerColor = color || theme.colors.primary;

  return (
    <View style={[styles.container, { width: size, height: size }]}>
      <View
        style={[
          styles.spinner,
          {
            width: size,
            height: size,
            borderRadius: size / 2,
            borderWidth: Math.max(2, size / 12),
            borderTopColor: spinnerColor,
            borderRightColor: spinnerColor + '40',
            borderBottomColor: spinnerColor + '20',
            borderLeftColor: spinnerColor + '10',
          },
        ]}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  spinner: {
    borderStyle: 'solid',
  },
});
