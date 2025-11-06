import React, { useState, useEffect } from 'react';
import { View, Text, StyleSheet, ScrollView, TouchableOpacity, RefreshControl, Dimensions } from 'react-native';
import type { OfflineProtocol } from '@offlineprotocol/react-native';
import type { NetworkTopology, NetworkNode, NetworkLink, MessageDeliveryStats } from '@offlineprotocol/react-native';

interface VisualizationScreenProps {
  protocol: OfflineProtocol | null;
  isStarted: boolean;
}

const { width } = Dimensions.get('window');

export function VisualizationScreen({ protocol, isStarted }: VisualizationScreenProps) {
  const [topology, setTopology] = useState<NetworkTopology | null>(null);
  const [messageStats, setMessageStats] = useState<MessageDeliveryStats[]>([]);
  const [successRate, setSuccessRate] = useState<number | null>(null);
  const [medianLatency, setMedianLatency] = useState<number | null>(null);
  const [medianHops, setMedianHops] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadVisualizationData = async () => {
    if (!protocol || !isStarted) {
      return;
    }

    try {
      setError(null);
      
      // Load all visualization data
      const [
        topoData,
        statsData,
        rateData,
        latencyData,
        hopsData,
      ] = await Promise.all([
        protocol.getTopology().catch(() => null),
        protocol.getMessageStats().catch(() => []),
        protocol.getDeliverySuccessRate().catch(() => null),
        protocol.getMedianLatency().catch(() => null),
        protocol.getMedianHops().catch(() => null),
      ]);

      setTopology(topoData);
      setMessageStats(statsData);
      setSuccessRate(rateData);
      setMedianLatency(latencyData);
      setMedianHops(hopsData);
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : 'Failed to load visualization data';
      setError(errorMsg);
      console.error('Failed to load visualization data:', err);
    }
  };

  useEffect(() => {
    if (isStarted) {
      loadVisualizationData();
      // Auto-refresh every 5 seconds
      const interval = setInterval(loadVisualizationData, 5000);
      return () => clearInterval(interval);
    }
  }, [protocol, isStarted]);

  const onRefresh = async () => {
    setRefreshing(true);
    await loadVisualizationData();
    setRefreshing(false);
  };

  if (!isStarted) {
    return (
      <View style={styles.emptyContainer}>
        <Text style={styles.emptyIcon}>📊</Text>
        <Text style={styles.emptyTitle}>Protocol Not Started</Text>
        <Text style={styles.emptyText}>
          Start the protocol to see network visualization and analytics
        </Text>
      </View>
    );
  }

  return (
    <ScrollView
      style={styles.container}
      refreshControl={
        <RefreshControl refreshing={refreshing} onRefresh={onRefresh} />
      }
    >
      {error && (
        <View style={styles.errorBanner}>
          <Text style={styles.errorText}>⚠️ {error}</Text>
        </View>
      )}

      {/* Key Metrics */}
      <View style={styles.section}>
        <Text style={styles.sectionTitle}>📈 Key Metrics</Text>
        <View style={styles.metricsGrid}>
          <View style={styles.metricCard}>
            <Text style={styles.metricValue}>
              {successRate !== null ? `${(successRate * 100).toFixed(1)}%` : '-'}
            </Text>
            <Text style={styles.metricLabel}>Success Rate</Text>
          </View>
          
          <View style={styles.metricCard}>
            <Text style={styles.metricValue}>
              {medianLatency !== null ? `${medianLatency}ms` : '-'}
            </Text>
            <Text style={styles.metricLabel}>Median Latency</Text>
          </View>
          
          <View style={styles.metricCard}>
            <Text style={styles.metricValue}>
              {medianHops !== null ? medianHops : '-'}
            </Text>
            <Text style={styles.metricLabel}>Median Hops</Text>
          </View>
          
          <View style={styles.metricCard}>
            <Text style={styles.metricValue}>
              {messageStats.length}
            </Text>
            <Text style={styles.metricLabel}>Total Messages</Text>
          </View>
        </View>
      </View>

      {/* Network Topology */}
      {topology && (
        <>
          <View style={styles.section}>
            <Text style={styles.sectionTitle}>🌐 Network Topology</Text>
            
            <View style={styles.card}>
              <View style={styles.topologyHeader}>
                <View style={styles.topologyInfo}>
                  <Text style={styles.topologyLabel}>Local Node</Text>
                  <Text style={styles.topologyValue}>{topology.local_user_id}</Text>
                </View>
                <View style={styles.topologyInfo}>
                  <Text style={styles.topologyLabel}>Timestamp</Text>
                  <Text style={styles.topologyValue}>
                    {new Date(topology.timestamp * 1000).toLocaleTimeString()}
                  </Text>
                </View>
              </View>

              <View style={styles.statsRow}>
                <View style={styles.statItem}>
                  <Text style={styles.statValue}>{topology.stats.total_nodes}</Text>
                  <Text style={styles.statLabel}>Nodes</Text>
                </View>
                <View style={styles.statItem}>
                  <Text style={styles.statValue}>{topology.stats.relay_nodes}</Text>
                  <Text style={styles.statLabel}>Relays</Text>
                </View>
                <View style={styles.statItem}>
                  <Text style={styles.statValue}>{topology.stats.total_connections}</Text>
                  <Text style={styles.statLabel}>Links</Text>
                </View>
                <View style={styles.statItem}>
                  <Text style={styles.statValue}>
                    {topology.stats.avg_link_quality.toFixed(2)}
                  </Text>
                  <Text style={styles.statLabel}>Avg Quality</Text>
                </View>
              </View>

              {topology.stats.network_diameter !== undefined && (
                <View style={styles.diameterInfo}>
                  <Text style={styles.diameterText}>
                    Network Diameter: {topology.stats.network_diameter} hops
                  </Text>
                </View>
              )}
            </View>
          </View>

          {/* Nodes */}
          {topology.nodes.length > 0 && (
            <View style={styles.section}>
              <Text style={styles.sectionTitle}>🔘 Network Nodes ({topology.nodes.length})</Text>
              {topology.nodes.map((node, index) => (
                <View key={index} style={styles.nodeCard}>
                  <View style={styles.nodeHeader}>
                    <Text style={styles.nodeId}>{node.user_id}</Text>
                    <View style={[
                      styles.roleBadge,
                      node.role === 'relay' ? styles.roleBadgeRelay : styles.roleBadgeNormal
                    ]}>
                      <Text style={styles.roleBadgeText}>
                        {node.role === 'relay' ? '⚡ Relay' : '👤 Normal'}
                      </Text>
                    </View>
                  </View>
                  
                  <View style={styles.nodeDetails}>
                    <View style={styles.nodeDetailItem}>
                      <Text style={styles.nodeDetailLabel}>Connections:</Text>
                      <Text style={styles.nodeDetailValue}>{node.connection_count}</Text>
                    </View>
                    {node.battery_level !== undefined && (
                      <View style={styles.nodeDetailItem}>
                        <Text style={styles.nodeDetailLabel}>Battery:</Text>
                        <Text style={styles.nodeDetailValue}>{node.battery_level}%</Text>
                      </View>
                    )}
                    <View style={styles.nodeDetailItem}>
                      <Text style={styles.nodeDetailLabel}>Transports:</Text>
                      <Text style={styles.nodeDetailValue}>
                        {node.transports.join(', ')}
                      </Text>
                    </View>
                  </View>
                </View>
              ))}
            </View>
          )}

          {/* Links */}
          {topology.links.length > 0 && (
            <View style={styles.section}>
              <Text style={styles.sectionTitle}>🔗 Network Links ({topology.links.length})</Text>
              {topology.links.map((link, index) => (
                <View key={index} style={styles.linkCard}>
                  <View style={styles.linkPath}>
                    <Text style={styles.linkNode}>{link.from}</Text>
                    <Text style={styles.linkArrow}>→</Text>
                    <Text style={styles.linkNode}>{link.to}</Text>
                  </View>
                  
                  <View style={styles.linkDetails}>
                    <View style={styles.linkQuality}>
                      <View style={styles.qualityBar}>
                        <View 
                          style={[
                            styles.qualityFill,
                            { 
                              width: `${link.quality * 100}%`,
                              backgroundColor: link.quality > 0.7 ? '#4caf50' : link.quality > 0.4 ? '#ff9800' : '#f44336'
                            }
                          ]} 
                        />
                      </View>
                      <Text style={styles.qualityText}>{(link.quality * 100).toFixed(0)}%</Text>
                    </View>
                    
                    <View style={styles.linkInfo}>
                      <Text style={styles.linkTransport}>{link.transport}</Text>
                      {link.rssi !== undefined && (
                        <Text style={styles.linkRssi}>RSSI: {link.rssi} dBm</Text>
                      )}
                    </View>
                  </View>
                </View>
              ))}
            </View>
          )}
        </>
      )}

      {/* Recent Message Stats */}
      {messageStats.length > 0 && (
        <View style={styles.section}>
          <Text style={styles.sectionTitle}>📨 Recent Messages ({messageStats.slice(0, 10).length})</Text>
          {messageStats.slice(0, 10).map((msg, index) => (
            <View key={index} style={styles.messageCard}>
              <View style={styles.messageHeader}>
                <Text style={styles.messageId} numberOfLines={1}>
                  {msg.message_id.substring(0, 8)}...
                </Text>
                <View style={[
                  styles.messageStatus,
                  msg.delivered_at ? styles.messageStatusDelivered : styles.messageStatusPending
                ]}>
                  <Text style={styles.messageStatusText}>
                    {msg.delivered_at ? '✓ Delivered' : '⏳ Pending'}
                  </Text>
                </View>
              </View>
              
              <View style={styles.messagePath}>
                <Text style={styles.messageNode}>{msg.sender}</Text>
                <Text style={styles.messageArrow}>→</Text>
                <Text style={styles.messageNode}>{msg.recipient}</Text>
              </View>
              
              <View style={styles.messageStats}>
                {msg.latency_ms && (
                  <Text style={styles.messageStat}>⏱ {msg.latency_ms}ms</Text>
                )}
                <Text style={styles.messageStat}>🔁 {msg.hop_count} hops</Text>
                {msg.retry_count > 0 && (
                  <Text style={styles.messageStat}>↻ {msg.retry_count} retries</Text>
                )}
                {msg.transport && (
                  <Text style={styles.messageStat}>📡 {msg.transport}</Text>
                )}
              </View>
            </View>
          ))}
        </View>
      )}

      {!topology && !messageStats.length && (
        <View style={styles.emptyContainer}>
          <Text style={styles.emptyIcon}>📊</Text>
          <Text style={styles.emptyTitle}>No Data Yet</Text>
          <Text style={styles.emptyText}>
            Send some messages to see network visualization
          </Text>
        </View>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  emptyContainer: {
    flex: 1,
    justifyContent: 'center',
    alignItems: 'center',
    padding: 32,
  },
  emptyIcon: {
    fontSize: 64,
    marginBottom: 16,
  },
  emptyTitle: {
    fontSize: 20,
    fontWeight: 'bold',
    color: '#333',
    marginBottom: 8,
    textAlign: 'center',
  },
  emptyText: {
    fontSize: 14,
    color: '#666',
    textAlign: 'center',
    lineHeight: 20,
  },
  errorBanner: {
    backgroundColor: '#ffebee',
    padding: 12,
    margin: 16,
    borderRadius: 8,
    borderLeftWidth: 4,
    borderLeftColor: '#f44336',
  },
  errorText: {
    color: '#c62828',
    fontSize: 14,
  },
  section: {
    padding: 16,
  },
  sectionTitle: {
    fontSize: 18,
    fontWeight: 'bold',
    color: '#333',
    marginBottom: 12,
  },
  metricsGrid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    marginHorizontal: -6,
  },
  metricCard: {
    width: (width - 48) / 2,
    margin: 6,
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 16,
    alignItems: 'center',
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.1,
    shadowRadius: 4,
    elevation: 3,
  },
  metricValue: {
    fontSize: 28,
    fontWeight: 'bold',
    color: '#2196f3',
    marginBottom: 4,
  },
  metricLabel: {
    fontSize: 12,
    color: '#666',
    textAlign: 'center',
  },
  card: {
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 16,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.1,
    shadowRadius: 4,
    elevation: 3,
  },
  topologyHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    marginBottom: 16,
    paddingBottom: 12,
    borderBottomWidth: 1,
    borderBottomColor: '#f0f0f0',
  },
  topologyInfo: {
    flex: 1,
  },
  topologyLabel: {
    fontSize: 12,
    color: '#666',
    marginBottom: 4,
  },
  topologyValue: {
    fontSize: 14,
    fontWeight: '600',
    color: '#333',
  },
  statsRow: {
    flexDirection: 'row',
    justifyContent: 'space-around',
    paddingVertical: 12,
  },
  statItem: {
    alignItems: 'center',
  },
  statValue: {
    fontSize: 20,
    fontWeight: 'bold',
    color: '#2196f3',
    marginBottom: 4,
  },
  statLabel: {
    fontSize: 11,
    color: '#666',
  },
  diameterInfo: {
    marginTop: 12,
    paddingTop: 12,
    borderTopWidth: 1,
    borderTopColor: '#f0f0f0',
    alignItems: 'center',
  },
  diameterText: {
    fontSize: 13,
    color: '#666',
    fontStyle: 'italic',
  },
  nodeCard: {
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 14,
    marginBottom: 8,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.08,
    shadowRadius: 2,
    elevation: 2,
  },
  nodeHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 10,
  },
  nodeId: {
    fontSize: 15,
    fontWeight: '600',
    color: '#333',
    flex: 1,
  },
  roleBadge: {
    paddingHorizontal: 10,
    paddingVertical: 4,
    borderRadius: 12,
  },
  roleBadgeRelay: {
    backgroundColor: '#4caf50',
  },
  roleBadgeNormal: {
    backgroundColor: '#2196f3',
  },
  roleBadgeText: {
    color: '#fff',
    fontSize: 11,
    fontWeight: '600',
  },
  nodeDetails: {
    flexDirection: 'row',
    flexWrap: 'wrap',
  },
  nodeDetailItem: {
    flexDirection: 'row',
    marginRight: 16,
    marginTop: 4,
  },
  nodeDetailLabel: {
    fontSize: 12,
    color: '#666',
    marginRight: 4,
  },
  nodeDetailValue: {
    fontSize: 12,
    color: '#333',
    fontWeight: '500',
  },
  linkCard: {
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 14,
    marginBottom: 8,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.08,
    shadowRadius: 2,
    elevation: 2,
  },
  linkPath: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 10,
  },
  linkNode: {
    flex: 1,
    fontSize: 13,
    fontWeight: '500',
    color: '#333',
  },
  linkArrow: {
    fontSize: 16,
    color: '#999',
    marginHorizontal: 8,
  },
  linkDetails: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  linkQuality: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    marginRight: 12,
  },
  qualityBar: {
    flex: 1,
    height: 8,
    backgroundColor: '#f0f0f0',
    borderRadius: 4,
    overflow: 'hidden',
    marginRight: 8,
  },
  qualityFill: {
    height: '100%',
    borderRadius: 4,
  },
  qualityText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#333',
    width: 36,
  },
  linkInfo: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  linkTransport: {
    fontSize: 11,
    color: '#666',
    backgroundColor: '#f5f5f5',
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 10,
    marginRight: 6,
  },
  linkRssi: {
    fontSize: 11,
    color: '#666',
  },
  messageCard: {
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 14,
    marginBottom: 8,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.08,
    shadowRadius: 2,
    elevation: 2,
  },
  messageHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 8,
  },
  messageId: {
    fontSize: 12,
    fontFamily: 'monospace',
    color: '#666',
    flex: 1,
    marginRight: 8,
  },
  messageStatus: {
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 10,
  },
  messageStatusDelivered: {
    backgroundColor: '#e8f5e9',
  },
  messageStatusPending: {
    backgroundColor: '#fff3cd',
  },
  messageStatusText: {
    fontSize: 10,
    fontWeight: '600',
  },
  messagePath: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 8,
  },
  messageNode: {
    flex: 1,
    fontSize: 13,
    fontWeight: '500',
    color: '#333',
  },
  messageArrow: {
    fontSize: 14,
    color: '#999',
    marginHorizontal: 8,
  },
  messageStats: {
    flexDirection: 'row',
    flexWrap: 'wrap',
  },
  messageStat: {
    fontSize: 11,
    color: '#666',
    backgroundColor: '#f5f5f5',
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 10,
    marginRight: 6,
    marginTop: 4,
  },
});

