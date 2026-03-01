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
  type ConnectionRequest,
} from '../providers/ProtocolProvider';
import { MessagePriority } from '@offline-protocol/mesh-sdk';
import { getUserInitials, generateAvatarColor } from '../utils/user';

interface RequestItemProps {
  request: ConnectionRequest;
  onAccept: () => void;
  onDecline: () => void;
  onPress: () => void;
  joining?: boolean;
}

function RequestItem({
  request,
  onAccept,
  onDecline,
  onPress,
  joining = false,
}: RequestItemProps) {
  const { theme } = useTheme();
  const avatarColor = generateAvatarColor(request.id);
  const initials = getUserInitials(request.name);
  const isIncoming = request.direction === 'incoming';

  return (
    <View
      style={[styles.contactItem, { backgroundColor: theme.colors.surface }]}
    >
      <View style={styles.contactContent}>
        <TouchableOpacity onPress={onPress} activeOpacity={0.7}>
          <View style={[styles.avatar, { backgroundColor: avatarColor }]}>
            <Text style={[styles.avatarText, { color: theme.colors.textInverse }]}>
              {initials}
            </Text>
          </View>
        </TouchableOpacity>
        <TouchableOpacity
          style={styles.contactInfo}
          onPress={onPress}
          activeOpacity={0.7}
        >
          <View style={styles.contactHeader}>
            <Text
              style={[styles.contactName, { color: theme.colors.text }]}
              numberOfLines={1}
            >
              {request.name}
            </Text>
            <Text
              style={[styles.status, { color: theme.colors.textSecondary }]}
            >
              {isIncoming ? 'Secure session invite' : 'Invite sent'}
            </Text>
          </View>
          <Text
            style={[styles.contactId, { color: theme.colors.textSecondary }]}
            numberOfLines={1}
          >
            ID: {request.id.slice(-8)}
          </Text>
        </TouchableOpacity>
        <View style={styles.actions}>
          {isIncoming ? (
            <View style={styles.requestActions}>
              <TouchableOpacity
                style={[
                  styles.requestButton,
                  styles.requestDeclineButton,
                  { backgroundColor: theme.colors.surfaceVariant },
                ]}
                onPress={onDecline}
                activeOpacity={0.7}
              >
                <Icon name="close" size={18} color={theme.colors.text} />
                <Text style={[styles.requestButtonLabel, { color: theme.colors.text }]}>
                  Ignore
                </Text>
              </TouchableOpacity>
              <TouchableOpacity
                style={[
                  styles.requestButton,
                  styles.requestAcceptButton,
                  { backgroundColor: theme.colors.primary },
                  joining && { opacity: 0.7 },
                ]}
                onPress={onAccept}
                disabled={joining}
                activeOpacity={0.7}
              >
                <Icon name="checkmark" size={18} color={theme.colors.textInverse} />
                <Text style={[styles.requestButtonLabel, { color: theme.colors.textInverse }]}>
                  {joining ? 'Joining...' : 'Join'}
                </Text>
              </TouchableOpacity>
            </View>
          ) : (
            <View
              style={[
                styles.pendingBadge,
                { backgroundColor: theme.colors.surfaceVariant },
              ]}
            >
              <Text
                style={[
                  styles.pendingText,
                  { color: theme.colors.textSecondary },
                ]}
              >
                Pending
              </Text>
            </View>
          )}
        </View>
      </View>
    </View>
  );
}

interface NeighborItemProps {
  neighbor: Contact;
  hasPendingSentRequest: boolean;
  onPress: () => void;
  onSendRequest: () => void;
}

