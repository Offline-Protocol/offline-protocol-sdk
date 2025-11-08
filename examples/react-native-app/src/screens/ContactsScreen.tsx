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
import { Contact } from '../providers/ProtocolProvider';
import { MessagePriority } from '@offlineprotocol/react-native';
import { getUserInitials, generateAvatarColor } from '../utils/user';

interface ContactItemProps {
  contact: Contact;
  onPress: () => void;
  onMessage: () => void;
  index: number;
}

function ContactItem({ contact, onPress, onMessage, index }: ContactItemProps) {
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
  const { contacts, isOnline, connectedPeersCount, sendMessage, chats } = useProtocol();
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
    gap: 8,
  },
  headerDebug: {
    fontSize: 12,
    fontWeight: '500',
    opacity: 0.8,
  },
  statusIndicator: {
    width: 12,
    height: 12,
    borderRadius: 6,
  },
  searchContainer: {
    paddingHorizontal: 20,
    paddingVertical: 16,
  },
  searchBar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderRadius: 25,
    marginBottom: 16,
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
  filterContainer: {
    flexDirection: 'row',
    gap: 8,
  },
  filterTab: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 8,
    paddingHorizontal: 12,
    borderRadius: 20,
    gap: 4,
  },
  filterText: {
    fontSize: 14,
  },
  filterCount: {
    fontSize: 12,
    fontWeight: '600',
  },
  listContainer: {
    flexGrow: 1,
    paddingHorizontal: 20,
    paddingTop: 8,
  },
  contactItem: {
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
  contactContent: {
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
  contactInfo: {
    flex: 1,
  },
  contactHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 4,
  },
  contactName: {
    fontSize: 16,
    fontWeight: '600',
    flex: 1,
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  status: {
    fontSize: 12,
    fontWeight: '500',
  },
  contactDetails: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  contactId: {
    fontSize: 12,
    fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
    flex: 1,
  },
  signalContainer: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  signalText: {
    fontSize: 12,
    fontWeight: '500',
  },
  actions: {
    marginLeft: 12,
    justifyContent: 'center',
  },
  messageButton: {
    width: 44,
    height: 44,
    borderRadius: 22,
    alignItems: 'center',
    justifyContent: 'center',
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
