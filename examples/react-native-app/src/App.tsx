import React, { useState } from 'react';
import {
  View,
  Text,
  TouchableOpacity,
  StyleSheet,
  StatusBar as RNStatusBar,
  TextInput,
  Alert,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { MessagePriority } from '@offlineprotocol/react-native';
import { useOfflineProtocol } from './hooks/useOfflineProtocol';
import { StatusBar } from './components/StatusBar';
import { EventLog } from './components/EventLog';
import { MessagingScreen } from './screens/MessagingScreen';
import { NetworkScreen } from './screens/NetworkScreen';
import { VisualizationScreen } from './screens/VisualizationScreen';

type Tab = 'messaging' | 'network' | 'visualization' | 'events';

/**
 * Offline Protocol Example App
 * 
 * BLE Transport is now managed automatically at the bindings level!
 * 
 * When you call start():
 * - BLE scanning begins automatically (discovers nearby devices)
 * - BLE advertising begins automatically (makes this device discoverable)
 * - Fragment polling starts (sends queued messages)
 * - Fragment receiving is active (receives messages from peers)
 * 
 * No manual BLE setup required - everything is handled natively.
 */
export default function App() {
  const [userId, setUserId] = useState('user_' + Math.random().toString(36).substr(2, 9));
  const [activeTab, setActiveTab] = useState<Tab>('messaging');

  const {
    protocol,
    isStarted,
    error,
    events,
    permissionsGranted,
    start,
    stop,
    sendMessage,
    clearEvents,
    requestPermissions,
  } = useOfflineProtocol({
    appId: 'offline-protocol-example',
    userId,
    transports: {
      ble: {
        enabled: true,
      },
      internet: {
        enabled: false, // Enable if you have a relay server
        serverAddress: 'wss://relay.example.com',
        autoReconnect: true,
      },
      wifiDirect: {
        enabled: false, // Android only
        deviceName: 'OfflineProtocolDevice',
        autoAccept: false,
      },
    },
    dors: {
      preferOnline: false, // Set to true to prefer Internet when available
      switchHysteresis: 15.0, // Minimum score improvement to switch
      switchCooldownSecs: 20, // Cooldown after switching
      bleToWifiRetryThreshold: 2, // Retries before escalating to WiFi Direct
      rssiSwitchThreshold: -85, // RSSI threshold for switching (dBm)
      congestionQueueThreshold: 50, // Queue depth for congestion
      stabilityWindowSecs: 8, // Stability check window
    },
    relay: {
      allowRelay: true,
      minBatteryForRelay: 20,
      relayThreshold: 3,
      relayPriority: 'auto',
    },
    network: {
      initialTtl: 10,
    },
    reliability: {
      ack: {
        defaultTimeoutMs: 6000,
        maxPendingAcks: 2000,
      },
      retry: {
        maxRetries: 5,
        initialDelayMs: 1500,
        maxDelayMs: 45000,
        backoffMultiplier: 2.5,
        outboxMaxLifetimeMs: 2 * 60 * 60 * 1000, // 2 hours
      },
      dedup: {
        maxTrackedMessages: 20000,
        retentionTimeSecs: 3 * 60 * 60, // 3 hours
      },
    },
    path: {
      forwardToTopK: 3,
      maxCongestionLevel: 0.7,
    },
  });

  const handleRequestPermissions = async () => {
    await requestPermissions();
  };

  const handleStartStop = async () => {
    if (isStarted) {
      await stop();
    } else {
      await start();
    }
  };

  const handleSendMessage = async (
    recipient: string,
    content: string,
    priority: MessagePriority
  ) => {
    const messageId = await sendMessage(recipient, content, priority);
    if (messageId) {
      Alert.alert('Success', `Message sent with ID: ${messageId.substring(0, 8)}...`);
    } else if (error) {
      Alert.alert('Error', error);
    }
  };

  const renderContent = () => {
    switch (activeTab) {
      case 'messaging':
        return (
          <MessagingScreen
            events={events}
            currentUserId={userId}
            onSendMessage={handleSendMessage}
            isStarted={isStarted}
          />
        );
      case 'network':
        return <NetworkScreen events={events} />;
      case 'visualization':
        return <VisualizationScreen protocol={protocol} isStarted={isStarted} />;
      case 'events':
        return <EventLog events={events} onClear={clearEvents} />;
    }
  };

  return (
    <SafeAreaView style={styles.container}>
      <RNStatusBar barStyle="dark-content" />
      
      <View style={styles.header}>
        <View style={styles.headerRow}>
          <Text style={styles.title}>Offline Protocol Example</Text>
          <TouchableOpacity
            style={[styles.powerButton, isStarted ? styles.powerButtonActive : styles.powerButtonInactive]}
            onPress={handleStartStop}
            accessibilityRole="button"
            accessibilityLabel={isStarted ? 'Stop protocol' : 'Start protocol'}
          >
            <Text style={styles.powerButtonIcon}>⏻</Text>
          </TouchableOpacity>
        </View>
        <View style={styles.userIdContainer}>
          <Text style={styles.userIdLabel}>User ID:</Text>
          <TextInput
            style={styles.userIdInput}
            value={userId}
            onChangeText={setUserId}
            editable={!isStarted}
            placeholder="Enter user ID"
          />
        </View>
      </View>

      <View style={styles.content}>
        <StatusBar isStarted={isStarted} error={error} />

        {!permissionsGranted && (
          <View style={styles.permissionWarning}>
            <Text style={styles.permissionWarningText}>
              ⚠️ Bluetooth permissions required for offline messaging
            </Text>
            <Text style={[styles.permissionWarningText, {fontSize: 12, marginTop: 4}]}>
              BLE operations are managed automatically at the native level
            </Text>
            <TouchableOpacity
              style={styles.permissionButton}
              onPress={handleRequestPermissions}
            >
              <Text style={styles.permissionButtonText}>
                Grant Permissions
              </Text>
            </TouchableOpacity>
          </View>
        )}

        <View style={styles.tabs}>
          <TouchableOpacity
            style={[styles.tab, activeTab === 'messaging' && styles.activeTab]}
            onPress={() => setActiveTab('messaging')}
          >
            <Text style={[styles.tabText, activeTab === 'messaging' && styles.activeTabText]}>
              💬 Messages
            </Text>
          </TouchableOpacity>
          <TouchableOpacity
            style={[styles.tab, activeTab === 'network' && styles.activeTab]}
            onPress={() => setActiveTab('network')}
          >
            <Text style={[styles.tabText, activeTab === 'network' && styles.activeTabText]}>
              🌐 Network
            </Text>
          </TouchableOpacity>
          <TouchableOpacity
            style={[styles.tab, activeTab === 'visualization' && styles.activeTab]}
            onPress={() => setActiveTab('visualization')}
          >
            <Text style={[styles.tabText, activeTab === 'visualization' && styles.activeTabText]}>
              📊 Analytics
            </Text>
          </TouchableOpacity>
          <TouchableOpacity
            style={[styles.tab, activeTab === 'events' && styles.activeTab]}
            onPress={() => setActiveTab('events')}
          >
            <Text style={[styles.tabText, activeTab === 'events' && styles.activeTabText]}>
              📝 Events ({events.length})
            </Text>
          </TouchableOpacity>
        </View>

        <View style={styles.tabContent}>{renderContent()}</View>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  header: {
    padding: 16,
    backgroundColor: '#2196f3',
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.2,
    shadowRadius: 4,
    elevation: 5,
  },
  headerRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 12,
  },
  title: {
    fontSize: 24,
    fontWeight: 'bold',
    color: '#fff',
  },
  powerButton: {
    width: 42,
    height: 42,
    borderRadius: 21,
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.15,
    shadowRadius: 3,
    elevation: 4,
  },
  powerButtonActive: {
    backgroundColor: '#f44336',
  },
  powerButtonInactive: {
    backgroundColor: '#4caf50',
  },
  powerButtonIcon: {
    fontSize: 20,
    color: '#fff',
  },
  userIdContainer: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  userIdLabel: {
    color: '#fff',
    fontSize: 14,
    fontWeight: '500',
    marginRight: 8,
  },
  userIdInput: {
    flex: 1,
    backgroundColor: '#fff',
    borderRadius: 6,
    paddingHorizontal: 12,
    paddingVertical: 8,
    fontSize: 14,
    color: '#333',
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.1,
    shadowRadius: 2,
    elevation: 2,
  },
  content: {
    flex: 1,
    padding: 16,
  },
  permissionWarning: {
    backgroundColor: '#fff3cd',
    borderWidth: 1,
    borderColor: '#ffc107',
    borderRadius: 8,
    padding: 16,
    marginBottom: 16,
  },
  permissionWarningText: {
    color: '#856404',
    fontSize: 14,
    marginBottom: 12,
    textAlign: 'center',
  },
  permissionButton: {
    backgroundColor: '#ffc107',
    paddingVertical: 10,
    paddingHorizontal: 20,
    borderRadius: 6,
    alignItems: 'center',
  },
  permissionButtonText: {
    color: '#856404',
    fontSize: 14,
    fontWeight: '600',
  },
  tabs: {
    flexDirection: 'row',
    backgroundColor: '#fff',
    borderRadius: 12,
    marginBottom: 16,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.1,
    shadowRadius: 3,
    elevation: 2,
    overflow: 'hidden',
  },
  tab: {
    flex: 1,
    paddingVertical: 12,
    paddingHorizontal: 8,
    alignItems: 'center',
    justifyContent: 'center',
  },
  activeTab: {
    backgroundColor: '#e3f2fd',
  },
  tabText: {
    fontSize: 11,
    fontWeight: '500',
    color: '#666',
    textAlign: 'center',
  },
  activeTabText: {
    color: '#2196f3',
    fontWeight: '700',
  },
  tabContent: {
    flex: 1,
  },
});

