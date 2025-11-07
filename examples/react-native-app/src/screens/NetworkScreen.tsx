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
} from '@offlineprotocol/react-native';

interface NetworkScreenProps {
  events: ProtocolEvent[];
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

export function NetworkScreen({ events }: NetworkScreenProps) {
  const networkState = React.useMemo((): NetworkState => {
    const state: NetworkState = {
      currentTransport: null,
      isRelay: false,
      neighbors: new Set(),
      metrics: null,
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
  }, [events]);

  const transportHistory = React.useMemo(() => {
    return events
      .filter((e) => e.type === 'transport_switched')
      .slice(0, 10)
      .map((e) => e as TransportSwitchedEvent);
  }, [events]);

  const discoveredNeighbors = React.useMemo(() => {
    return events
      .filter((e) => e.type === 'neighbor_discovered')
      .slice(0, 10)
      .map((e) => e as NeighborDiscoveredEvent);
  }, [events]);

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
            <Text style={styles.statusLabel}>Transport:</Text>
            <Text style={styles.statusValue}>
              {networkState.currentTransport || 'None'}
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
            <Text style={styles.statusLabel}>Connected Neighbors:</Text>
            <Text style={styles.statusValue}>{networkState.neighbors.size}</Text>
          </View>
        </View>
      </View>

      {networkState.metrics && (
        <View style={styles.section}>
          <Text style={styles.sectionTitle}>Network Metrics</Text>
          
          <View style={styles.card}>
            <View style={styles.metricRow}>
              <Text style={styles.metricLabel}>Neighbor Count</Text>
              <Text style={styles.metricValue}>{networkState.metrics.neighborCount}</Text>
            </View>
            
            <View style={styles.metricRow}>
              <Text style={styles.metricLabel}>Relay Count</Text>
              <Text style={styles.metricValue}>{networkState.metrics.relayCount}</Text>
            </View>
            
            <View style={styles.metricRow}>
              <Text style={styles.metricLabel}>Delivery Ratio</Text>
              <Text style={styles.metricValue}>
                {(networkState.metrics.deliveryRatio * 100).toFixed(1)}%
              </Text>
            </View>
            
            <View style={styles.metricRow}>
              <Text style={styles.metricLabel}>Avg Latency</Text>
              <Text style={styles.metricValue}>
                {networkState.metrics.avgLatencyMs.toFixed(0)}ms
              </Text>
            </View>
          </View>
        </View>
      )}

      <View style={styles.section}>
        <Text style={styles.sectionTitle}>Transport History</Text>
        
        {transportHistory.length === 0 ? (
          <Text style={styles.emptyText}>No transport switches yet</Text>
        ) : (
          <View style={styles.card}>
            {transportHistory.map((event, index) => (
              <View key={index} style={styles.historyItem}>
                <Text style={styles.historyTransport}>
                  {event.from || 'None'} → {event.to}
                </Text>
                <Text style={styles.historyReason}>{event.reason}</Text>
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
  historyTransport: {
    fontSize: 14,
    color: '#333',
    fontWeight: '600',
    marginBottom: 4,
  },
  historyReason: {
    fontSize: 12,
    color: '#666',
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

