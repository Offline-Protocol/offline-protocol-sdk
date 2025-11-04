import React from 'react';
import { View, Text, StyleSheet } from 'react-native';

interface StatusBarProps {
  isStarted: boolean;
  error: string | null;
}

export function StatusBar({ isStarted, error }: StatusBarProps) {
  return (
    <View style={[styles.container, error ? styles.error : isStarted ? styles.started : styles.stopped]}>
      <View style={[styles.indicator, error ? styles.errorIndicator : isStarted ? styles.startedIndicator : styles.stoppedIndicator]} />
      <Text style={styles.text}>
        {error ? `Error: ${error}` : isStarted ? 'Protocol Started' : 'Protocol Stopped'}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 12,
    borderRadius: 8,
    marginBottom: 16,
  },
  started: {
    backgroundColor: '#e6f7e6',
  },
  stopped: {
    backgroundColor: '#f5f5f5',
  },
  error: {
    backgroundColor: '#ffe6e6',
  },
  indicator: {
    width: 12,
    height: 12,
    borderRadius: 6,
    marginRight: 8,
  },
  startedIndicator: {
    backgroundColor: '#4caf50',
  },
  stoppedIndicator: {
    backgroundColor: '#9e9e9e',
  },
  errorIndicator: {
    backgroundColor: '#f44336',
  },
  text: {
    fontSize: 14,
    fontWeight: '500',
    color: '#333',
  },
});

