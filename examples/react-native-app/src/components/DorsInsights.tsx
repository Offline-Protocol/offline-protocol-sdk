import React, { useMemo } from 'react';
import { View, Text, StyleSheet } from 'react-native';
import type { DorsMetrics, MessageMetrics, TransportHealth } from '../utils/deriveInsights';

interface DorsInsightsProps {
  dors: DorsMetrics;
  messages: MessageMetrics;
  variant?: 'default' | 'compact';
}

const TRANSPORT_THEME: Record<string, { label: string; color: string }> = {
  BLE: { label: 'BLE Mesh', color: '#1d4ed8' },
  WiFiDirect: { label: 'Wi-Fi Direct', color: '#ea580c' },
  Internet: { label: 'Internet', color: '#16a34a' },
};

function getTransportTheme(transport: string | null) {
  if (!transport) {
    return { label: 'Auto', color: '#6366f1' };
  }
  return TRANSPORT_THEME[transport] ?? { label: transport, color: '#0ea5e9' };
}

function formatPercent(value: number | null): string {
  if (value === null || Number.isNaN(value)) {
    return '—';
  }
  return `${Math.round(value * 100)}%`;
}

function formatTime(at: number | null): string {
  if (!at) {
    return 'No recent switch';
  }
  const date = new Date(at);
  return `${date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
}

function renderTransportHealth(health: Record<string, TransportHealth>) {
  const entries = Object.entries(health);
  if (entries.length === 0) {
    return (
      <Text style={styles.healthEmpty}>No transport usage yet — send a message to see live stats.</Text>
    );
  }

  return entries.slice(0, 3).map(([transport, metrics]) => {
    const theme = getTransportTheme(transport);
    return (
      <View key={transport} style={styles.healthRow}>
        <View style={[styles.healthDot, { backgroundColor: theme.color }]} />
        <View style={styles.healthColumn}>
          <Text style={styles.healthTransport}>{theme.label}</Text>
          <Text style={styles.healthStats}>
            {`Delivered ${metrics.delivered} • Received ${metrics.received}`}
          </Text>
        </View>
        <Text style={styles.healthTimestamp}>
          {metrics.lastSeenAt ? formatTime(metrics.lastSeenAt) : '—'}
        </Text>
      </View>
    );
  });
}

export function DorsInsights({ dors, messages, variant = 'default' }: DorsInsightsProps) {
  const isCompact = variant === 'compact';
  const theme = useMemo(() => getTransportTheme(dors.currentTransport), [dors.currentTransport]);
  const lastSwitchReason = dors.lastSwitch?.reason ?? 'Monitoring network conditions';
  const lastSwitchTime = dors.lastSwitch ? formatTime(dors.lastSwitch.at) : '—';
  const topTransportEntry = useMemo(() => {
    const entries = Object.entries(dors.transportHealth);
    if (entries.length === 0) {
      return null;
    }
    if (dors.currentTransport && dors.transportHealth[dors.currentTransport]) {
      return [dors.currentTransport, dors.transportHealth[dors.currentTransport]] as const;
    }
    return entries[0] as [string, TransportHealth];
  }, [dors.currentTransport, dors.transportHealth]);

  if (isCompact) {
    return (
      <View style={[styles.card, styles.cardCompact]}>
        <View style={styles.compactHeaderRow}>
          <View style={[styles.compactBadge, { borderColor: theme.color, backgroundColor: `${theme.color}1a` }]}>
            <View style={[styles.dot, styles.compactDot, { backgroundColor: theme.color }]} />
            <Text style={[styles.compactBadgeText, { color: theme.color }]}>{theme.label}</Text>
          </View>
          <View style={styles.compactHeadlineColumn}>
            <Text style={styles.compactTitle}>DORS is optimizing delivery</Text>
            <Text style={styles.compactSubTitle}>
              {`Success ${formatPercent(messages.successRate)} • Pending ${messages.pending}`}
            </Text>
          </View>
          <Text style={styles.compactLatency}>
            {messages.averageLatencyMs !== null ? `${Math.round(messages.averageLatencyMs)} ms` : '—'}
          </Text>
        </View>
        <View style={styles.compactSwitchRow}>
          <Text style={styles.compactSwitchText} numberOfLines={2}>
            {lastSwitchReason}
          </Text>
          <Text style={styles.compactTimestamp}>{lastSwitchTime}</Text>
        </View>
        <Text style={styles.compactFootnote} numberOfLines={1}>
          {topTransportEntry
            ? `${getTransportTheme(topTransportEntry[0]).label} delivered ${
                topTransportEntry[1].delivered
              } • ${topTransportEntry[1].lastSeenAt ? formatTime(topTransportEntry[1].lastSeenAt) : '—'}`
            : 'Send a message to see transport health'}
        </Text>
      </View>
    );
  }

  return (
    <View style={styles.card}>
      <View style={styles.headerRow}>
        <Text style={styles.title}>Dynamic Offline Relay Switch</Text>
        <View style={[styles.badge, { backgroundColor: `${theme.color}1a`, borderColor: theme.color }]}>
          <View style={[styles.dot, { backgroundColor: theme.color }]} />
          <Text style={[styles.badgeText, { color: theme.color }]}>{theme.label}</Text>
        </View>
      </View>

      <View style={styles.metricsRow}>
        <View style={styles.metric}>
          <Text style={styles.metricLabel}>Success rate</Text>
          <Text style={styles.metricValue}>{formatPercent(messages.successRate)}</Text>
        </View>
        <View style={styles.metric}>
          <Text style={styles.metricLabel}>Pending</Text>
          <Text style={styles.metricValue}>{messages.pending}</Text>
        </View>
        <View style={styles.metric}>
          <Text style={styles.metricLabel}>Avg latency</Text>
          <Text style={styles.metricValue}>
            {messages.averageLatencyMs !== null ? `${Math.round(messages.averageLatencyMs)} ms` : '—'}
          </Text>
        </View>
      </View>

      <View style={styles.switchInfo}>
        <Text style={styles.switchLabel}>Last decision</Text>
        <Text style={styles.switchText}>{lastSwitchReason}</Text>
        <Text style={styles.switchTimestamp}>{lastSwitchTime}</Text>
      </View>

      <View style={styles.healthContainer}>
        <Text style={styles.healthLabel}>Transport health</Text>
        {renderTransportHealth(dors.transportHealth)}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    backgroundColor: '#0f172a',
    borderRadius: 20,
    padding: 18,
    gap: 14,
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: 6 },
    shadowOpacity: 0.25,
    shadowRadius: 14,
    elevation: 6,
  },
  cardCompact: {
    paddingHorizontal: 14,
    paddingVertical: 12,
    gap: 10,
  },
  headerRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  title: {
    flex: 1,
    color: '#e2e8f0',
    fontSize: 16,
    fontWeight: '700',
    marginRight: 12,
  },
  badge: {
    flexDirection: 'row',
    alignItems: 'center',
    borderWidth: 1,
    borderRadius: 999,
    paddingHorizontal: 12,
    paddingVertical: 4,
    gap: 6,
  },
  dot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  badgeText: {
    fontSize: 12,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  compactHeaderRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
  },
  compactBadge: {
    flexDirection: 'row',
    alignItems: 'center',
    borderWidth: 1,
    borderRadius: 999,
    paddingHorizontal: 10,
    paddingVertical: 4,
    gap: 6,
  },
  compactDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
  compactBadgeText: {
    fontSize: 11,
    fontWeight: '700',
    textTransform: 'uppercase',
    letterSpacing: 0.6,
  },
  compactHeadlineColumn: {
    flex: 1,
    gap: 2,
  },
  compactTitle: {
    color: '#e2e8f0',
    fontSize: 13,
    fontWeight: '700',
  },
  compactSubTitle: {
    color: '#94a3b8',
    fontSize: 11,
  },
  compactLatency: {
    color: '#f8fafc',
    fontSize: 13,
    fontWeight: '700',
  },
  compactSwitchRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    justifyContent: 'space-between',
    gap: 12,
  },
  compactSwitchText: {
    flex: 1,
    color: '#e2e8f0',
    fontSize: 12,
    lineHeight: 16,
    fontWeight: '600',
  },
  compactTimestamp: {
    color: '#cbd5f5',
    fontSize: 11,
    minWidth: 60,
    textAlign: 'right',
  },
  compactFootnote: {
    color: '#94a3b8',
    fontSize: 11,
  },
  metricsRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
  },
  metric: {
    flex: 1,
    paddingRight: 12,
  },
  metricLabel: {
    color: '#94a3b8',
    fontSize: 12,
    marginBottom: 4,
  },
  metricValue: {
    color: '#f8fafc',
    fontSize: 18,
    fontWeight: '700',
  },
  switchInfo: {
    backgroundColor: '#1e293b',
    borderRadius: 14,
    padding: 14,
    gap: 6,
  },
  switchLabel: {
    color: '#94a3b8',
    fontSize: 11,
    textTransform: 'uppercase',
    letterSpacing: 0.8,
  },
  switchText: {
    color: '#e2e8f0',
    fontSize: 13,
    lineHeight: 18,
    fontWeight: '600',
  },
  switchTimestamp: {
    color: '#cbd5f5',
    fontSize: 12,
  },
  healthContainer: {
    backgroundColor: '#111827',
    borderRadius: 14,
    padding: 14,
    gap: 12,
  },
  healthLabel: {
    color: '#94a3b8',
    fontSize: 12,
    textTransform: 'uppercase',
    letterSpacing: 0.8,
  },
  healthRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
  },
  healthColumn: {
    flex: 1,
  },
  healthDot: {
    width: 10,
    height: 10,
    borderRadius: 5,
  },
  healthTransport: {
    color: '#f8fafc',
    fontSize: 13,
    fontWeight: '600',
  },
  healthStats: {
    color: '#94a3b8',
    fontSize: 12,
    marginTop: 2,
  },
  healthTimestamp: {
    color: '#cbd5f5',
    fontSize: 11,
  },
  healthEmpty: {
    color: '#94a3b8',
    fontSize: 12,
  },
});

