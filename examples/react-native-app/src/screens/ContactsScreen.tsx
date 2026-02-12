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
  Alert,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
// import { useNavigation } from '@react-navigation/native';
import { Icon } from '../components/Icon';
import LinearGradient from 'react-native-linear-gradient';
// import Animated, { FadeInDown, FadeInRight } from 'react-native-reanimated';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import {
  Contact,
  type ConnectionStatus,
  type IncomingConnectionRequest,
} from '../providers/ProtocolProvider';
import { MessagePriority } from '@offline-protocol/mesh-sdk';
import { getUserInitials, generateAvatarColor } from '../utils/user';

interface ContactItemProps {
  contact: Contact;
  connectionStatus: ConnectionStatus;
  onPress: () => void;
  onMessage: () => void;
  onSendConnectionRequest: () => void;
  onAcceptConnectionRequest: () => void;
  onRejectConnectionRequest: () => void;
  index: number;
}

function ContactItem({
  contact,
  connectionStatus,
  onPress,
  onMessage,
  onSendConnectionRequest,
  onAcceptConnectionRequest,
  onRejectConnectionRequest,
  index,
}: ContactItemProps) {
  const { theme } = useTheme();
  const avatarColor = generateAvatarColor(contact.id);
  const initials = getUserInitials(contact.name);

  const getDistanceIcon = (distance?: 'near' | 'medium' | 'far') => {
    switch (distance) {
      case 'near':
        return 'radio-button-on';
      case 'medium':
        return 'radio-button-off';
      case 'far':
        return 'ellipse-outline';
      default:
        return 'help-circle-outline';
    }
  };

  const getDistanceColor = (distance?: 'near' | 'medium' | 'far') => {
    switch (distance) {
      case 'near':
        return theme.colors.success;
      case 'medium':
        return theme.colors.warning;
      case 'far':
        return theme.colors.error;
      default:
        return theme.colors.textSecondary;
    }
  };

  const formatLastSeen = (lastSeen?: number) => {
    if (!lastSeen) return 'Never seen';
    
    const now = Date.now();
    const diffInMinutes = (now - lastSeen) / (1000 * 60);
    
    if (diffInMinutes < 1) return 'Just now';
    if (diffInMinutes < 60) return `${Math.floor(diffInMinutes)}m ago`;
    if (diffInMinutes < 1440) return `${Math.floor(diffInMinutes / 60)}h ago`;
    return `${Math.floor(diffInMinutes / 1440)}d ago`;
  };

  return (
    <View 
      style={[styles.contactItem, { backgroundColor: theme.colors.surface }]}
    >
      <View style={styles.contactContent}>
        {/* Avatar - Clickable for profile */}
        <TouchableOpacity onPress={onPress} activeOpacity={0.7}>
          <View style={[styles.avatar, { backgroundColor: avatarColor }]}>
            <Text style={[styles.avatarText, { color: theme.colors.textInverse }]}>
              {initials}
            </Text>
            {contact.isOnline && (
              <View style={[styles.onlineIndicator, { backgroundColor: theme.colors.online }]} />
            )}
          </View>
        </TouchableOpacity>

        {/* Contact Info - Clickable for profile */}
        <TouchableOpacity style={styles.contactInfo} onPress={onPress} activeOpacity={0.7}>
          <View style={styles.contactHeader}>
            <Text style={[styles.contactName, { color: theme.colors.text }]} numberOfLines={1}>
              {contact.name}
            </Text>
            <View style={styles.statusRow}>
              {contact.isOnline && contact.distance && (
                <Icon
                  name={getDistanceIcon(contact.distance)}
                  size={12}
                  color={getDistanceColor(contact.distance)}
                  style={{ marginRight: 4 }}
                />
              )}
              <Text style={[styles.status, { color: contact.isOnline ? theme.colors.online : theme.colors.textSecondary }]}>
                {contact.isOnline ? 'Online' : formatLastSeen(contact.lastSeen)}
              </Text>
            </View>
          </View>
          
          <View style={styles.contactDetails}>
            <Text style={[styles.contactId, { color: theme.colors.textSecondary }]} numberOfLines={1}>
              ID: {contact.id.slice(-8)}
            </Text>
            
            {contact.signalStrength !== undefined && (
              <View style={styles.signalContainer}>
                <Icon
                  name="wifi"
                  size={12}
                  color={theme.colors.textSecondary}
                  style={{ marginRight: 4 }}
                />
                <Text style={[styles.signalText, { color: theme.colors.textSecondary }]}>
                  {Math.round(contact.signalStrength * 100)}%
                </Text>
              </View>
            )}
          </View>
        </TouchableOpacity>

        {/* Actions: Connect / Request sent / Accept+Decline / Message */}
        <View style={styles.actions}>
          {connectionStatus === 'pending_received' && (
            <>
              <TouchableOpacity
                style={[styles.messageButton, styles.declineButton, { backgroundColor: theme.colors.surfaceVariant }]}
                onPress={onRejectConnectionRequest}
                activeOpacity={0.7}
                hitSlop={{ top: 15, bottom: 15, left: 15, right: 15 }}
              >
                <Icon name="close" size={18} color={theme.colors.text} />
              </TouchableOpacity>
              <TouchableOpacity
                style={[styles.messageButton, { backgroundColor: theme.colors.primary }]}
                onPress={onAcceptConnectionRequest}
                activeOpacity={0.7}
                hitSlop={{ top: 15, bottom: 15, left: 15, right: 15 }}
              >
                <Icon name="checkmark" size={18} color={theme.colors.textInverse} />
              </TouchableOpacity>
            </>
          )}
          {connectionStatus === 'pending_sent' && (
            <View style={[styles.pendingBadge, { backgroundColor: theme.colors.surfaceVariant }]}>
              <Text style={[styles.pendingText, { color: theme.colors.textSecondary }]}>
                Request sent
              </Text>
            </View>
          )}
          {(connectionStatus === 'none' || connectionStatus === 'rejected') && (
            <TouchableOpacity
              style={[styles.messageButton, { backgroundColor: theme.colors.primary }]}
              onPress={onSendConnectionRequest}
              activeOpacity={0.7}
              hitSlop={{ top: 15, bottom: 15, left: 15, right: 15 }}
            >
              <Icon name="person-add" size={18} color={theme.colors.textInverse} />
            </TouchableOpacity>
          )}
          {connectionStatus === 'connected' && (
            <TouchableOpacity
              style={[styles.messageButton, { backgroundColor: theme.colors.primary }]}
              onPress={() => {
                console.log(`[ContactItem] Message button tapped for ${contact.name}`);
                onMessage();
              }}
              activeOpacity={0.7}
              hitSlop={{ top: 15, bottom: 15, left: 15, right: 15 }}
            >
              <Icon name="chatbubble" size={18} color={theme.colors.textInverse} />
            </TouchableOpacity>
          )}
        </View>
      </View>
    </View>
  );
}