function NeighborItem({
  neighbor,
  hasPendingSentRequest,
  onPress,
  onSendRequest,
}: NeighborItemProps) {
  const { theme } = useTheme();
  const avatarColor = generateAvatarColor(neighbor.id);
  const initials = getUserInitials(neighbor.name);

  return (
    <View
      style={[styles.contactItem, { backgroundColor: theme.colors.surface }]}
    >
      <View style={styles.contactContent}>
        <TouchableOpacity onPress={onPress} activeOpacity={0.7}>
          <View style={[styles.avatar, { backgroundColor: avatarColor }]}>
            <Text style={[styles.avatarText, { color: theme.colors.textInverse }]}>
              {initials}
            </Text>
            <View
              style={[
                styles.onlineIndicator,
                { backgroundColor: theme.colors.online },
              ]}
            />
          </View>
        </TouchableOpacity>
        <TouchableOpacity
          style={styles.contactInfo}
          onPress={onPress}
          activeOpacity={0.7}
        >
          <View style={styles.contactHeader}>
            <Text
              style={[styles.contactName, { color: theme.colors.text }]}
              numberOfLines={1}
            >
              {neighbor.name}
            </Text>
            <Text
              style={[styles.status, { color: theme.colors.online }]}
            >
              Nearby
            </Text>
          </View>
          <Text
            style={[styles.contactId, { color: theme.colors.textSecondary }]}
            numberOfLines={1}
          >
            ID: {neighbor.id.slice(-8)}
          </Text>
        </TouchableOpacity>
        <View style={styles.actions}>
          {hasPendingSentRequest ? (
            <View
              style={[
                styles.pendingBadge,
                { backgroundColor: theme.colors.surfaceVariant },
              ]}
            >
              <Text
                style={[styles.pendingText, { color: theme.colors.textSecondary }]}
              >
                Pending
              </Text>
            </View>
          ) : (
            <TouchableOpacity
              style={[styles.messageButton, { backgroundColor: theme.colors.primary }]}
              onPress={onSendRequest}
              activeOpacity={0.7}
            >
              <Icon name="person-add" size={18} color={theme.colors.textInverse} />
            </TouchableOpacity>
          )}
        </View>
      </View>
    </View>
  );
}

interface ContactItemProps {
  contact: Contact;
  onPress: () => void;
  onMessage: () => void;
  index: number;
}

