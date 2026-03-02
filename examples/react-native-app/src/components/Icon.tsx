import React from 'react';
import { Text, StyleSheet } from 'react-native';

interface IconProps {
  name: string;
  size?: number;
  color?: string;
  style?: any;
}

// Simple icon mapping using Unicode symbols and emojis
const ICON_MAP: Record<string, string> = {
  // Navigation
  'chatbubbles': '💬',
  'chatbubbles-outline': '💬',
  'people': '👥',
  'people-outline': '👥',
  'analytics': '📊',
  'analytics-outline': '📊',
  'settings': '⚙️',
  'settings-outline': '⚙️',
  'chevron-forward': '›',
  'chevron-back': '‹',
  'arrow-forward': '→',
  'arrow-back': '←',
  
  // Communication
  'chatbubble': '💬',
  'send': '📤',
  'mail': '📧',
  'call': '📞',
  'videocam': '📹',
  
  // Status & Connection
  'wifi': '📶',
  'radio-button-on': '🟢',
  'radio-button-off': '⚪',
  'ellipse-outline': '⚫',
  'checkmark': '✓',
  'checkmark-done': '✓✓',
  'time-outline': '⏰',
  'alert-circle-outline': '⚠️',
  
  // User & Profile
  'person': '👤',
  'person-add': '👤+',
  'person-remove': '👤-',
  'shield-checkmark': '🛡️',
  'pencil': '✏️',
  'close-circle': '✕',
  'search': '🔍',
  'lock-closed': '🔒',
  'lock-open': '🔓',
  'alert-circle': '⚠️',
  
  // Actions
  'play-circle': '▶️',
  'stop-circle': '⏹️',
  'refresh': '🔄',
  'shuffle': '🔀',
  'information-circle': 'ℹ️',
  'help-circle': '❓',
  'notifications': '🔔',
  'volume-high': '🔊',
  'color-palette': '🎨',
  
  // Network & Tech
  'git-network': '🌐',
  'trending-up': '📈',
  'trending-down': '📉',
  'remove': '−',
  'flash': '⚡',
  'ellipsis-horizontal': '⋯',
  'location': '📍',
  'time': '⏱️',
  
  // Media & Attachments
  'attach': '📎',
  'image': '🖼️',
  'mic': '🎙️',
  'document': '📄',
  'film': '🎬',
  'musical-note': '🎵',
  'camera': '📷',
  'close': '✕',

  // Default fallback
  'default': '•',
};

export function Icon({ name, size = 20, color = '#000', style }: IconProps) {
  const iconChar = ICON_MAP[name] || ICON_MAP['default'];
  
  return (
    <Text
      style={[
        styles.icon,
        {
          fontSize: size,
          color,
          lineHeight: size * 1.2,
        },
        style,
      ]}
    >
      {iconChar}
    </Text>
  );
}

const styles = StyleSheet.create({
  icon: {
    textAlign: 'center',
    includeFontPadding: false,
  },
});
