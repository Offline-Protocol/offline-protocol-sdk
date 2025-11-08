import React, { useState, useMemo } from 'react';
import {
  View,
  Text,
  StyleSheet,
  FlatList,
  TouchableOpacity,
  TextInput,
  Platform,
  RefreshControl,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
// import { useNavigation } from '@react-navigation/native';
import { Icon } from '../components/Icon';
import LinearGradient from 'react-native-linear-gradient';
// import Animated, { FadeInDown, FadeInRight } from 'react-native-reanimated';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { Chat } from '../providers/ProtocolProvider';
import { getUserInitials, generateAvatarColor } from '../utils/user';

interface ChatItemProps {
  chat: Chat;
  onPress: () => void;
  index: number;
}

function ChatItem({ chat, onPress, index }: ChatItemProps) {
  const { theme } = useTheme();
  const avatarColor = generateAvatarColor(chat.peerId);
  const initials = getUserInitials(chat.peerName);
  
  const formatTime = (timestamp: number) => {
    const now = new Date();
    const messageTime = new Date(timestamp);
    const diffInHours = (now.getTime() - messageTime.getTime()) / (1000 * 60 * 60);
    
    if (diffInHours < 1) {
      return 'now';
    } else if (diffInHours < 24) {
      return messageTime.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } else if (diffInHours < 48) {
      return 'yesterday';
    } else {
      return messageTime.toLocaleDateString([], { month: 'short', day: 'numeric' });
    }
  };

  const truncateMessage = (message: string, maxLength: number = 50) => {
    return message.length > maxLength ? `${message.substring(0, maxLength)}...` : message;
  };

  return (
    <View 
      style={[styles.chatItem, { backgroundColor: theme.colors.surface }]}
    >
      <TouchableOpacity onPress={onPress} style={styles.chatItemContent}>
        {/* Avatar */}
        <View style={[styles.avatar, { backgroundColor: avatarColor }]}>
          <Text style={[styles.avatarText, { color: theme.colors.textInverse }]}>
            {initials}
          </Text>
          {chat.isOnline && (
            <View style={[styles.onlineIndicator, { backgroundColor: theme.colors.online }]} />
          )}
        </View>

        {/* Chat Info */}
        <View style={styles.chatInfo}>
          <View style={styles.chatHeader}>
            <Text style={[styles.peerName, { color: theme.colors.text }]} numberOfLines={1}>
              {chat.peerName}
            </Text>
            {chat.lastMessage && (
              <Text style={[styles.timestamp, { color: theme.colors.textSecondary }]}>
                {formatTime(chat.lastMessage.timestamp)}
              </Text>
            )}
          </View>
          
          <View style={styles.messageRow}>
            {chat.lastMessage ? (
              <Text 
                style={[
                  styles.lastMessage, 
                  { 
                    color: theme.colors.textSecondary,
                    fontWeight: chat.unreadCount > 0 ? '600' : '400',
                  }
                ]} 
                numberOfLines={1}
              >
                {chat.lastMessage.isFromMe && '✓ '}
                {truncateMessage(chat.lastMessage.content)}
              </Text>
            ) : (
              <Text style={[styles.noMessages, { color: theme.colors.textSecondary }]}>
                No messages yet
              </Text>
            )}
            
            {chat.unreadCount > 0 && (
              <View style={[styles.unreadBadge, { backgroundColor: theme.colors.primary }]}>
                <Text style={[styles.unreadText, { color: theme.colors.textInverse }]}>
                  {chat.unreadCount > 99 ? '99+' : chat.unreadCount}
                </Text>
              </View>
            )}
          </View>
        </View>

        {/* Chevron */}
        <Icon 
          name="chevron-forward" 
          size={20} 
          color={theme.colors.textSecondary} 
          style={{ marginLeft: 8 }}
        />
      </TouchableOpacity>
    </View>
  );
}

interface ChatsScreenProps {
  onNavigateToChatDetail: (peerId: string, peerName: string) => void;
}

export function ChatsScreen({ onNavigateToChatDetail }: ChatsScreenProps) {
  const { theme } = useTheme();
  const { chats, isOnline, connectedPeersCount, markAsRead } = useProtocol();
  const [searchQuery, setSearchQuery] = useState('');
  const [refreshing, setRefreshing] = useState(false);

  const filteredChats = useMemo(() => {
    if (!searchQuery.trim()) {
      return chats.sort((a, b) => {
        const aTime = a.lastMessage?.timestamp || 0;
        const bTime = b.lastMessage?.timestamp || 0;
        return bTime - aTime;
      });
    }
    
    return chats.filter(chat =>
      chat.peerName.toLowerCase().includes(searchQuery.toLowerCase())
    ).sort((a, b) => {
      const aTime = a.lastMessage?.timestamp || 0;
      const bTime = b.lastMessage?.timestamp || 0;
      return bTime - aTime;
    });
  }, [chats, searchQuery]);

  const totalUnreadCount = chats.reduce((sum, chat) => sum + chat.unreadCount, 0);

  const handleChatPress = (chat: Chat) => {
    markAsRead(chat.id);
    onNavigateToChatDetail(chat.peerId, chat.peerName);
  };

  const handleRefresh = async () => {
    setRefreshing(true);
    // In a real app, you might refresh the connection or sync data
    setTimeout(() => setRefreshing(false), 1000);
  };

  const renderEmptyState = () => (
    <View 
      style={styles.emptyState}
    >
      <View style={[styles.emptyIcon, { backgroundColor: theme.colors.surfaceVariant }]}>
        <Icon name="chatbubbles-outline" size={48} color={theme.colors.textSecondary} />
      </View>
      <Text style={[styles.emptyTitle, { color: theme.colors.text }]}>
        No chats yet
      </Text>
      <Text style={[styles.emptySubtitle, { color: theme.colors.textSecondary }]}>
        {isOnline 
          ? `${connectedPeersCount} device${connectedPeersCount !== 1 ? 's' : ''} nearby. Go to Contacts to start a conversation.`
          : 'Turn on the messenger to discover nearby devices and start chatting.'
        }
      </Text>
    </View>
  );

  return (
    <View style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {/* Header */}
      <LinearGradient
        colors={[theme.colors.primary, theme.colors.primaryDark]}
        style={styles.header}
      >
        <View style={styles.headerContent}>
          <View>
            <Text style={[styles.headerTitle, { color: theme.colors.textInverse }]}>
              Messages
            </Text>
            <Text style={[styles.headerSubtitle, { color: theme.colors.textInverse }]}>
              {isOnline 
                ? `${connectedPeersCount} nearby • ${totalUnreadCount} unread`
                : 'Offline'
              }
            </Text>
          </View>
          
          <View style={styles.headerActions}>
            <View style={[
              styles.statusIndicator,
              { backgroundColor: isOnline ? theme.colors.online : theme.colors.offline }
            ]} />
          </View>
        </View>
      </LinearGradient>

      {/* Search Bar */}
      <View style={[styles.searchContainer, { backgroundColor: theme.colors.surface }]}>
        <View style={[styles.searchBar, { backgroundColor: theme.colors.background }]}>
          <Icon name="search" size={20} color={theme.colors.textSecondary} />
          <TextInput
            style={[styles.searchInput, { color: theme.colors.text }]}
            placeholder="Search chats..."
            placeholderTextColor={theme.colors.textSecondary}
            value={searchQuery}
            onChangeText={setSearchQuery}
            returnKeyType="search"
          />
          {searchQuery.length > 0 && (
            <TouchableOpacity onPress={() => setSearchQuery('')} style={styles.clearButton}>
              <Icon name="close-circle" size={20} color={theme.colors.textSecondary} />
            </TouchableOpacity>
          )}
        </View>
      </View>

      {/* Chat List */}
      <FlatList
        data={filteredChats}
        keyExtractor={(item) => item.id}
        renderItem={({ item, index }) => (
          <ChatItem
            chat={item}
            onPress={() => handleChatPress(item)}
            index={index}
          />
        )}
        contentContainerStyle={styles.listContainer}
        showsVerticalScrollIndicator={false}
        ListEmptyComponent={renderEmptyState}
        refreshControl={
          <RefreshControl
            refreshing={refreshing}
            onRefresh={handleRefresh}
            tintColor={theme.colors.primary}
            colors={[theme.colors.primary]}
          />
        }
      />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    paddingTop: 20,
    paddingBottom: 24,
    paddingHorizontal: 20,
    borderBottomLeftRadius: 20,
    borderBottomRightRadius: 20,
  },
  headerContent: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'flex-end',
  },
  headerTitle: {
    fontSize: 28,
    fontWeight: '700',
    marginBottom: 4,
  },
  headerSubtitle: {
    fontSize: 14,
    fontWeight: '500',
    opacity: 0.9,
  },
  headerActions: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  statusIndicator: {
    width: 12,
    height: 12,
    borderRadius: 6,
  },
  searchContainer: {
    paddingHorizontal: 20,
    paddingVertical: 16,
    borderBottomWidth: 1,
    borderBottomColor: 'transparent',
  },
  searchBar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderRadius: 25,
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
  searchInput: {
    flex: 1,
    marginLeft: 12,
    fontSize: 16,
  },
  clearButton: {
    padding: 4,
  },
  listContainer: {
    flexGrow: 1,
    paddingHorizontal: 20,
    paddingTop: 8,
  },
  chatItem: {
    marginBottom: 8,
    borderRadius: 16,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 2 },
        shadowOpacity: 0.05,
        shadowRadius: 4,
      },
      android: {
        elevation: 1,
      },
    }),
  },
  chatItemContent: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 16,
  },
  avatar: {
    width: 52,
    height: 52,
    borderRadius: 26,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 16,
    position: 'relative',
  },
  avatarText: {
    fontSize: 18,
    fontWeight: '600',
  },
  onlineIndicator: {
    position: 'absolute',
    bottom: 2,
    right: 2,
    width: 14,
    height: 14,
    borderRadius: 7,
    borderWidth: 2,
    borderColor: 'white',
  },
  chatInfo: {
    flex: 1,
  },
  chatHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 4,
  },
  peerName: {
    fontSize: 16,
    fontWeight: '600',
    flex: 1,
  },
  timestamp: {
    fontSize: 12,
    fontWeight: '500',
  },
  messageRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  lastMessage: {
    fontSize: 14,
    flex: 1,
  },
  noMessages: {
    fontSize: 14,
    fontStyle: 'italic',
    flex: 1,
  },
  unreadBadge: {
    minWidth: 20,
    height: 20,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 6,
    marginLeft: 8,
  },
  unreadText: {
    fontSize: 12,
    fontWeight: '600',
  },
  emptyState: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
    paddingTop: 80,
  },
  emptyIcon: {
    width: 80,
    height: 80,
    borderRadius: 40,
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 24,
  },
  emptyTitle: {
    fontSize: 24,
    fontWeight: '600',
    marginBottom: 12,
    textAlign: 'center',
  },
  emptySubtitle: {
    fontSize: 16,
    textAlign: 'center',
    lineHeight: 22,
  },
});