interface ContactsScreenProps {
  onNavigateToProfile: (userId: string) => void;
  onNavigateToChatDetail: (peerId: string, peerName: string) => void;
}

export function ContactsScreen({ onNavigateToProfile, onNavigateToChatDetail }: ContactsScreenProps) {
  const { theme } = useTheme();
  const {
    contacts,
    isOnline,
    connectedPeersCount,
    sendMessage,
    chats,
    incomingConnectionRequests,
    getConnectionStatus,
    sendConnectionRequest,
    acceptConnectionRequest,
    rejectConnectionRequest,
  } = useProtocol();
  const [searchQuery, setSearchQuery] = useState('');
  const [refreshing, setRefreshing] = useState(false);
  const [filter, setFilter] = useState<'all' | 'online' | 'offline'>('all');

  const filteredContacts = useMemo(() => {
    let filtered = contacts;

    // Apply search filter
    if (searchQuery.trim()) {
      filtered = filtered.filter(contact =>
        contact.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        contact.id.toLowerCase().includes(searchQuery.toLowerCase())
      );
    }

    // Apply status filter
    switch (filter) {
      case 'online':
        filtered = filtered.filter(contact => contact.isOnline);
        break;
      case 'offline':
        filtered = filtered.filter(contact => !contact.isOnline);
        break;
    }

    // Sort by online status first, then by name
    return filtered.sort((a, b) => {
      if (a.isOnline !== b.isOnline) {
        return a.isOnline ? -1 : 1;
      }
      return a.name.localeCompare(b.name);
    });
  }, [contacts, searchQuery, filter]);

  const handleContactPress = (contact: Contact) => {
    onNavigateToProfile(contact.id);
  };

  const handleMessage = (contact: Contact) => {
    console.log(`[ContactsScreen] Message button pressed for contact: ${contact.id} (${contact.name})`);
    const existingChat = chats.find(chat => chat.peerId === contact.id);
    if (existingChat) {
      onNavigateToChatDetail(contact.id, contact.name);
      return;
    }
    if (Platform.OS === 'ios') {
      Alert.prompt(
        `Message ${contact.name}`,
        'Enter your message:',
        [
          { text: 'Cancel', style: 'cancel' },
          {
            text: 'Send',
            onPress: async (text?: string) => {
              if (text?.trim()) {
                try {
                  await sendMessage(contact.id, text.trim(), MessagePriority.Medium);
                  onNavigateToChatDetail(contact.id, contact.name);
                } catch (error) {
                  Alert.alert('Send Failed', (error as Error)?.message ?? 'Failed to send message. Please try again.');
                }
              }
            },
          },
        ],
        'plain-text'
      );
      return;
    }
    onNavigateToChatDetail(contact.id, contact.name);
  };

  const handleSendConnectionRequest = async (contact: Contact) => {
    try {
      await sendConnectionRequest(contact.id);
    } catch (e) {
      Alert.alert('Request Failed', (e as Error)?.message ?? 'Failed to send connection request.');
    }
  };

  const handleAcceptConnectionRequest = async (senderId: string) => {
    try {
      await acceptConnectionRequest(senderId);
    } catch (e) {
      Alert.alert('Accept Failed', (e as Error)?.message ?? 'Failed to accept connection request.');
    }
  };

  const handleRejectConnectionRequest = async (senderId: string) => {
    try {
      await rejectConnectionRequest(senderId);
    } catch (e) {
      Alert.alert('Decline Failed', (e as Error)?.message ?? 'Failed to decline connection request.');
    }
  };

  const handleRefresh = async () => {
    setRefreshing(true);
    // In a real app, you might refresh the discovery or sync data
    setTimeout(() => setRefreshing(false), 1000);
  };

  const renderEmptyState = () => (
    <View style={styles.emptyState}>
      <View style={[styles.emptyIcon, { backgroundColor: theme.colors.surfaceVariant }]}>
        <Icon name="people-outline" size={48} color={theme.colors.textSecondary} />
      </View>
      <Text style={[styles.emptyTitle, { color: theme.colors.text }]}>
        {searchQuery.trim() ? 'No contacts found' : 'No contacts nearby'}
      </Text>
      <Text style={[styles.emptySubtitle, { color: theme.colors.textSecondary }]}>
        {searchQuery.trim() 
          ? 'Try adjusting your search terms.'
          : isOnline
            ? 'Make sure other devices have the app running and are nearby.'
            : 'Turn on the messenger to discover nearby devices.'
        }
      </Text>
    </View>
  );

  const getFilterCount = (filterType: 'all' | 'online' | 'offline') => {
    switch (filterType) {
      case 'online':
        return contacts.filter(c => c.isOnline).length;
      case 'offline':
        return contacts.filter(c => !c.isOnline).length;
      default:
        return contacts.length;
    }
  };

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
              Contacts
            </Text>
            <Text style={[styles.headerSubtitle, { color: theme.colors.textInverse }]}>
              {isOnline 
                ? `${connectedPeersCount} nearby • ${contacts.length} total`
                : 'Offline'
              }
            </Text>
          </View>
          
          <View style={styles.headerActions}>
            <Text style={[styles.headerDebug, { color: theme.colors.textInverse }]}>
              {filteredContacts.length} contacts
            </Text>
            <View style={[
              styles.statusIndicator,
              { backgroundColor: isOnline ? theme.colors.online : theme.colors.offline }
            ]} />
          </View>
        </View>
      </LinearGradient>

      {/* Search and Filter */}
      <View style={[styles.searchContainer, { backgroundColor: theme.colors.surface }]}>
        <View style={[styles.searchBar, { backgroundColor: theme.colors.background }]}>
          <Icon name="search" size={20} color={theme.colors.textSecondary} />
          <TextInput
            style={[styles.searchInput, { color: theme.colors.text }]}
            placeholder="Search contacts..."
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

        {/* Filter Tabs */}
        <View style={styles.filterContainer}>
          {(['all', 'online', 'offline'] as const).map((filterType) => (
            <TouchableOpacity
              key={filterType}
              style={[
                styles.filterTab,
                {
                  backgroundColor: filter === filterType ? theme.colors.primary : 'transparent',
                },
              ]}
              onPress={() => setFilter(filterType)}
              activeOpacity={0.7}
            >
              <Text
                style={[
                  styles.filterText,
                  {
                    color: filter === filterType ? theme.colors.textInverse : theme.colors.textSecondary,
                    fontWeight: filter === filterType ? '600' : '500',
                  },
                ]}
              >
                {filterType.charAt(0).toUpperCase() + filterType.slice(1)}
              </Text>
              <Text
                style={[
                  styles.filterCount,
                  {
                    color: filter === filterType ? theme.colors.textInverse : theme.colors.textSecondary,
                  },
                ]}
              >
                {getFilterCount(filterType)}
              </Text>
            </TouchableOpacity>
          ))}
        </View>
      </View>

      {/* Contacts List */}
      <FlatList
        data={filteredContacts}
        keyExtractor={(item) => item.id}
        ListHeaderComponent={
          incomingConnectionRequests.length > 0 ? (
            <View style={[styles.incomingSection, { backgroundColor: theme.colors.surfaceVariant }]}>
              <Text style={[styles.incomingSectionTitle, { color: theme.colors.textSecondary }]}>
                Connection requests
              </Text>
              {incomingConnectionRequests.map((req: IncomingConnectionRequest) => (
                <View
                  key={req.sender}
                  style={[styles.incomingRow, { backgroundColor: theme.colors.surface }]}
                >
                  <View style={styles.incomingRowInfo}>
                    <Text style={[styles.incomingRowName, { color: theme.colors.text }]} numberOfLines={1}>
                      {req.senderName || `User ${req.sender.slice(-4)}`}
                    </Text>
                    <Text style={[styles.incomingRowId, { color: theme.colors.textSecondary }]} numberOfLines={1}>
                      ID: {req.sender.slice(-8)}
                    </Text>
                  </View>
                  <View style={styles.incomingRowActions}>
                    <TouchableOpacity
                      style={[styles.incomingButton, styles.declineButton, { backgroundColor: theme.colors.surfaceVariant }]}
                      onPress={() => handleRejectConnectionRequest(req.sender)}
                    >
                      <Text style={[styles.incomingButtonText, { color: theme.colors.text }]}>Decline</Text>
                    </TouchableOpacity>
                    <TouchableOpacity
                      style={[styles.incomingButton, { backgroundColor: theme.colors.primary }]}
                      onPress={() => handleAcceptConnectionRequest(req.sender)}
                    >
                      <Text style={[styles.incomingButtonText, { color: theme.colors.textInverse }]}>Accept</Text>
                    </TouchableOpacity>
                  </View>
                </View>
              ))}
            </View>
          ) : null
        }
        renderItem={({ item, index }) => (
          <ContactItem
            contact={item}
            connectionStatus={getConnectionStatus(item.id)}
            onPress={() => handleContactPress(item)}
            onMessage={() => handleMessage(item)}
            onSendConnectionRequest={() => handleSendConnectionRequest(item)}
            onAcceptConnectionRequest={() => handleAcceptConnectionRequest(item.id)}
            onRejectConnectionRequest={() => handleRejectConnectionRequest(item.id)}
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
    paddingTop: 16,
    paddingBottom: 20,
    paddingHorizontal: 16,
    borderBottomLeftRadius: 16,
    borderBottomRightRadius: 16,
  },
  headerContent: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'flex-end',
  },
  headerTitle: {
    fontSize: 26,
    fontWeight: '700',
    marginBottom: 2,
  },
  headerSubtitle: {
    fontSize: 13,
    fontWeight: '500',
    opacity: 0.9,
  },
  headerActions: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  headerDebug: {
    fontSize: 11,
    fontWeight: '500',
    opacity: 0.8,
  },
  statusIndicator: {
    width: 10,
    height: 10,
    borderRadius: 5,
  },
  searchContainer: {
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  searchBar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 12,
    marginBottom: 12,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 1 },
        shadowOpacity: 0.08,
        shadowRadius: 3,
      },
      android: {
        elevation: 1,
      },
    }),
  },
  searchInput: {
    flex: 1,
    marginLeft: 10,
    fontSize: 16,
    paddingVertical: 0,
  },
  clearButton: {
    padding: 4,
  },
  filterContainer: {
    flexDirection: 'row',
    gap: 6,
  },
  filterTab: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 7,
    paddingHorizontal: 10,
    borderRadius: 10,
    gap: 4,
  },
  filterText: {
    fontSize: 13,
  },
  filterCount: {
    fontSize: 11,
    fontWeight: '600',
  },
  listContainer: {
    flexGrow: 1,
    paddingHorizontal: 16,
    paddingTop: 6,
    paddingBottom: 16,
  },
  contactItem: {
    marginBottom: 6,
    borderRadius: 14,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 1 },
        shadowOpacity: 0.04,
        shadowRadius: 3,
      },
      android: {
        elevation: 1,
      },
    }),
  },
  contactContent: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 12,
  },
  avatar: {
    width: 46,
    height: 46,
    borderRadius: 23,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 12,
    position: 'relative',
  },
  avatarText: {
    fontSize: 16,
    fontWeight: '600',
  },
  onlineIndicator: {
    position: 'absolute',
    bottom: 1,
    right: 1,
    width: 12,
    height: 12,
    borderRadius: 6,
    borderWidth: 2,
    borderColor: 'white',
  },
  contactInfo: {
    flex: 1,
    justifyContent: 'center',
  },
  contactHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 2,
  },
  contactName: {
    fontSize: 15,
    fontWeight: '600',
    flex: 1,
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  status: {
    fontSize: 11,
    fontWeight: '500',
  },
  contactDetails: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  contactId: {
    fontSize: 11,
    fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
    flex: 1,
  },
  signalContainer: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  signalText: {
    fontSize: 11,
    fontWeight: '500',
  },
  actions: {
    marginLeft: 10,
    justifyContent: 'center',
    flexDirection: 'row',
    gap: 8,
    alignItems: 'center',
  },
  messageButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 1 },
        shadowOpacity: 0.08,
        shadowRadius: 3,
      },
      android: {
        elevation: 1,
      },
    }),
  },
  declineButton: {},
  pendingBadge: {
    paddingHorizontal: 10,
    paddingVertical: 8,
    borderRadius: 10,
    justifyContent: 'center',
  },
  pendingText: {
    fontSize: 12,
    fontWeight: '500',
  },
  incomingSection: {
    padding: 12,
    borderRadius: 14,
    marginBottom: 12,
  },
  incomingSectionTitle: {
    fontSize: 12,
    fontWeight: '600',
    marginBottom: 8,
    textTransform: 'uppercase',
  },
  incomingRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    padding: 12,
    borderRadius: 12,
    marginBottom: 6,
  },
  incomingRowInfo: {
    flex: 1,
    marginRight: 12,
  },
  incomingRowName: {
    fontSize: 15,
    fontWeight: '600',
  },
  incomingRowId: {
    fontSize: 12,
    marginTop: 2,
  },
  incomingRowActions: {
    flexDirection: 'row',
    gap: 8,
  },
  incomingButton: {
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderRadius: 10,
  },
  incomingButtonText: {
    fontSize: 14,
    fontWeight: '600',
  },
  emptyState: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
    paddingTop: 60,
  },
  emptyIcon: {
    width: 72,
    height: 72,
    borderRadius: 36,
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 20,
  },
  emptyTitle: {
    fontSize: 20,
    fontWeight: '600',
    marginBottom: 10,
    textAlign: 'center',
  },
  emptySubtitle: {
    fontSize: 15,
    textAlign: 'center',
    lineHeight: 21,
  },
});
