import React from 'react';
import {View, Text, StyleSheet} from 'react-native';
import {formatRelativeTime} from '../utils';
import {NEARBY_THRESHOLD_MS} from '../constants';

interface PresenceIndicatorProps {
  isNearby: boolean;
  lastSeen: number;
  compact?: boolean;
}

export function PresenceIndicator({isNearby, lastSeen, compact}: PresenceIndicatorProps) {
  const now = Date.now();
  const isRecent = now - lastSeen < NEARBY_THRESHOLD_MS;
  const isOnline = isNearby && isRecent;

  return (
    <View style={styles.container}>
      <View style={[styles.dot, isOnline ? styles.dotOnline : styles.dotOffline]} />
      {!compact && (
        <Text style={[styles.text, isOnline ? styles.textOnline : styles.textOffline]}>
          {isOnline ? 'Nearby' : lastSeen > 0 ? formatRelativeTime(lastSeen) : 'Offline'}
        </Text>
      )}
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
  dotOffline: {
    backgroundColor: '#C7C7CC',
  },
  text: {
    fontSize: 12,
  },
  textOnline: {
    color: '#34C759',
  },
  textOffline: {
    color: '#8E8E93',
  },
});
