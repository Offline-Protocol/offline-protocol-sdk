import React, {useState, useMemo} from 'react';
import {View, Text, StyleSheet, StatusBar, SafeAreaView} from 'react-native';
import {ProtocolProvider, useProtocol} from './context/ProtocolContext';
import {TabBar} from './components/TabBar';
import {OnboardingScreen} from './screens/OnboardingScreen';
import {PeersScreen} from './screens/PeersScreen';
import {ChatScreen} from './screens/ChatScreen';
import {LogsScreen} from './screens/LogsScreen';
import {formatUserId} from './utils';
import type {TabName} from './types';

function MainApp() {
  const [isOnboarded, setIsOnboarded] = useState(false);
  const [activeTab, setActiveTab] = useState<TabName>('chat');
  const {chats, userName, userId, isTransportEnabled} = useProtocol();

  const totalUnread = useMemo(() => {
    let count = 0;
    for (const chat of chats.values()) {
      count += chat.unreadCount;
    }
    return count;
  }, [chats]);

  if (!isOnboarded) {
    return <OnboardingScreen onComplete={() => setIsOnboarded(true)} />;
  }

  return (
    <SafeAreaView style={styles.container}>
      <StatusBar barStyle="dark-content" backgroundColor="#FFFFFF" />

      {/* Top Bar */}
      <View style={styles.topBar}>
        <View style={styles.topBarLeft}>
          <Text style={styles.topBarTitle}>Nostr Transport</Text>
          <View style={styles.statusRow}>
            <View
              style={[
                styles.statusDot,
                isTransportEnabled ? styles.connected : styles.disconnected,
              ]}
            />
            <Text style={styles.statusText}>
              {isTransportEnabled ? 'Connected' : 'Disconnected'}
            </Text>
          </View>
        </View>
        <View style={styles.topBarRight}>
          <Text style={styles.topBarUser}>{userName}</Text>
          <Text style={styles.topBarId}>{formatUserId(userId)}</Text>
        </View>
      </View>

      {/* Screen Content */}
      <View style={styles.content}>
        {activeTab === 'peers' && <PeersScreen />}
        {activeTab === 'chat' && <ChatScreen />}
        {activeTab === 'logs' && <LogsScreen />}
      </View>

      {/* Bottom Tab Bar */}
      <TabBar
        activeTab={activeTab}
        onTabChange={setActiveTab}
        unreadChats={totalUnread}
      />
    </SafeAreaView>
  );
}

export default function App() {
  return (
    <ProtocolProvider>
      <MainApp />
    </ProtocolProvider>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#F2F2F7',
  },
  topBar: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  topBarLeft: {
    gap: 4,
  },
  topBarTitle: {
    fontSize: 18,
    fontWeight: '700',
    color: '#1C1C1E',
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  statusDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  connected: {
    backgroundColor: '#4CAF50',
  },
  disconnected: {
    backgroundColor: '#F44336',
  },
  statusText: {
    fontSize: 11,
    color: '#8E8E93',
  },
  topBarRight: {
    alignItems: 'flex-end',
  },
  topBarUser: {
    fontSize: 14,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  topBarId: {
    fontSize: 11,
    color: '#8E8E93',
  },
  content: {
    flex: 1,
  },
});
