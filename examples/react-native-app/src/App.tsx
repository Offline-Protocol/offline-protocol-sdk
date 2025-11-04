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

type Tab = 'messaging' | 'network' | 'events';

export default function App() {
  const [userId, setUserId] = useState('user_' + Math.random().toString(36).substr(2, 9));
  const [activeTab, setActiveTab] = useState<Tab>('messaging');

  const {
    isStarted,
    error,
    events,
    start,
    stop,
    sendMessage,
    clearEvents,
  } = useOfflineProtocol({
    appId: 'offline-protocol-example',
    userId,
    transport: {
      bleEnabled: true,
      wifiDirectEnabled: true,
      internetEnabled: true,
    },
    dors: {
      preferOnline: true,
    },
    relay: {
      allowRelay: true,
      minBatteryForRelay: 20,
      relayThreshold: 3,
    },
    network: {
      initialTtl: 10,
    },
  });

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
      case 'events':
        return <EventLog events={events} onClear={clearEvents} />;
    }
  };

  return (
    <SafeAreaView style={styles.container}>
      <RNStatusBar barStyle="dark-content" />
      
      <View style={styles.header}>
        <Text style={styles.title}>Offline Protocol Example</Text>
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

        <TouchableOpacity
          style={[styles.controlButton, isStarted ? styles.stopButton : styles.startButton]}
          onPress={handleStartStop}
        >
          <Text style={styles.controlButtonText}>
            {isStarted ? 'Stop Protocol' : 'Start Protocol'}
          </Text>
        </TouchableOpacity>

        <View style={styles.tabs}>
          <TouchableOpacity
            style={[styles.tab, activeTab === 'messaging' && styles.activeTab]}
            onPress={() => setActiveTab('messaging')}
          >
            <Text style={[styles.tabText, activeTab === 'messaging' && styles.activeTabText]}>
              Messaging
            </Text>
          </TouchableOpacity>
          <TouchableOpacity
            style={[styles.tab, activeTab === 'network' && styles.activeTab]}
            onPress={() => setActiveTab('network')}
          >
            <Text style={[styles.tabText, activeTab === 'network' && styles.activeTabText]}>
              Network
            </Text>
          </TouchableOpacity>
          <TouchableOpacity
            style={[styles.tab, activeTab === 'events' && styles.activeTab]}
            onPress={() => setActiveTab('events')}
          >
            <Text style={[styles.tabText, activeTab === 'events' && styles.activeTabText]}>
              Events ({events.length})
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
    backgroundColor: '#fff',
  },
  header: {
    padding: 16,
    backgroundColor: '#2196f3',
  },
  title: {
    fontSize: 24,
    fontWeight: 'bold',
    color: '#fff',
    marginBottom: 12,
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
  },
  content: {
    flex: 1,
    padding: 16,
  },
  controlButton: {
    paddingVertical: 14,
    borderRadius: 8,
    alignItems: 'center',
    marginBottom: 16,
  },
  startButton: {
    backgroundColor: '#4caf50',
  },
  stopButton: {
    backgroundColor: '#f44336',
  },
  controlButtonText: {
    color: '#fff',
    fontSize: 16,
    fontWeight: '600',
  },
  tabs: {
    flexDirection: 'row',
    borderBottomWidth: 2,
    borderBottomColor: '#e0e0e0',
    marginBottom: 16,
  },
  tab: {
    flex: 1,
    paddingVertical: 12,
    alignItems: 'center',
  },
  activeTab: {
    borderBottomWidth: 2,
    borderBottomColor: '#2196f3',
  },
  tabText: {
    fontSize: 14,
    fontWeight: '500',
    color: '#666',
  },
  activeTabText: {
    color: '#2196f3',
    fontWeight: '600',
  },
  tabContent: {
    flex: 1,
  },
});

