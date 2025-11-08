import { Dimensions, Platform } from 'react-native';

const { width, height } = Dimensions.get('window');

// Breakpoints based on common device sizes
export const BREAKPOINTS = {
  xs: 0,
  sm: 576,
  md: 768,
  lg: 992,
  xl: 1200,
} as const;

export type Breakpoint = keyof typeof BREAKPOINTS;

// Device type detection
export const isTablet = width >= BREAKPOINTS.md;
export const isLargeTablet = width >= BREAKPOINTS.lg;
export const isSmallDevice = width < BREAKPOINTS.sm;
export const isLandscape = width > height;

// Responsive utilities
export function getResponsiveValue<T>(values: Partial<Record<Breakpoint, T>>, fallback: T): T {
  const sortedBreakpoints = (Object.keys(BREAKPOINTS) as Breakpoint[])
    .sort((a, b) => BREAKPOINTS[b] - BREAKPOINTS[a]); // Sort descending

  for (const breakpoint of sortedBreakpoints) {
    if (width >= BREAKPOINTS[breakpoint] && values[breakpoint] !== undefined) {
      return values[breakpoint]!;
    }
  }

  return fallback;
}

// Responsive spacing
export function getResponsiveSpacing(base: number): number {
  if (isTablet) {
    return base * 1.5;
  }
  if (isSmallDevice) {
    return base * 0.8;
  }
  return base;
}

// Responsive font size
export function getResponsiveFontSize(base: number): number {
  if (isTablet) {
    return base * 1.2;
  }
  if (isSmallDevice) {
    return base * 0.9;
  }
  return base;
}

// Grid utilities
export function getGridColumns(): number {
  return getResponsiveValue({
    xs: 1,
    sm: 2,
    md: 3,
    lg: 4,
    xl: 5,
  }, 2);
}

// Safe area utilities
export function getSafeAreaPadding(): {
  paddingTop: number;
  paddingBottom: number;
} {
  const base = Platform.OS === 'ios' ? 20 : 10;
  
  return {
    paddingTop: isTablet ? base * 1.5 : base,
    paddingBottom: isTablet ? base * 1.5 : base,
  };
}

// Dynamic dimensions
export const SCREEN_WIDTH = width;
export const SCREEN_HEIGHT = height;

// Common responsive sizes
export const RESPONSIVE_SIZES = {
  headerHeight: getResponsiveValue({ xs: 60, md: 80 }, 60),
  tabBarHeight: getResponsiveValue({ xs: 60, md: 80 }, 60),
  buttonHeight: getResponsiveValue({ xs: 44, md: 52 }, 44),
  avatarSize: getResponsiveValue({ xs: 40, md: 48 }, 40),
  iconSize: getResponsiveValue({ xs: 20, md: 24 }, 20),
} as const;

// Layout helpers
export function getContainerPadding(): number {
  return getResponsiveValue({
    xs: 16,
    sm: 20,
    md: 24,
    lg: 32,
  }, 16);
}

export function getCardSpacing(): number {
  return getResponsiveValue({
    xs: 8,
    sm: 12,
    md: 16,
  }, 8);
}

// Text scaling
export function getTextScale(): number {
  if (isTablet) return 1.1;
  if (isSmallDevice) return 0.95;
  return 1;
}
