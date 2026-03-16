import React, {useState} from 'react';
import {
  View,
  Text,
  SectionList,
  TouchableOpacity,
  Switch,
  StyleSheet,
  ActivityIndicator,
} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {formatUserId, formatMessageTime} from '../utils';

export function ServicesScreen() {
  const {
    registeredServices,
    discoveredServices,
    serviceLog,
    registerService,
    unregisterService,
    discoverServices,
    sendServiceRequest,
  } = useProtocol();

  const [isScanning, setIsScanning] = useState(false);
  const [pendingPings, setPendingPings] = useState<Set<string>>(new Set());

  const isPingRegistered = registeredServices.includes('ping.v1');

  const handleTogglePing = async () => {
    try {
      if (isPingRegistered) {
        await unregisterService('ping.v1');
      } else {
        await registerService('ping.v1', '1.0');
      }
    } catch (error) {
      console.warn('Service toggle failed:', error);
    }
  };

  const handleScan = async () => {
    setIsScanning(true);
    try {
      await discoverServices();
    } catch (error) {
      console.warn('Discovery failed:', error);
    }
    // Give some time for responses
    setTimeout(() => setIsScanning(false), 3000);
  };

  const handlePing = async (provider: string, serviceId: string) => {
    const key = `${provider}-${serviceId}`;
    setPendingPings(prev => new Set(prev).add(key));
    try {
      await sendServiceRequest(provider, serviceId, 'ping', 'ping');
    } catch (error) {
      console.warn('Ping failed:', error);
    }
    setTimeout(() => {
      setPendingPings(prev => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }, 5000);
  };

  // Recent log entries (last 20)
  const recentLog = serviceLog.slice(-20).reverse();

  const sections = [
    {
      title: 'My Services',
      data: [{key: 'ping-toggle'}],
    },
    {
      title: 'Discover',
      data: [{key: 'scan-button'}, ...discoveredServices.map((s, i) => ({key: `svc-${i}`, ...s}))],
    },
    ...(recentLog.length > 0
      ? [{
          title: 'Activity Log',
          data: recentLog.map((entry, i) => ({key: `log-${i}`, ...entry})),
        }]
      : []),
  ];

  const renderItem = ({item}: {item: any}) => {
    // Ping toggle
    if (item.key === 'ping-toggle') {
      return (
        <View style={styles.row}>
          <View style={styles.rowContent}>
            <Text style={styles.serviceName}>Ping Service</Text>
            <Text style={styles.serviceDetail}>
              {isPingRegistered
                ? 'Active — auto-responds "pong" to requests'
                : 'Register to respond to ping requests'}
            </Text>
          </View>
          <Switch
            value={isPingRegistered}
            onValueChange={handleTogglePing}
            trackColor={{true: '#34C759'}}
          />
        </View>
      );
    }

    // Scan button
    if (item.key === 'scan-button') {
      return (
        <TouchableOpacity
          style={styles.scanButton}
          onPress={handleScan}
          disabled={isScanning}>
          {isScanning ? (
            <ActivityIndicator color="#007AFF" size="small" />
          ) : (
            <Text style={styles.scanText}>Scan for Services</Text>
          )}
        </TouchableOpacity>
      );
    }

    // Discovered service
    if (item.serviceId) {
      const pingKey = `${item.provider}-${item.serviceId}`;
      const isPinging = pendingPings.has(pingKey);

      return (
        <View style={styles.row}>
          <View style={styles.rowContent}>
            <Text style={styles.serviceName}>{item.serviceId}</Text>
            <Text style={styles.serviceDetail}>
              Provider: {formatUserId(item.provider)} · v{item.version}
            </Text>
          </View>
          <TouchableOpacity
            style={[styles.pingButton, isPinging && styles.pingButtonDisabled]}
            onPress={() => handlePing(item.provider, item.serviceId)}
            disabled={isPinging}>
            <Text style={styles.pingText}>
              {isPinging ? '...' : 'Ping'}
            </Text>
          </TouchableOpacity>
        </View>
      );
    }

    // Log entry
    if (item.type) {
      const isRequest = item.type === 'request';
      return (
        <View style={styles.logRow}>
          <Text style={[styles.logType, isRequest ? styles.logRequest : styles.logResponse]}>
            {isRequest ? '← REQ' : '→ RES'}
          </Text>
          <View style={styles.logContent}>
            <Text style={styles.logBody} numberOfLines={1}>{item.body}</Text>
            <Text style={styles.logMeta}>
              {item.from === 'me' ? 'Sent' : `From ${formatUserId(item.from)}`} · {formatMessageTime(item.timestamp)}
            </Text>
          </View>
        </View>
      );
    }

    return null;
  };

  return (
    <SectionList
      sections={sections}
      renderItem={renderItem}
      renderSectionHeader={({section}) => (
        <View style={styles.sectionHeader}>
          <Text style={styles.sectionTitle}>{section.title}</Text>
        </View>
      )}
      keyExtractor={item => item.key}
      contentContainerStyle={styles.list}
      stickySectionHeadersEnabled={false}
    />
  );
}

const styles = StyleSheet.create({
  list: {
    paddingBottom: 16,
  },
  sectionHeader: {
    paddingHorizontal: 16,
    paddingTop: 20,
    paddingBottom: 8,
    backgroundColor: '#F2F2F7',
  },
  sectionTitle: {
    fontSize: 13,
    fontWeight: '600',
    color: '#8E8E93',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 12,
  },
  rowContent: {
    flex: 1,
    gap: 2,
  },
  serviceName: {
    fontSize: 16,
    fontWeight: '500',
    color: '#1C1C1E',
  },
  serviceDetail: {
    fontSize: 13,
    color: '#8E8E93',
  },
  scanButton: {
    backgroundColor: '#FFFFFF',
    paddingVertical: 14,
    alignItems: 'center',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  scanText: {
    fontSize: 16,
    color: '#007AFF',
    fontWeight: '600',
  },
  pingButton: {
    backgroundColor: '#007AFF',
    paddingHorizontal: 16,
    paddingVertical: 7,
    borderRadius: 16,
  },
  pingButtonDisabled: {
    opacity: 0.5,
  },
  pingText: {
    color: '#FFFFFF',
    fontSize: 14,
    fontWeight: '600',
  },
  logRow: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 10,
  },
  logType: {
    fontSize: 11,
    fontWeight: '700',
    width: 44,
    textAlign: 'center',
    paddingVertical: 2,
    borderRadius: 4,
    overflow: 'hidden',
  },
  logRequest: {
    color: '#FF9500',
    backgroundColor: '#FFF5E6',
  },
  logResponse: {
    color: '#34C759',
    backgroundColor: '#EDFCF0',
  },
  logContent: {
    flex: 1,
    gap: 1,
  },
  logBody: {
    fontSize: 14,
    color: '#1C1C1E',
  },
  logMeta: {
    fontSize: 11,
    color: '#8E8E93',
  },
});
