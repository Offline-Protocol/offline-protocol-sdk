import React, { useMemo } from 'react';
import { View, Text, StyleSheet } from 'react-native';

interface StatusBarProps {
  isStarted: boolean;
  error: string | null;
  currentTransport?: string | null;
  pendingMessages?: number;
  neighborCount?: number;
  activeTransports?: string[];
  relayPriority?: string | null;
  batteryLevel?: number | null;
}

export function StatusBar({
  isStarted,
  error,
  currentTransport,
  pendingMessages,
  neighborCount,
  activeTransports,
  relayPriority,
  batteryLevel,
}: StatusBarProps) {
  const statusLabel = error ? `Error: ${error}` : isStarted ? 'Protocol Started' : 'Protocol Stopped';
  const formattedTransport = useMemo(() => {
    if (currentTransport) {
      return formatTransportName(currentTransport);
    }
    if (activeTransports && activeTransports.length > 0) {
      return activeTransports.map(formatTransportName).join(', ');
    }
    return 'Automatic routing';
  }, [activeTransports, currentTransport]);
  const relayLabel = relayPriority ? relayPriority.toUpperCase() : 'AUTO';
  const batteryLabel = typeof batteryLevel === 'number' ? `${batteryLevel}%` : '—';

  const detailLine =
    error || !isStarted
      ? null
      : `Routing • ${formattedTransport}   Relay • ${relayLabel}   Battery • ${batteryLabel}   Pending • ${pendingMessages ?? 0}   Neighbors • ${neighborCount ?? 0}`;

  return (
    <View
      style={[
        styles.container,
        error ? styles.error : isStarted ? styles.started : styles.stopped,
      ]}
    >
      <View
        style={[
          styles.indicator,
          error ? styles.errorIndicator : isStarted ? styles.startedIndicator : styles.stoppedIndicator,
        ]}
      />
      <View style={styles.textColumn}>
        <Text style={styles.title}>{statusLabel}</Text>
        {detailLine ? <Text style={styles.subtitle}>{detailLine}</Text> : null}
      </View>
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
  textColumn: {
    flex: 1,
  },
  title: {
    fontSize: 14,
    fontWeight: '500',
    color: '#333',
  },
  subtitle: {
    marginTop: 2,
    fontSize: 12,
    color: '#475569',
  },
});

function formatTransportName(raw: string): string {
  const lower = raw.toLowerCase();
  if (lower === 'ble') {
    return 'BLE';
  }
  if (lower === 'wifidirect' || lower === 'wifi_direct') {
    return 'Wi-Fi Direct';
  }
  if (lower === 'internet') {
    return 'Internet';
  }
  return raw;
}

