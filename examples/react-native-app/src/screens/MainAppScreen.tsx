import React, { useState } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  Platform,
  Dimensions,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import LinearGradient from 'react-native-linear-gradient';

// Screens
import { ChatsScreen } from './ChatsScreen';
import { ContactsScreen } from './ContactsScreen';
import { AnalyticsScreen } from './AnalyticsScreen';
import { SettingsScreen } from './SettingsScreen';
import { ChatDetailScreen } from './ChatDetailScreen';
import { ProfileScreen } from './ProfileScreen';
import { ControlCenterScreen } from './ControlCenterScreen';
import { VisualizationScreen } from './VisualizationScreen';
import { NetworkScreen } from './NetworkScreen';

// Components
import { Icon } from '../components/Icon';

// Hooks
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';

type Tab = 'chats' | 'contacts' | 'analytics' | 'settings';
type Screen = 'main' | 'chatDetail' | 'profile' | 'controlCenter' | 'visualization' | 'network';

interface ChatDetailParams {
  peerId: string;
  peerName: string;
}

interface ProfileParams {
  userId?: string;
}

export function MainAppScreen() {
  const { theme } = useTheme();
  const {
    connectedPeersCount,
    chats,
    isOnline,
    events,
    insights,
    protocol,
    activeTransports,
    forcedTransport,
    relayPriority,
    batteryLevel,
    dorsConfig,
    fileTransfers,
    refreshRuntimeState,
    enableTransport,
    disableTransport,
    forceTransport,
    releaseTransportLock,
    setBatteryLevel,
    setRelayPriority,
    updateDorsConfig,
    sendFile,
    cancelFileTransfer,
  } = useProtocol();
  
  const [activeTab, setActiveTab] = useState<Tab>('chats');
  const [currentScreen, setCurrentScreen] = useState<Screen>('main');
  const [chatDetailParams, setChatDetailParams] = useState<ChatDetailParams | null>(null);
  const [profileParams, setProfileParams] = useState<ProfileParams | null>(null);

  const { width } = Dimensions.get('window');
  const isTablet = width >= 768;

  // Navigation helpers
  const navigateToChatDetail = (peerId: string, peerName: string) => {
    setChatDetailParams({ peerId, peerName });
    setCurrentScreen('chatDetail');
  };

  const navigateToProfile = (userId?: string) => {
    setProfileParams({ userId });
    setCurrentScreen('profile');
  };

  const navigateToControlCenter = () => {
    setCurrentScreen('controlCenter');
  };

  const navigateToVisualization = () => {
    setCurrentScreen('visualization');
  };

  const navigateToNetwork = () => {
    setCurrentScreen('network');
  };

  const navigateBack = () => {
    setCurrentScreen('main');
    setChatDetailParams(null);
    setProfileParams(null);
  };

  const totalUnreadCount = chats.reduce((sum, chat) => sum + chat.unreadCount, 0);

  const tabs = [
    {
      id: 'chats' as const,
      label: 'Chats',
      icon: 'chatbubbles',
      iconActive: 'chatbubbles',
      badge: totalUnreadCount > 0 ? totalUnreadCount : undefined,
    },
    {
      id: 'contacts' as const,
      label: 'Contacts',
      icon: 'people-outline',
      iconActive: 'people',
      badge: connectedPeersCount > 0 ? connectedPeersCount : undefined,
    },
    {
      id: 'analytics' as const,
      label: 'Analytics',
      icon: 'analytics-outline',
      iconActive: 'analytics',
    },
    {
      id: 'settings' as const,
      label: 'Settings',
      icon: 'settings-outline',
      iconActive: 'settings',
    },
  ];

  const renderTabBar = () => (
    <View style={[styles.tabBar, { backgroundColor: theme.colors.surface }]}>
      {tabs.map((tab) => {
        const isActive = activeTab === tab.id;
        const iconName = isActive ? tab.iconActive : tab.icon;
        
        return (
          <TouchableOpacity
            key={tab.id}
            style={[
              styles.tabItem,
              isTablet && styles.tabItemTablet,
            ]}
            onPress={() => setActiveTab(tab.id)}
            activeOpacity={0.7}
          >
            <View style={styles.tabIconContainer}>
              <Icon 
                name={iconName} 
                size={isTablet ? 28 : 24} 
                color={isActive ? theme.colors.primary : theme.colors.textSecondary} 
              />
              {tab.badge && (
                <View style={[styles.badge, { backgroundColor: theme.colors.primary }]}>
                  <Text style={[styles.badgeText, { color: theme.colors.textInverse }]}>
                    {tab.badge > 99 ? '99+' : tab.badge}
                  </Text>
                </View>
              )}
            </View>
            <Text
              style={[
                styles.tabLabel,
                isTablet && styles.tabLabelTablet,
                {
                  color: isActive ? theme.colors.primary : theme.colors.textSecondary,
                  fontWeight: isActive ? '600' : '400',
                },
              ]}
            >
              {tab.label}
            </Text>
          </TouchableOpacity>
        );
      })}
    </View>
  );

  const renderScreen = () => {
    if (currentScreen === 'chatDetail' && chatDetailParams) {
      return (
        <ChatDetailScreen
          peerId={chatDetailParams.peerId}
          peerName={chatDetailParams.peerName}
          onBack={navigateBack}
          onNavigateToProfile={navigateToProfile}
        />
      );
    }

    if (currentScreen === 'profile' && profileParams) {
      return (
        <ProfileScreen
          userId={profileParams.userId}
          onBack={navigateBack}
          onNavigateToChatDetail={navigateToChatDetail}
        />
      );
    }

    if (currentScreen === 'controlCenter') {
      return (
        <ControlCenterScreen
          isStarted={isOnline}
          activeTransports={activeTransports}
          forcedTransport={forcedTransport}
          relayPriority={relayPriority}
          batteryLevel={batteryLevel}
          dorsConfig={dorsConfig}
          fileTransfers={fileTransfers}
          onRefresh={refreshRuntimeState}
          onEnableTransport={enableTransport}
          onDisableTransport={disableTransport}
          onForceTransport={forceTransport}
          onReleaseTransport={releaseTransportLock}
          onSetBatteryLevel={setBatteryLevel}
          onSetRelayPriority={setRelayPriority}
          onUpdateDors={updateDorsConfig}
          onSendFile={sendFile}
          onCancelFile={cancelFileTransfer}
        />
      );
    }

    if (currentScreen === 'visualization') {
      return (
        <VisualizationScreen
          protocol={protocol}
          isStarted={isOnline}
          insights={insights}
        />
      );
    }

    if (currentScreen === 'network') {
      return <NetworkScreen events={events} insights={insights} />;
    }

    // Main tabs
    switch (activeTab) {
      case 'chats':
        return <ChatsScreen onNavigateToChatDetail={navigateToChatDetail} />;
      case 'contacts':
        return (
          <ContactsScreen
            onNavigateToProfile={navigateToProfile}
            onNavigateToChatDetail={navigateToChatDetail}
          />
        );
      case 'analytics':
        return (
          <AnalyticsScreen
            onOpenVisualization={navigateToVisualization}
            onOpenNetwork={navigateToNetwork}
          />
        );
      case 'settings':
        return (
          <SettingsScreen
            onOpenControlCenter={navigateToControlCenter}
            onOpenNetwork={navigateToNetwork}
            onOpenVisualization={navigateToVisualization}
          />
        );
      default:
        return <ChatsScreen onNavigateToChatDetail={navigateToChatDetail} />;
    }
  };

  const showBackButton = currentScreen !== 'main';

  return (
    <SafeAreaView style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {/* Header with back button */}
      {showBackButton && (
        <View style={[styles.header, { backgroundColor: theme.colors.surface }]}>
          <TouchableOpacity
            style={styles.backButton}
            onPress={navigateBack}
            activeOpacity={0.7}
          >
            <Icon name="arrow-back" size={24} color={theme.colors.primary} />
            <Text style={[styles.backText, { color: theme.colors.primary }]}>
              Back
            </Text>
          </TouchableOpacity>
        </View>
      )}

      {/* Main content */}
      <View style={styles.content}>
        {renderScreen()}
      </View>

      {/* Tab bar - only show on main screen */}
      {currentScreen === 'main' && renderTabBar()}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(0,0,0,0.1)',
  },
  backButton: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 8,
  },
  backText: {
    fontSize: 16,
    fontWeight: '500',
    marginLeft: 8,
  },
  content: {
    flex: 1,
  },
  tabBar: {
    flexDirection: 'row',
    borderTopWidth: 1,
    borderTopColor: 'rgba(0,0,0,0.1)',
    paddingBottom: Platform.OS === 'ios' ? 20 : 10,
    paddingTop: 10,
    paddingHorizontal: 8,
  },
  tabItem: {
    flex: 1,
    alignItems: 'center',
    paddingVertical: 8,
    paddingHorizontal: 4,
  },
  tabItemTablet: {
    paddingVertical: 12,
  },
  tabIconContainer: {
    position: 'relative',
    marginBottom: 4,
  },
  badge: {
    position: 'absolute',
    top: -6,
    right: -6,
    minWidth: 18,
    height: 18,
    borderRadius: 9,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 4,
  },
  badgeText: {
    fontSize: 10,
    fontWeight: '600',
  },
  tabLabel: {
    fontSize: 12,
    textAlign: 'center',
  },
  tabLabelTablet: {
    fontSize: 14,
  },
});
