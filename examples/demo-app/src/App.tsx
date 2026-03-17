import React, {useState, useCallback, useMemo} from 'react';
import {View, Text, StyleSheet, StatusBar} from 'react-native';
import {SafeAreaProvider, SafeAreaView} from 'react-native-safe-area-context';
import {ProtocolProvider, useProtocol} from './context/ProtocolContext';
import {TabBar} from './components/TabBar';
import {OnboardingScreen} from './screens/OnboardingScreen';
import {PeopleScreen} from './screens/PeopleScreen';
import {ChatsScreen} from './screens/ChatsScreen';
import {GroupsScreen} from './screens/GroupsScreen';
import {ServicesScreen} from './screens/ServicesScreen';
import type {TabName} from './types';
import {formatUserId} from './utils';

function MainApp() {
  const [isOnboarded, setIsOnboarded] = useState(false);
  const [activeTab, setActiveTab] = useState<TabName>('people');
  const [chatPeerId, setChatPeerId] = useState<string | null>(null);
  const {chats, userName, userId} = useProtocol();

  const totalUnread = useMemo(() => {
    let count = 0;
    for (const chat of chats.values()) {
      count += chat.unreadCount;
    }
    return count;
  }, [chats]);

  const handleOpenChat = useCallback((peerId: string) => {
    setChatPeerId(peerId);
    setActiveTab('chats');
  }, []);

  const handleClearChatPeer = useCallback(() => {
    setChatPeerId(null);
  }, []);

  if (!isOnboarded) {
    return <OnboardingScreen onComplete={() => setIsOnboarded(true)} />;
  }

  return (
    <SafeAreaView style={styles.container} edges={['top']}>
      <StatusBar barStyle="dark-content" backgroundColor="#FFFFFF" />

      {/* Top Bar */}
      <View style={styles.topBar}>
        <Text style={styles.topBarTitle}>Offline Demo</Text>
        <View style={styles.topBarRight}>
          <Text style={styles.topBarUser}>{userName}</Text>
          <Text style={styles.topBarId}>{formatUserId(userId)}</Text>
        </View>
      </View>

      {/* Screen Content */}
      <View style={styles.content}>
        {activeTab === 'people' && (
          <PeopleScreen onOpenChat={handleOpenChat} />
        )}
        {activeTab === 'chats' && (
          <ChatsScreen
            initialPeerId={chatPeerId}
            onClearInitialPeer={handleClearChatPeer}
          />
        )}
        {activeTab === 'groups' && <GroupsScreen />}
        {activeTab === 'services' && <ServicesScreen />}
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
    <SafeAreaProvider>
      <ProtocolProvider>
        <MainApp />
      </ProtocolProvider>
    </SafeAreaProvider>
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
  topBarTitle: {
    fontSize: 18,
    fontWeight: '700',
    color: '#1C1C1E',
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
