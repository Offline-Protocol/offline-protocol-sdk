import React from 'react';
import { View, Text, StyleSheet, ScrollView } from 'react-native';
import type {
  ProtocolEvent,
  TransportSwitchedEvent,
  RelayPromotedEvent,
  RelayDemotedEvent,
  NeighborDiscoveredEvent,
  NeighborLostEvent,
  NetworkMetricsEvent,
  DiagnosticEvent,
} from '@offline-protocol/mesh-sdk';
import type { DerivedInsights } from '../utils/deriveInsights';

const TRANSPORT_THEME: Record<string, { background: string; border: string; text: string }> = {
  BLE: { background: '#dbeafe', border: '#1d4ed8', text: '#1d4ed8' },
  WiFiDirect: { background: '#ffedd5', border: '#c2410c', text: '#c2410c' },
  Internet: { background: '#dcfce7', border: '#15803d', text: '#15803d' },
};

function getTransportTheme(transport: string) {
  return TRANSPORT_THEME[transport] ?? { background: '#e2e8f0', border: '#cbd5f5', text: '#0f172a' };
}

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

interface NetworkScreenProps {
  events: ProtocolEvent[];
  insights: DerivedInsights;
}

interface NetworkState {
  currentTransport: string | null;
  isRelay: boolean;
  neighbors: Set<string>;
  metrics: {
    neighborCount: number;
    relayCount: number;
    deliveryRatio: number;
    avgLatencyMs: number;
  } | null;
}

