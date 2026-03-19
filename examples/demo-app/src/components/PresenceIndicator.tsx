import React from 'react';
import {View, Text, StyleSheet} from 'react-native';
import {formatRelativeTime} from '../utils';
import type {PresenceStatus} from '../types';

interface PresenceIndicatorProps {
  presenceStatus: PresenceStatus;
  lastSeen: number;
  isTyping?: boolean;
  compact?: boolean;
}

export function PresenceIndicator({presenceStatus, lastSeen, isTyping, compact}: PresenceIndicatorProps) {
  if (isTyping) {
    return (
      <View style={styles.container}>
        <View style={[styles.dot, styles.dotTyping]} />
        {!compact && <Text style={styles.textTyping}>typing...</Text>}
      </View>
    );
  }

  const isOnline = presenceStatus === 'online';
  const isAway = presenceStatus === 'away';

  const dotStyle = isOnline ? styles.dotOnline : isAway ? styles.dotAway : styles.dotOffline;
  const textStyle = isOnline ? styles.textOnline : isAway ? styles.textAway : styles.textOffline;

  let label: string;
  if (isOnline) {
    label = 'Online';
  } else if (isAway) {
    label = 'Away';
  } else {
    label = lastSeen > 0 ? formatRelativeTime(lastSeen) : 'Offline';
  }

  return (
    <View style={styles.container}>
      <View style={[styles.dot, dotStyle]} />
      {!compact && <Text style={[styles.text, textStyle]}>{label}</Text>}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  dot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  dotOnline: {
    backgroundColor: '#34C759',
  },
  dotAway: {
    backgroundColor: '#FF9500',
  },
  dotOffline: {
    backgroundColor: '#C7C7CC',
  },
  dotTyping: {
    backgroundColor: '#007AFF',
  },
  text: {
    fontSize: 12,
  },
  textOnline: {
    color: '#34C759',
  },
  textAway: {
    color: '#FF9500',
  },
  textOffline: {
    color: '#8E8E93',
  },
  textTyping: {
    fontSize: 12,
    color: '#007AFF',
    fontStyle: 'italic',
  },
});