function ContactItem({ contact, onPress, onMessage, index: _index }: ContactItemProps) {
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

        {/* Message Button - Separate touchable */}
        <View style={styles.actions}>
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
    connectionRequests,
    neighbors,
    isOnline,
    connectedPeersCount,
    sendMessage,
    sendConnectionRequest,
    acceptConnectionRequest,
    rejectConnectionRequest,
    chats,
  } = useProtocol();
  const [searchQuery, setSearchQuery] = useState('');
  const [refreshing, setRefreshing] = useState(false);
  const [filter, setFilter] = useState<'contacts' | 'requests' | 'neighbors'>(
    'contacts',
  );
  const [joiningRequestIds, setJoiningRequestIds] = useState<Set<string>>(new Set());

  const filteredContacts = useMemo(() => {
    let filtered = contacts;
    if (searchQuery.trim()) {
      filtered = filtered.filter(
        contact =>
          contact.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
          contact.id.toLowerCase().includes(searchQuery.toLowerCase()),
      );
    }
    return filtered.sort((a, b) => a.name.localeCompare(b.name));
  }, [contacts, searchQuery]);

  const filteredRequests = useMemo(() => {
    let filtered = connectionRequests;
    if (searchQuery.trim()) {
      filtered = filtered.filter(
        req =>
          req.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
          req.id.toLowerCase().includes(searchQuery.toLowerCase()),
      );
    }
    return filtered.sort((a, b) => b.timestamp - a.timestamp);
  }, [connectionRequests, searchQuery]);

  const filteredNeighbors = useMemo(() => {
    let filtered = neighbors;
    if (searchQuery.trim()) {
      filtered = filtered.filter(
        n =>
          n.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
          n.id.toLowerCase().includes(searchQuery.toLowerCase()),
      );
    }
    return filtered.sort((a, b) => a.name.localeCompare(b.name));
  }, [neighbors, searchQuery]);

  const handleContactPress = (contact: Contact) => {
    onNavigateToProfile(contact.id);
  };

  const handleMessage = (contact: Contact) => {
    console.log(`[ContactsScreen] Message button pressed for contact: ${contact.id} (${contact.name})`);
    
    // Check if chat already exists
    const existingChat = chats.find(chat => chat.peerId === contact.id);
    
    if (existingChat) {
      console.log(`[ContactsScreen] Existing chat found, navigating to chat detail`);
      onNavigateToChatDetail(contact.id, contact.name);
      return;
    }

    if (Platform.OS === 'ios') {
      console.log(`[ContactsScreen] No existing chat, showing message prompt`);
      Alert.prompt(
        `Message ${contact.name}`,
        'Enter your message:',
        [
          { text: 'Cancel', style: 'cancel' },
          {
            text: 'Send',
            onPress: async (text?: string) => {
              console.log(`[ContactsScreen] Prompt response: "${text}"`);
              if (text?.trim()) {
                try {
                  console.log(`[ContactsScreen] Calling sendMessage for ${contact.id}`);
                  await sendMessage(contact.id, text.trim(), MessagePriority.Medium);
                  console.log(`[ContactsScreen] Message sent, navigating to chat`);
                  onNavigateToChatDetail(contact.id, contact.name);
                } catch (error) {
                  console.error(`[ContactsScreen] Failed to send message:`, error);
                  Alert.alert('Send Failed', 'Failed to send message. Please try again.');
                }
              }
            },
          },
        ],
        'plain-text'
      );
      return;
    }

    console.log(`[ContactsScreen] Android detected, navigating directly to chat detail`);
    onNavigateToChatDetail(contact.id, contact.name);
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

  const getFilterCount = (
    filterType: 'contacts' | 'requests' | 'neighbors',
  ) => {
    switch (filterType) {
      case 'contacts':
        return contacts.length;
      case 'requests':
        return connectionRequests.length;
      case 'neighbors':
        return neighbors.length;
      default:
        return 0;
    }
  };

  const hasSentRequestToNeighbor = (id: string) =>
    connectionRequests.some(r => r.id === id && r.direction === 'sent');

  const handleAcceptRequest = async (request: ConnectionRequest) => {
    if (joiningRequestIds.has(request.id)) {
      return;
    }
    setJoiningRequestIds(prev => new Set(prev).add(request.id));
    try {
      await acceptConnectionRequest(request.id);
      Alert.alert(
        'Join Requested',
        'Secure session acceptance sent. Waiting for peer confirmation.',
      );
    } catch (e) {
      Alert.alert(
        'Join Failed',
        (e as Error)?.message ?? 'Failed to join secure session invite.',
      );
    } finally {
      setJoiningRequestIds(prev => {
        const next = new Set(prev);
        next.delete(request.id);
        return next;
      });
    }
  };

  const handleDeclineRequest = async (request: ConnectionRequest) => {
    try {
      await rejectConnectionRequest(request.id);
    } catch (e) {
      Alert.alert(
        'Ignore Failed',
        (e as Error)?.message ?? 'Failed to ignore secure session invite.',
      );
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
                ? `${connectedPeersCount} nearby • ${contacts.length} contacts`
                : 'Offline'}
            </Text>
          </View>

          <View style={styles.headerActions}>
            <Text style={[styles.headerDebug, { color: theme.colors.textInverse }]}>
              {filter === 'contacts'
                ? `${filteredContacts.length} contacts`
                : filter === 'requests'
                  ? `${filteredRequests.length} invites`
                  : `${filteredNeighbors.length} nearby`}
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

        {/* Filter Tabs: Contacts | Invites | Neighbors */}
        <View style={styles.filterContainer}>
          {(['contacts', 'requests', 'neighbors'] as const).map(filterType => (
            <TouchableOpacity
              key={filterType}
              style={[
                styles.filterTab,
                {
                  backgroundColor:
                    filter === filterType ? theme.colors.primary : 'transparent',
                },
              ]}
              onPress={() => setFilter(filterType)}
              activeOpacity={0.7}
            >
              <Text
                style={[
                  styles.filterText,
                  {
                    color:
                      filter === filterType
                        ? theme.colors.textInverse
                        : theme.colors.textSecondary,
                    fontWeight: filter === filterType ? '600' : '500',
                  },
                ]}
              >
                {filterType === 'requests'
                  ? 'Invites'
                  : filterType.charAt(0).toUpperCase() + filterType.slice(1)}
              </Text>
              <Text
                style={[
                  styles.filterCount,
                  {
                    color:
                      filter === filterType
                        ? theme.colors.textInverse
                        : theme.colors.textSecondary,
                  },
                ]}
              >
                {getFilterCount(filterType)}
              </Text>
            </TouchableOpacity>
          ))}
        </View>
      </View>

      {/* Contacts, Invites, or Neighbors List */}
      {filter === 'contacts' ? (
        <FlatList
          data={filteredContacts}
          keyExtractor={item => item.id}
          renderItem={({ item, index }) => (
            <ContactItem
              contact={item}
              onPress={() => handleContactPress(item)}
              onMessage={() => handleMessage(item)}
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
      ) : filter === 'requests' ? (
        <FlatList
          data={filteredRequests}
          keyExtractor={item => `${item.id}-${item.direction}-${item.timestamp}`}
          renderItem={({ item }) => (
            <RequestItem
              request={item}
              onPress={() => onNavigateToProfile(item.id)}
              onAccept={() => handleAcceptRequest(item)}
              onDecline={() => handleDeclineRequest(item)}
              joining={joiningRequestIds.has(item.id)}
            />
          )}
          contentContainerStyle={styles.listContainer}
          showsVerticalScrollIndicator={false}
          ListEmptyComponent={() => (
            <View style={styles.emptyState}>
              <View
                style={[
                  styles.emptyIcon,
                  { backgroundColor: theme.colors.surfaceVariant },
                ]}
              >
                <Icon
                  name="mail-outline"
                  size={48}
                  color={theme.colors.textSecondary}
                />
              </View>
              <Text style={[styles.emptyTitle, { color: theme.colors.text }]}>
                {searchQuery.trim() ? 'No invites found' : 'No secure session invites'}
              </Text>
              <Text
                style={[styles.emptySubtitle, { color: theme.colors.textSecondary }]}
              >
                {searchQuery.trim()
                  ? 'Try adjusting your search.'
                  : 'Incoming and sent secure session invites will appear here.'}
              </Text>
            </View>
          )}
          refreshControl={
            <RefreshControl
              refreshing={refreshing}
              onRefresh={handleRefresh}
              tintColor={theme.colors.primary}
              colors={[theme.colors.primary]}
            />
          }
        />
      ) : (
        <FlatList
          data={filteredNeighbors}
          keyExtractor={item => item.id}
          renderItem={({ item }) => (
            <NeighborItem
              neighbor={item}
              hasPendingSentRequest={hasSentRequestToNeighbor(item.id)}
              onPress={() => onNavigateToProfile(item.id)}
              onSendRequest={async () => {
                try {
                  await sendConnectionRequest(item.id);
                } catch (e) {
                  Alert.alert(
                    'Invite Failed',
                    (e as Error)?.message ?? 'Failed to send secure session invite.',
                  );
                }
              }}
            />
          )}
          contentContainerStyle={styles.listContainer}
          showsVerticalScrollIndicator={false}
          ListEmptyComponent={() => (
            <View style={styles.emptyState}>
              <View
                style={[
                  styles.emptyIcon,
                  { backgroundColor: theme.colors.surfaceVariant },
                ]}
              >
                <Icon
                  name="radio-outline"
                  size={48}
                  color={theme.colors.textSecondary}
                />
              </View>
              <Text style={[styles.emptyTitle, { color: theme.colors.text }]}>
                {searchQuery.trim() ? 'No neighbors found' : 'No nearby devices'}
              </Text>
              <Text
                style={[styles.emptySubtitle, { color: theme.colors.textSecondary }]}
              >
                {searchQuery.trim()
                  ? 'Try adjusting your search.'
                  : isOnline
                    ? 'Devices with the app open nearby will appear here.'
                    : 'Turn on the messenger to discover nearby devices.'}
              </Text>
            </View>
          )}
          refreshControl={
            <RefreshControl
              refreshing={refreshing}
              onRefresh={handleRefresh}
              tintColor={theme.colors.primary}
              colors={[theme.colors.primary]}
            />
          }
        />
      )}
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
  },
  requestActions: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  requestButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 12,
    minHeight: 44,
    minWidth: 100,
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
  requestAcceptButton: {},
  requestDeclineButton: {},
  requestButtonLabel: {
    fontSize: 15,
    fontWeight: '600',
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
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
  },
  pendingText: {
    fontSize: 13,
    fontWeight: '500',
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
