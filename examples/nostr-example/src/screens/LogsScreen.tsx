import React, {useRef} from 'react';
import {View, Text, ScrollView, TouchableOpacity, StyleSheet} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {formatMessageTime} from '../utils';
import type {LogEntry} from '../types';

const LEVEL_COLORS: Record<LogEntry['level'], string> = {
  info: '#2196F3',
  warning: '#FF9800',
  error: '#F44336',
  debug: '#9E9E9E',
};

export function LogsScreen() {
  const {logs, isTransportEnabled, toggleTransport, stop, isStarted} = useProtocol();
  const scrollRef = useRef<ScrollView>(null);

  return (
    <View style={styles.container}>
      {/* Controls */}
      <View style={styles.controls}>
        {isStarted && (
          <TouchableOpacity
            style={[styles.button, isTransportEnabled ? styles.disableButton : styles.enableButton]}
            onPress={toggleTransport}>
            <Text style={styles.buttonText}>
              {isTransportEnabled ? 'Disable Nostr' : 'Enable Nostr'}
            </Text>
          </TouchableOpacity>
        )}
        {isStarted && (
          <TouchableOpacity style={[styles.button, styles.stopButton]} onPress={stop}>
            <Text style={styles.buttonText}>Stop</Text>
          </TouchableOpacity>
        )}
      </View>

      {/* Log Entries */}
      <ScrollView
        ref={scrollRef}
        style={styles.logList}
        onContentSizeChange={() => scrollRef.current?.scrollToEnd({animated: false})}>
        {logs.length === 0 ? (
          <Text style={styles.emptyText}>No log entries yet.</Text>
        ) : (
          logs.map(log => (
            <View key={log.id} style={styles.logEntry}>
              <Text style={styles.logTime}>
                {formatMessageTime(log.timestamp)}
              </Text>
              <Text style={[styles.logLevel, {color: LEVEL_COLORS[log.level]}]}>
                [{log.level.toUpperCase()}]
              </Text>
              <Text style={styles.logMessage}>{log.message}</Text>
            </View>
          ))
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  controls: {
    flexDirection: 'row',
    padding: 12,
    gap: 8,
    backgroundColor: '#FFFFFF',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  button: {
    flex: 1,
    paddingVertical: 10,
    borderRadius: 8,
    alignItems: 'center',
  },
  enableButton: {
    backgroundColor: '#2196F3',
  },
  disableButton: {
    backgroundColor: '#FF9800',
  },
  stopButton: {
    backgroundColor: '#F44336',
  },
  buttonText: {
    color: '#FFFFFF',
    fontWeight: '600',
    fontSize: 14,
  },
  logList: {
    flex: 1,
    padding: 12,
  },
  logEntry: {
    flexDirection: 'row',
    paddingVertical: 3,
    gap: 6,
  },
  logTime: {
    fontSize: 11,
    fontFamily: 'monospace',
    color: '#8E8E93',
  },
  logLevel: {
    fontSize: 11,
    fontFamily: 'monospace',
    fontWeight: '600',
  },
  logMessage: {
    fontSize: 12,
    fontFamily: 'monospace',
    flex: 1,
    color: '#333333',
  },
  emptyText: {
    textAlign: 'center',
    color: '#8E8E93',
    marginTop: 40,
    fontSize: 14,
  },
});