export function NetworkScreen({ events, insights }: NetworkScreenProps) {
  const dorsMetrics = insights.dorsMetrics;
  const neighborMetrics = insights.neighborMetrics;
  const networkSummary = insights.networkSummary;
  const messageMetrics = insights.messageMetrics;
  const networkState = React.useMemo((): NetworkState => {
    const state: NetworkState = {
      currentTransport: dorsMetrics.currentTransport,
      isRelay: false,
      neighbors: new Set(neighborMetrics.peers),
      metrics:
        networkSummary.neighborCount !== null &&
        networkSummary.relayCount !== null &&
        networkSummary.deliveryRatio !== null &&
        networkSummary.avgLatencyMs !== null
          ? {
              neighborCount: networkSummary.neighborCount,
              relayCount: networkSummary.relayCount,
              deliveryRatio: networkSummary.deliveryRatio,
              avgLatencyMs: networkSummary.avgLatencyMs,
            }
          : null,
    };

    events.forEach((event) => {
      switch (event.type) {
        case 'transport_switched':
          state.currentTransport = (event as TransportSwitchedEvent).to;
          break;
        case 'relay_promoted':
          state.isRelay = true;
          break;
        case 'relay_demoted':
          state.isRelay = false;
          break;
        case 'neighbor_discovered':
          const neighborEvent = event as NeighborDiscoveredEvent;
          state.neighbors.add(neighborEvent.peer_id);
          // If no explicit transport set yet, use the transport from neighbor discovery
          if (!state.currentTransport && neighborEvent.transport) {
            state.currentTransport = neighborEvent.transport;
          }
          break;
        case 'neighbor_lost':
          state.neighbors.delete((event as NeighborLostEvent).peer_id);
          break;
        case 'network_metrics':
          state.metrics = {
            neighborCount: (event as NetworkMetricsEvent).neighbor_count,
            relayCount: (event as NetworkMetricsEvent).relay_count,
            deliveryRatio: (event as NetworkMetricsEvent).delivery_ratio,
            avgLatencyMs: (event as NetworkMetricsEvent).avg_latency_ms,
          };
          break;
        case 'diagnostic': {
          const diag = event as DiagnosticEvent;
          const context = (diag.context ?? {}) as Record<string, unknown>;
          const stateValue = typeof context.state === 'string' ? context.state.toLowerCase() : undefined;

          if (diag.message === 'Starting BLE transport') {
            state.currentTransport = 'BLE (starting)';
          }

          if (diag.message === 'BLE transport state changed' && stateValue) {
            if (stateValue === 'running') {
              state.currentTransport = 'BLE';
            } else if (stateValue === 'unavailable' || stateValue === 'stopped') {
              state.currentTransport = null;
            }
          }

          if (diag.message === 'BLE transport stopped') {
            state.currentTransport = null;
          }

          if (diag.message === 'Peer discovered') {
            const peerId = typeof context.peerId === 'string' ? context.peerId : undefined;
            if (peerId) {
              state.neighbors.add(peerId);
              if (!state.currentTransport) {
                state.currentTransport = 'BLE';
              }
            }
          }

          if (diag.message === 'Disconnected from BLE peripheral') {
            const peerId = typeof context.peerId === 'string' ? context.peerId : undefined;
            if (peerId) {
              state.neighbors.delete(peerId);
            }
          }

          break;
        }
      }
    });

    return state;
  }, [dorsMetrics.currentTransport, events, neighborMetrics.peers, networkSummary.avgLatencyMs, networkSummary.deliveryRatio, networkSummary.neighborCount, networkSummary.relayCount]);

  const transportHistory = React.useMemo(
    () => dorsMetrics.switches.slice().reverse().slice(0, 10),
    [dorsMetrics.switches],
  );

  const discoveredNeighbors = React.useMemo(() => {
    return events
      .filter((e) => e.type === 'neighbor_discovered')
      .slice(0, 10)
      .map((e) => e as NeighborDiscoveredEvent);
  }, [events]);

  const transportHealthEntries = React.useMemo(
    () => Object.entries(dorsMetrics.transportHealth),
    [dorsMetrics.transportHealth],
  );

  const deliveryRateLabel =
    messageMetrics.successRate !== null ? `${Math.round(messageMetrics.successRate * 100)}%` : '—';
  const avgLatencyLabel =
    messageMetrics.averageLatencyMs !== null ? `${Math.round(messageMetrics.averageLatencyMs)} ms` : '—';
  const summaryMetrics = networkState.metrics;

  return (
    <ScrollView style={styles.container}>
      <View style={styles.section}>
        <View style={styles.helpBox}>
          <Text style={styles.helpTitle}>🌐 Auto-Discovery Active</Text>
          <Text style={styles.helpText}>
            When the protocol is running, your device automatically discovers nearby peers using Bluetooth and WiFi Direct. 
            No manual pairing needed! Share User IDs to send messages.
          </Text>
        </View>
      </View>

      <View style={styles.section}>
        <Text style={styles.sectionTitle}>Current Status</Text>
        
        <View style={styles.card}>
          <View style={styles.statusRow}>
            <Text style={styles.statusLabel}>Active Transport:</Text>
            <Text style={styles.statusValue}>
              {dorsMetrics.currentTransport ?? networkState.currentTransport ?? 'Automatic'}
            </Text>
          </View>
          
          <View style={styles.statusRow}>
            <Text style={styles.statusLabel}>Relay Status:</Text>
            <View style={[styles.badge, networkState.isRelay ? styles.badgeActive : styles.badgeInactive]}>
              <Text style={styles.badgeText}>
                {networkState.isRelay ? 'Active Relay' : 'Not Relay'}
              </Text>
            </View>
          </View>
          
          <View style={styles.statusRow}>
            <Text style={styles.statusLabel}>Neighbors:</Text>
            <Text style={styles.statusValue}>{neighborMetrics.total}</Text>
          </View>

          <View style={styles.statusRow}>
            <Text style={styles.statusLabel}>Delivery Rate:</Text>
            <Text style={styles.statusValue}>{deliveryRateLabel}</Text>
          </View>

          <View style={styles.statusRow}>
            <Text style={styles.statusLabel}>Avg Latency:</Text>
            <Text style={styles.statusValue}>{avgLatencyLabel}</Text>
          </View>

          <View style={styles.statusRow}>
            <Text style={styles.statusLabel}>Pending:</Text>
            <Text style={styles.statusValue}>{messageMetrics.pending}</Text>
          </View>

          <View style={styles.statusMiniGrid}>
            {[
              { label: 'Sent', value: messageMetrics.sent },
              { label: 'Received', value: messageMetrics.received },
              { label: 'Delivered', value: messageMetrics.delivered },
            ].map((item, index, arr) => (
              <View
                key={item.label}
                style={[
                  styles.statusMiniCard,
                  index !== arr.length - 1 && styles.statusMiniCardSpacer,
                ]}
              >
                <Text style={styles.statusMiniValue}>{item.value}</Text>
                <Text style={styles.statusMiniLabel}>{item.label}</Text>
              </View>
            ))}
          </View>

          {dorsMetrics.lastSwitch ? (
            <View style={styles.switchCard}>
              <Text style={styles.switchTitle}>Last DORS decision</Text>
              <Text style={styles.switchRoute}>
                {(dorsMetrics.lastSwitch.from ?? 'None')} → {dorsMetrics.lastSwitch.to}
              </Text>
              <Text style={styles.switchReason}>{dorsMetrics.lastSwitch.reason}</Text>
              <Text style={styles.switchTimestamp}>{formatTime(dorsMetrics.lastSwitch.at)}</Text>
            </View>
          ) : null}
        </View>
      </View>

        <View style={styles.section}>
          <Text style={styles.sectionTitle}>Network Metrics</Text>
          
          <View style={styles.card}>
            <View style={styles.metricRow}>
            <Text style={styles.metricLabel}>Live Neighbors</Text>
            <Text style={styles.metricValue}>{neighborMetrics.total}</Text>
            </View>
            
            <View style={styles.metricRow}>
              <Text style={styles.metricLabel}>Relay Count</Text>
            <Text style={styles.metricValue}>
              {summaryMetrics ? summaryMetrics.relayCount : '—'}
            </Text>
            </View>
            
            <View style={styles.metricRow}>
              <Text style={styles.metricLabel}>Delivery Ratio</Text>
              <Text style={styles.metricValue}>
              {summaryMetrics
                ? `${(summaryMetrics.deliveryRatio * 100).toFixed(1)}%`
                : deliveryRateLabel}
              </Text>
            </View>
            
            <View style={styles.metricRow}>
              <Text style={styles.metricLabel}>Avg Latency</Text>
              <Text style={styles.metricValue}>
              {summaryMetrics
                ? `${summaryMetrics.avgLatencyMs.toFixed(0)}ms`
                : avgLatencyLabel}
              </Text>
            </View>
          </View>
      </View>

      <View style={styles.section}>
        <Text style={styles.sectionTitle}>Transport Health</Text>
        
        {transportHealthEntries.length === 0 ? (
          <Text style={styles.emptyText}>No transport usage yet</Text>
        ) : (
          <View style={styles.card}>
            {transportHealthEntries.map(([transport, stats]) => {
              const theme = getTransportTheme(transport);
              return (
                <View key={transport} style={styles.healthItem}>
                  <View
                    style={[
                      styles.healthChip,
                      { backgroundColor: theme.background, borderColor: theme.border },
                    ]}
                  >
                    <Text style={[styles.healthChipText, { color: theme.text }]}>{transport}</Text>
                  </View>
                  <View style={styles.healthStatsRow}>
                    <Text style={styles.healthStat}>{`Delivered ${stats.delivered}`}</Text>
                    <Text style={styles.healthStat}>{`Received ${stats.received}`}</Text>
                    <Text style={styles.healthStat}>
                      {stats.lastSeenAt ? `Last ${formatTime(stats.lastSeenAt)}` : 'No traffic yet'}
                    </Text>
                  </View>
                </View>
              );
            })}
        </View>
      )}
      </View>

      <View style={styles.section}>
        <Text style={styles.sectionTitle}>Transport History</Text>
        
        {transportHistory.length === 0 ? (
          <Text style={styles.emptyText}>No transport switches yet</Text>
        ) : (
          <View style={styles.card}>
            {transportHistory.map((entry, index) => (
              <View key={`${entry.at}-${index}`} style={styles.historyItem}>
                <View style={styles.historyHeader}>
                <Text style={styles.historyTransport}>
                    {(entry.from ?? 'None')} → {entry.to}
                </Text>
                  <Text style={styles.historyTimestamp}>{formatTime(entry.at)}</Text>
                </View>
                <Text style={styles.historyReason}>{entry.reason}</Text>
              </View>
            ))}
          </View>
        )}
      </View>

      <View style={styles.section}>
        <Text style={styles.sectionTitle}>Discovered Neighbors</Text>
        
        {discoveredNeighbors.length === 0 ? (
          <Text style={styles.emptyText}>No neighbors discovered yet</Text>
        ) : (
          <View style={styles.card}>
            {discoveredNeighbors.map((event, index) => (
              <View key={index} style={styles.neighborItem}>
                <Text style={styles.neighborId}>{event.peer_id}</Text>
                <Text style={styles.neighborTransport}>via {event.transport}</Text>
                {event.rssi !== undefined && (
                  <Text style={styles.neighborRssi}>RSSI: {event.rssi}</Text>
                )}
              </View>
            ))}
          </View>
        )}
      </View>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  section: {
    padding: 16,
  },
  helpBox: {
    backgroundColor: '#e8f5e9',
    padding: 16,
    borderRadius: 8,
    borderLeftWidth: 4,
    borderLeftColor: '#4caf50',
  },
  helpTitle: {
    fontSize: 16,
    fontWeight: 'bold',
    color: '#2e7d32',
    marginBottom: 8,
  },
  helpText: {
    fontSize: 14,
    color: '#424242',
    lineHeight: 20,
  },
  sectionTitle: {
    fontSize: 18,
    fontWeight: 'bold',
    color: '#333',
    marginBottom: 12,
  },
  card: {
    backgroundColor: '#fff',
    borderRadius: 8,
    padding: 16,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.1,
    shadowRadius: 4,
    elevation: 2,
  },
  statusRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    paddingVertical: 8,
    borderBottomWidth: 1,
    borderBottomColor: '#f0f0f0',
  },
  statusLabel: {
    fontSize: 14,
    color: '#666',
    fontWeight: '500',
  },
  statusValue: {
    fontSize: 14,
    color: '#333',
    fontWeight: '600',
  },
  statusMiniGrid: {
    flexDirection: 'row',
    marginTop: 12,
    marginBottom: 8,
  },
  statusMiniCard: {
    flex: 1,
    backgroundColor: '#eef2ff',
    borderRadius: 12,
    paddingVertical: 8,
    alignItems: 'center',
  },
  statusMiniCardSpacer: {
    marginRight: 8,
  },
  statusMiniValue: {
    fontSize: 14,
    fontWeight: '700',
    color: '#1d4ed8',
  },
  statusMiniLabel: {
    fontSize: 11,
    color: '#475569',
    textTransform: 'uppercase',
    letterSpacing: 0.6,
    marginTop: 2,
  },
  switchCard: {
    marginTop: 12,
    backgroundColor: '#f1f5f9',
    borderRadius: 12,
    padding: 12,
    borderWidth: 1,
    borderColor: '#e2e8f0',
  },
  switchTitle: {
    fontSize: 12,
    fontWeight: '600',
    color: '#475569',
    textTransform: 'uppercase',
    letterSpacing: 0.8,
    marginBottom: 4,
  },
  switchRoute: {
    fontSize: 14,
    fontWeight: '700',
    color: '#0f172a',
  },
  switchReason: {
    fontSize: 12,
    color: '#475569',
    marginTop: 2,
  },
  switchTimestamp: {
    fontSize: 12,
    color: '#64748b',
    marginTop: 4,
  },
  badge: {
    paddingHorizontal: 12,
    paddingVertical: 4,
    borderRadius: 12,
  },
  badgeActive: {
    backgroundColor: '#4caf50',
  },
  badgeInactive: {
    backgroundColor: '#9e9e9e',
  },
  badgeText: {
    color: '#fff',
    fontSize: 12,
    fontWeight: '600',
  },
  metricRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    paddingVertical: 10,
    borderBottomWidth: 1,
    borderBottomColor: '#f0f0f0',
  },
  metricLabel: {
    fontSize: 14,
    color: '#666',
  },
  metricValue: {
    fontSize: 16,
    color: '#2196f3',
    fontWeight: '600',
  },
  emptyText: {
    textAlign: 'center',
    color: '#999',
    fontSize: 14,
    padding: 16,
  },
  historyItem: {
    paddingVertical: 10,
    borderBottomWidth: 1,
    borderBottomColor: '#f0f0f0',
  },
  historyHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 4,
  },
  historyTransport: {
    fontSize: 14,
    color: '#333',
    fontWeight: '600',
  },
  historyTimestamp: {
    fontSize: 12,
    color: '#94a3b8',
  },
  historyReason: {
    fontSize: 12,
    color: '#666',
  },
  healthItem: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingVertical: 10,
    borderBottomWidth: 1,
    borderBottomColor: '#f0f0f0',
  },
  healthChip: {
    borderWidth: 1,
    borderRadius: 999,
    paddingHorizontal: 12,
    paddingVertical: 4,
  },
  healthChipText: {
    fontSize: 12,
    fontWeight: '600',
  },
  healthStatsRow: {
    flex: 1,
    marginLeft: 12,
  },
  healthStat: {
    fontSize: 12,
    color: '#475569',
    marginBottom: 2,
  },
  neighborItem: {
    paddingVertical: 10,
    borderBottomWidth: 1,
    borderBottomColor: '#f0f0f0',
  },
  neighborId: {
    fontSize: 14,
    color: '#333',
    fontWeight: '600',
    marginBottom: 4,
  },
  neighborTransport: {
    fontSize: 12,
    color: '#666',
    marginBottom: 2,
  },
  neighborRssi: {
    fontSize: 12,
    color: '#999',
  },
});

