import React from 'react';
import {
  View,
  Text,
  StyleSheet,
  ScrollView,
  TouchableOpacity,
  Platform,
  Alert,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
// import { useRoute, useNavigation } from '@react-navigation/native';
import { Icon } from '../components/Icon';
import LinearGradient from 'react-native-linear-gradient';
// import Animated, { FadeInDown, FadeInUp } from 'react-native-reanimated';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { MessagePriority } from '@offlineprotocol/react-native';
import { getUserInitials, generateAvatarColor, formatUserId } from '../utils/user';

interface InfoRowProps {
  icon: string;
  label: string;
  value: string;
  index: number;
}

function InfoRow({ icon, label, value, index }: InfoRowProps) {
  const { theme } = useTheme();

  return (
    <View 
      style={[styles.infoRow, { backgroundColor: theme.colors.surface }]}
    >
      <View style={[styles.infoIcon, { backgroundColor: theme.colors.primary + '20' }]}>
        <Icon name={icon} size={20} color={theme.colors.primary} />
      </View>
      <View style={styles.infoContent}>
        <Text style={[styles.infoLabel, { color: theme.colors.textSecondary }]}>
          {label}
        </Text>
        <Text style={[styles.infoValue, { color: theme.colors.text }]}>
          {value}
        </Text>
      </View>
    </View>
  );
}

interface ProfileScreenProps {
  userId?: string;
  onBack: () => void;
  onNavigateToChatDetail: (peerId: string, peerName: string) => void;
}

export function ProfileScreen({ userId, onBack, onNavigateToChatDetail }: ProfileScreenProps) {
  const { theme } = useTheme();
  
  const { 
    contacts, 
    chats, 
    currentUserId, 
    currentUserName, 
    sendMessage 
  } = useProtocol();

  // Determine if this is the current user's profile
  const isOwnProfile = !userId || userId === currentUserId;
  const targetUserId = userId || currentUserId;
  
  // Get contact/user info
  const contact = contacts.find(c => c.id === targetUserId);
  const chat = chats.find(c => c.peerId === targetUserId);
  const userName = isOwnProfile ? currentUserName : (contact?.name || `User ${targetUserId.slice(-4)}`);
  
  const avatarColor = generateAvatarColor(targetUserId);
  const initials = getUserInitials(userName);

  const handleMessage = () => {
    if (isOwnProfile) return;

    if (chat) {
      onNavigateToChatDetail(targetUserId, userName);
    } else {
      Alert.prompt(
        `Message ${userName}`,
        'Enter your message:',
        [
          { text: 'Cancel', style: 'cancel' },
          {
            text: 'Send',
            onPress: async (text?: string) => {
              if (text?.trim()) {
                try {
                  await sendMessage(targetUserId, text.trim(), MessagePriority.Medium);
                  onNavigateToChatDetail(targetUserId, userName);
                } catch (error) {
                  Alert.alert('Send Failed', 'Failed to send message. Please try again.');
                }
              }
            },
          },
        ],
        'plain-text'
      );
    }
  };

  // Call features removed as requested

  const getStatusText = () => {
    if (isOwnProfile) {
      return 'This is you';
    }
    if (contact?.isOnline) {
      return 'Online';
    }
    if (contact?.lastSeen) {
      const now = Date.now();
      const diffInMinutes = (now - contact.lastSeen) / (1000 * 60);
      
      if (diffInMinutes < 1) return 'Just now';
      if (diffInMinutes < 60) return `Last seen ${Math.floor(diffInMinutes)}m ago`;
      if (diffInMinutes < 1440) return `Last seen ${Math.floor(diffInMinutes / 60)}h ago`;
      return `Last seen ${Math.floor(diffInMinutes / 1440)}d ago`;
    }
    return 'Never seen';
  };

  const getDistanceText = () => {
    if (isOwnProfile || !contact?.distance) return null;
    
    switch (contact.distance) {
      case 'near':
        return 'Very close';
      case 'medium':
        return 'Nearby';
      case 'far':
        return 'Far away';
      default:
        return null;
    }
  };

  const getSignalStrength = () => {
    if (isOwnProfile || contact?.signalStrength === undefined) return null;
    return `${Math.round(contact.signalStrength * 100)}%`;
  };

  const getMessageCount = () => {
    if (!chat) return 0;
    return chat.messages.length;
  };

  return (
    <View style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {/* Header */}
      <LinearGradient
        colors={[avatarColor, avatarColor + 'CC']}
        style={styles.header}
      >
        <View  style={styles.headerContent}>
          {/* Avatar */}
          <View style={styles.avatarContainer}>
            <View style={[styles.avatar, { backgroundColor: 'rgba(255, 255, 255, 0.2)' }]}>
              <Text style={[styles.avatarText, { color: theme.colors.textInverse }]}>
                {initials}
              </Text>
              {!isOwnProfile && contact?.isOnline && (
                <View style={[styles.onlineIndicator, { backgroundColor: theme.colors.online }]} />
              )}
            </View>
          </View>

          {/* Name and Status */}
          <Text style={[styles.userName, { color: theme.colors.textInverse }]}>
            {userName}
          </Text>
          <Text style={[styles.userStatus, { color: theme.colors.textInverse }]}>
            {getStatusText()}
          </Text>
        </View>
      </LinearGradient>

      <ScrollView 
        style={styles.content}
        showsVerticalScrollIndicator={false}
        contentContainerStyle={styles.scrollContent}
      >
        {/* Action Buttons */}
        {!isOwnProfile && (
          <View  style={styles.actionsContainer}>
            <TouchableOpacity
              style={[styles.actionButton, { backgroundColor: theme.colors.primary }]}
              onPress={handleMessage}
              activeOpacity={0.8}
            >
              <Icon name="chatbubble" size={20} color={theme.colors.textInverse} />
              <Text style={[styles.actionText, { color: theme.colors.textInverse }]}>
                Send Message
              </Text>
            </TouchableOpacity>

          </View>
        )}

        {/* Info Section */}
        <View style={styles.infoSection}>
          <InfoRow
            icon="person"
            label="User ID"
            value={formatUserId(targetUserId)}
            index={0}
          />

          {getMessageCount() > 0 && (
            <InfoRow
              icon="chatbubbles"
              label="Messages"
              value={`${getMessageCount()} messages`}
              index={1}
            />
          )}

          {getDistanceText() && (
            <InfoRow
              icon="location"
              label="Distance"
              value={getDistanceText()!}
              index={2}
            />
          )}

          {getSignalStrength() && (
            <InfoRow
              icon="wifi"
              label="Signal Strength"
              value={getSignalStrength()!}
              index={3}
            />
          )}

          {contact?.lastSeen && !isOwnProfile && (
            <InfoRow
              icon="time"
              label="Last Seen"
              value={new Date(contact.lastSeen).toLocaleString()}
              index={4}
            />
          )}
        </View>

        {/* Stats Section */}
        {chat && (
          <View 
            style={[styles.statsSection, { backgroundColor: theme.colors.surface }]}
          >
            <Text style={[styles.statsTitle, { color: theme.colors.text }]}>
              Chat Statistics
            </Text>
            
            <View style={styles.statsGrid}>
              <View style={styles.statItem}>
                <Text style={[styles.statValue, { color: theme.colors.primary }]}>
                  {chat.messages.filter(m => m.isFromMe).length}
                </Text>
                <Text style={[styles.statLabel, { color: theme.colors.textSecondary }]}>
                  Sent
                </Text>
              </View>
              
              <View style={styles.statItem}>
                <Text style={[styles.statValue, { color: theme.colors.success }]}>
                  {chat.messages.filter(m => !m.isFromMe).length}
                </Text>
                <Text style={[styles.statLabel, { color: theme.colors.textSecondary }]}>
                  Received
                </Text>
              </View>
              
              <View style={styles.statItem}>
                <Text style={[styles.statValue, { color: theme.colors.warning }]}>
                  {chat.unreadCount}
                </Text>
                <Text style={[styles.statLabel, { color: theme.colors.textSecondary }]}>
                  Unread
                </Text>
              </View>
            </View>
          </View>
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    paddingTop: 40,
    paddingBottom: 32,
    paddingHorizontal: 20,
    borderBottomLeftRadius: 24,
    borderBottomRightRadius: 24,
  },
  headerContent: {
    alignItems: 'center',
  },
  avatarContainer: {
    marginBottom: 16,
  },
  avatar: {
    width: 100,
    height: 100,
    borderRadius: 50,
    alignItems: 'center',
    justifyContent: 'center',
    position: 'relative',
    borderWidth: 3,
    borderColor: 'rgba(255, 255, 255, 0.3)',
  },
  avatarText: {
    fontSize: 36,
    fontWeight: '700',
  },
  onlineIndicator: {
    position: 'absolute',
    bottom: 4,
    right: 4,
    width: 20,
    height: 20,
    borderRadius: 10,
    borderWidth: 3,
    borderColor: 'white',
  },
  userName: {
    fontSize: 24,
    fontWeight: '700',
    marginBottom: 4,
    textAlign: 'center',
  },
  userStatus: {
    fontSize: 16,
    fontWeight: '500',
    opacity: 0.9,
    textAlign: 'center',
  },
  content: {
    flex: 1,
  },
  scrollContent: {
    paddingTop: 20,
    paddingBottom: 40,
  },
  actionsContainer: {
    paddingHorizontal: 20,
    marginBottom: 24,
  },
  actionButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 16,
    borderRadius: 25,
    gap: 8,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 2 },
        shadowOpacity: 0.1,
        shadowRadius: 4,
      },
      android: {
        elevation: 2,
      },
    }),
  },
  actionText: {
    fontSize: 14,
    fontWeight: '600',
  },
  infoSection: {
    paddingHorizontal: 20,
    marginBottom: 24,
    gap: 12,
  },
  infoRow: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 16,
    borderRadius: 12,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 1 },
        shadowOpacity: 0.05,
        shadowRadius: 2,
      },
      android: {
        elevation: 1,
      },
    }),
  },
  infoIcon: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 16,
  },
  infoContent: {
    flex: 1,
  },
  infoLabel: {
    fontSize: 14,
    fontWeight: '500',
    marginBottom: 2,
  },
  infoValue: {
    fontSize: 16,
    fontWeight: '600',
  },
  statsSection: {
    marginHorizontal: 20,
    padding: 20,
    borderRadius: 16,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 2 },
        shadowOpacity: 0.1,
        shadowRadius: 4,
      },
      android: {
        elevation: 2,
      },
    }),
  },
  statsTitle: {
    fontSize: 18,
    fontWeight: '600',
    marginBottom: 16,
    textAlign: 'center',
  },
  statsGrid: {
    flexDirection: 'row',
    justifyContent: 'space-around',
  },
  statItem: {
    alignItems: 'center',
  },
  statValue: {
    fontSize: 24,
    fontWeight: '700',
    marginBottom: 4,
  },
  statLabel: {
    fontSize: 14,
    fontWeight: '500',
  },
});
