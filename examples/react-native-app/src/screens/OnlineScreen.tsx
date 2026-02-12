import React, { useState, useCallback, useRef, useEffect } from 'react';
import {
  View,
  Text,
  StyleSheet,
  ScrollView,
  TouchableOpacity,
  TextInput,
  FlatList,
  KeyboardAvoidingView,
  Platform,
  Alert,
  ActivityIndicator,
} from 'react-native';
import LinearGradient from 'react-native-linear-gradient';
import { Icon } from '../components/Icon';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import type { OnlineMessage } from '../providers/ProtocolProvider';

type Tab = 'chat' | 'users' | 'groups';

export function OnlineScreen() {
  const { theme } = useTheme();
  const {
    currentUserName,
    currentUserId,
    sendMessage: protocolSendMessage,
    relayReady,
    error,
    protocol,
  } = useProtocol();
  const flatListRef = useRef<FlatList>(null);

  const [activeTab, setActiveTab] = useState<Tab>('chat');
  const [usernameInput, setUsernameInput] = useState(currentUserName || 'Me');
  const [recipientId, setRecipientId] = useState('');
  const [messageInput, setMessageInput] = useState('');
  const [checkUserId, setCheckUserId] = useState('');
  const [messages, setMessages] = useState<OnlineMessage[]>([]);
  const [onlineUsers] = useState<Map<string, { userId: string; username: string; isOnline: boolean; lastSeen?: Date }>>(new Map());
  const [groups, setGroups] = useState<Array<{ groupId: string; name: string; createdAt: Date }>>([]);

  // Subscribe to protocol message_received for direct messages in this tab
  useEffect(() => {
    if (!protocol) return;
    const onMessage = (event: { type: string; sender?: string; content?: string; message_id?: string; timestamp?: number }) => {
      if (event.type !== 'message_received' || !event.sender || !event.content) return;
      const msg: OnlineMessage = {
        id: event.message_id ?? `msg_${Date.now()}`,
        sender: event.sender,
        content: event.content,
        timestamp: event.timestamp ? new Date(event.timestamp) : new Date(),
        isFromMe: event.sender === currentUserId,
      };
      setMessages(prev => [...prev, msg]);
    };
    protocol.on('all', onMessage as (e: unknown) => void);
    return () => { protocol.off?.('all', onMessage as (e: unknown) => void); };
  }, [protocol, currentUserId]);

  const handleSendMessage = useCallback(async () => {
    if (!recipientId.trim() || !messageInput.trim()) {
      Alert.alert('Error', 'Please enter recipient ID and message');
      return;
    }
    try {
      await protocolSendMessage(recipientId.trim(), messageInput.trim());
      setMessageInput('');
    } catch (e) {
      Alert.alert('Error', (e as Error)?.message ?? 'Failed to send');
    }
  }, [recipientId, messageInput, protocolSendMessage]);

  const clearMessages = useCallback(() => setMessages([]), []);

  const handleCheckPresence = useCallback(() => {
    if (!checkUserId.trim()) {
      Alert.alert('Error', 'Please enter a user ID to check');
      return;
    }
    Alert.alert('Presence', 'Presence check is not available when using native relay.');
  }, [checkUserId]);

  // Sync username input with profile username
  useEffect(() => {
    if (currentUserName && currentUserName !== 'Me') {
      setUsernameInput(currentUserName);
    }
  }, [currentUserName]);

  useEffect(() => {
    if (messages.length > 0) {
      setTimeout(() => {
        flatListRef.current?.scrollToEnd({ animated: true });
      }, 100);
    }
  }, [messages.length]);

  const getStatusColor = () =>
    relayReady ? theme.colors.online : theme.colors.offline;
  const getStatusLabel = () =>
    relayReady ? 'Relay ready' : 'Waiting for relay...';

  const renderConnectionCard = () => (
    <View style={[styles.card, { backgroundColor: theme.colors.surface }]}>
      <View style={styles.cardHeader}>
        <View style={styles.statusRow}>
          <View
            style={[styles.statusDot, { backgroundColor: getStatusColor() }]}
          />
          <Text style={[styles.statusLabel, { color: theme.colors.text }]}>
            {getStatusLabel()}
          </Text>
        </View>
      </View>

      <View style={styles.userInfo}>
        <Text
          style={[styles.userLabel, { color: theme.colors.textSecondary }]}
        >
          Logged in as:
        </Text>
        <Text style={[styles.userName, { color: theme.colors.text }]}>
          {currentUserName || 'Me'}
        </Text>
        <Text style={[styles.userId, { color: theme.colors.textSecondary }]}>
          ID: {currentUserId}
        </Text>
      </View>

      {error && (
        <View
          style={[
            styles.errorBox,
            { backgroundColor: theme.colors.error + '20' },
          ]}
        >
          <Icon name="alert-circle" size={16} color={theme.colors.error} />
          <Text style={[styles.errorText, { color: theme.colors.error }]}>
            {error}
          </Text>
        </View>
      )}

      <View style={styles.buttonRow}>
        <Text style={[styles.buttonText, { color: theme.colors.textSecondary }]}>
          {relayReady ? 'Relay connected via native SDK' : 'Waiting for relay...'}
        </Text>
      </View>
    </View>
  );

  const renderMessageBubble = ({ item }: { item: OnlineMessage }) => (
    <View
      style={[
        styles.messageBubble,
        item.isFromMe
          ? [styles.myMessage, { backgroundColor: theme.colors.primary }]
          : [styles.theirMessage, { backgroundColor: theme.colors.surface }],
      ]}
    >
      {!item.isFromMe && (
        <Text
          style={[styles.messageSender, { color: theme.colors.textSecondary }]}
        >
          {item.sender}
        </Text>
      )}
      <Text
        style={[
          styles.messageText,
          {
            color: item.isFromMe ? theme.colors.textInverse : theme.colors.text,
          },
        ]}
      >
        {item.content}
      </Text>
      <Text
        style={[
          styles.messageTime,
          {
            color: item.isFromMe
              ? theme.colors.textInverse
              : theme.colors.textSecondary,
          },
        ]}
      >
        {item.timestamp.toLocaleTimeString([], {
          hour: '2-digit',
          minute: '2-digit',
        })}
      </Text>
    </View>
  );

  const renderChatTab = () => (
    <View style={styles.tabContent}>
      {!relayReady ? (
        <View style={styles.centeredMessage}>
          <Icon
            name="lock-closed"
            size={48}
            color={theme.colors.textSecondary}
          />
          <Text
            style={[styles.centeredText, { color: theme.colors.textSecondary }]}
          >
            Waiting for relay to start chatting
          </Text>
        </View>
      ) : (
        <>
          <View
            style={[
              styles.recipientInput,
              { backgroundColor: theme.colors.surface },
            ]}
          >
            <Text
              style={[styles.inputLabel, { color: theme.colors.textSecondary }]}
            >
              Recipient ID:
            </Text>
            <TextInput
              style={[
                styles.textInput,
                {
                  color: theme.colors.text,
                  backgroundColor: theme.colors.background,
                },
              ]}
              value={recipientId}
              onChangeText={setRecipientId}
              placeholder="Enter user ID..."
              placeholderTextColor={theme.colors.textSecondary}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>

          <View style={styles.messagesContainer}>
            {messages.length === 0 ? (
              <View style={styles.emptyMessages}>
                <Icon
                  name="chatbubbles-outline"
                  size={48}
                  color={theme.colors.textSecondary}
                />
                <Text
                  style={[
                    styles.emptyText,
                    { color: theme.colors.textSecondary },
                  ]}
                >
                  No messages yet
                </Text>
                <Text
                  style={[
                    styles.emptySubtext,
                    { color: theme.colors.textSecondary },
                  ]}
                >
                  Send a message to start the conversation
                </Text>
              </View>
            ) : (
              <FlatList
                ref={flatListRef}
                data={messages}
                keyExtractor={item => item.id}
                renderItem={renderMessageBubble}
                contentContainerStyle={styles.messagesList}
                showsVerticalScrollIndicator={false}
              />
            )}
          </View>

          <View
            style={[
              styles.inputContainer,
              { backgroundColor: theme.colors.surface },
            ]}
          >
            <View
              style={[
                styles.inputWrapper,
                { backgroundColor: theme.colors.background },
              ]}
            >
              <TextInput
                style={[styles.messageInput, { color: theme.colors.text }]}
                value={messageInput}
                onChangeText={setMessageInput}
                placeholder="Type a message..."
                placeholderTextColor={theme.colors.textSecondary}
                multiline
                maxLength={500}
              />
            </View>
            <TouchableOpacity
              style={[
                styles.sendButton,
                {
                  backgroundColor:
                    messageInput.trim() && recipientId.trim()
                      ? theme.colors.primary
                      : theme.colors.border,
                },
              ]}
              onPress={handleSendMessage}
              disabled={!messageInput.trim() || !recipientId.trim()}
            >
              <Icon
                name="send"
                size={20}
                color={
                  messageInput.trim() && recipientId.trim()
                    ? theme.colors.textInverse
                    : theme.colors.textSecondary
                }
              />
            </TouchableOpacity>
          </View>

          {messages.length > 0 && (
            <TouchableOpacity
              style={[
                styles.clearButton,
                { backgroundColor: theme.colors.surface },
              ]}
              onPress={clearMessages}
            >
              <Icon name="trash-outline" size={16} color={theme.colors.error} />
              <Text
                style={[styles.clearButtonText, { color: theme.colors.error }]}
              >
                Clear Messages
              </Text>
            </TouchableOpacity>
          )}
        </>
      )}
    </View>
  );

  const renderUsersTab = () => (
    <View style={styles.tabContent}>
      {!relayReady ? (
        <View style={styles.centeredMessage}>
          <Icon name="people" size={48} color={theme.colors.textSecondary} />
          <Text
            style={[styles.centeredText, { color: theme.colors.textSecondary }]}
          >
            Connect and authenticate to check user presence
          </Text>
        </View>
      ) : (
        <ScrollView showsVerticalScrollIndicator={false}>
          <View
            style={[styles.card, { backgroundColor: theme.colors.surface }]}
          >
            <Text style={[styles.cardTitle, { color: theme.colors.text }]}>
              Check User Presence
            </Text>
            <View style={styles.presenceInputRow}>
              <TextInput
                style={[
                  styles.textInput,
                  styles.presenceInput,
                  {
                    color: theme.colors.text,
                    backgroundColor: theme.colors.background,
                  },
                ]}
                value={checkUserId}
                onChangeText={setCheckUserId}
                placeholder="Enter user ID..."
                placeholderTextColor={theme.colors.textSecondary}
                autoCapitalize="none"
                autoCorrect={false}
              />
              <TouchableOpacity
                style={[
                  styles.checkButton,
                  { backgroundColor: theme.colors.primary },
                ]}
                onPress={handleCheckPresence}
              >
                <Icon
                  name="search"
                  size={20}
                  color={theme.colors.textInverse}
                />
              </TouchableOpacity>
            </View>
          </View>

          <View
            style={[styles.card, { backgroundColor: theme.colors.surface }]}
          >
            <Text style={[styles.cardTitle, { color: theme.colors.text }]}>
              Known Users ({onlineUsers.size})
            </Text>
            {onlineUsers.size === 0 ? (
              <Text
                style={[
                  styles.emptyText,
                  { color: theme.colors.textSecondary },
                ]}
              >
                No users checked yet
              </Text>
            ) : (
              Array.from(onlineUsers.values()).map(user => (
                <View key={user.userId} style={styles.userItem}>
                  <View style={styles.userItemLeft}>
                    <View
                      style={[
                        styles.userStatusDot,
                        {
                          backgroundColor: user.isOnline
                            ? theme.colors.online
                            : theme.colors.offline,
                        },
                      ]}
                    />
                    <View>
                      <Text
                        style={[
                          styles.userItemName,
                          { color: theme.colors.text },
                        ]}
                      >
                        {user.username}
                      </Text>
                      <Text
                        style={[
                          styles.userItemId,
                          { color: theme.colors.textSecondary },
                        ]}
                      >
                        {user.userId}
                      </Text>
                    </View>
                  </View>
                  <Text
                    style={[
                      styles.userStatus,
                      {
                        color: user.isOnline
                          ? theme.colors.online
                          : theme.colors.textSecondary,
                      },
                    ]}
                  >
                    {user.isOnline
                      ? 'Online'
                      : user.lastSeen
                      ? `Last seen ${user.lastSeen.toLocaleTimeString()}`
                      : 'Offline'}
                  </Text>
                </View>
              ))
            )}
          </View>
        </ScrollView>
      )}
    </View>
  );

  const renderGroupsTab = () => (
    <View style={styles.tabContent}>
      {!relayReady ? (
        <View style={styles.centeredMessage}>
          <Icon
            name="people-circle"
            size={48}
            color={theme.colors.textSecondary}
          />
          <Text
            style={[styles.centeredText, { color: theme.colors.textSecondary }]}
          >
            Connect and authenticate to manage groups
          </Text>
        </View>
      ) : (
        <ScrollView showsVerticalScrollIndicator={false}>
          <View
            style={[styles.card, { backgroundColor: theme.colors.surface }]}
          >
            <Text style={[styles.cardTitle, { color: theme.colors.text }]}>
              Your Groups ({groups.length})
            </Text>
            {groups.length === 0 ? (
              <Text
                style={[
                  styles.emptyText,
                  { color: theme.colors.textSecondary },
                ]}
              >
                No groups yet
              </Text>
            ) : (
              groups.map(group => (
                <View key={group.groupId} style={styles.groupItem}>
                  <Icon
                    name="people-circle"
                    size={24}
                    color={theme.colors.primary}
                  />
                  <View style={styles.groupItemInfo}>
                    <Text
                      style={[
                        styles.groupItemName,
                        { color: theme.colors.text },
                      ]}
                    >
                      {group.name}
                    </Text>
                    <Text
                      style={[
                        styles.groupItemId,
                        { color: theme.colors.textSecondary },
                      ]}
                    >
                      {group.groupId}
                    </Text>
                  </View>
                </View>
              ))
            )}
          </View>

          <View
            style={[
              styles.infoBox,
              { backgroundColor: theme.colors.primary + '15' },
            ]}
          >
            <Icon
              name="information-circle"
              size={20}
              color={theme.colors.primary}
            />
            <Text style={[styles.infoText, { color: theme.colors.text }]}>
              Group management uses the SDK relay connection. Create groups,
              add members, and send group messages.
            </Text>
          </View>
        </ScrollView>
      )}
    </View>
  );

  const tabs = [
    { id: 'chat' as const, label: 'Chat', icon: 'chatbubbles' },
    { id: 'users' as const, label: 'Users', icon: 'people' },
    { id: 'groups' as const, label: 'Groups', icon: 'people-circle' },
  ];

  const renderTabContent = () => {
    if (activeTab === 'chat') return renderChatTab();
    if (activeTab === 'users') return renderUsersTab();
    if (activeTab === 'groups') return renderGroupsTab();
    return null;
  };

  // Chat tab has FlatList, so we don't wrap it in ScrollView
  const shouldUseScrollView =
    activeTab !== 'chat' || !relayReady || messages.length === 0;

  return (
    <KeyboardAvoidingView
      style={[styles.container, { backgroundColor: theme.colors.background }]}
      behavior={Platform.OS === 'ios' ? 'padding' : 'height'}
    >
      <LinearGradient
        colors={[theme.colors.primary, theme.colors.primaryDark]}
        style={styles.header}
      >
        <View style={styles.headerContent}>
          <Text
            style={[styles.headerTitle, { color: theme.colors.textInverse }]}
          >
            Online Transport
          </Text>
          <Text
            style={[styles.headerSubtitle, { color: theme.colors.textInverse }]}
          >
            Relay (SDK connection only)
          </Text>
        </View>
      </LinearGradient>

      {shouldUseScrollView ? (
        <ScrollView
          style={styles.content}
          contentContainerStyle={styles.scrollContent}
          showsVerticalScrollIndicator={false}
          keyboardShouldPersistTaps="handled"
        >
          {renderConnectionCard()}

          <View
            style={[styles.tabBar, { backgroundColor: theme.colors.surface }]}
          >
            {tabs.map(tab => (
              <TouchableOpacity
                key={tab.id}
                style={[
                  styles.tab,
                  activeTab === tab.id && [
                    styles.activeTab,
                    { borderColor: theme.colors.primary },
                  ],
                ]}
                onPress={() => setActiveTab(tab.id)}
              >
                <Icon
                  name={tab.icon}
                  size={20}
                  color={
                    activeTab === tab.id
                      ? theme.colors.primary
                      : theme.colors.textSecondary
                  }
                />
                <Text
                  style={[
                    styles.tabLabel,
                    {
                      color:
                        activeTab === tab.id
                          ? theme.colors.primary
                          : theme.colors.textSecondary,
                    },
                  ]}
                >
                  {tab.label}
                </Text>
              </TouchableOpacity>
            ))}
          </View>

          {renderTabContent()}
        </ScrollView>
      ) : (
        <View style={[styles.content, styles.scrollContent]}>
          {renderConnectionCard()}

          <View
            style={[styles.tabBar, { backgroundColor: theme.colors.surface }]}
          >
            {tabs.map(tab => (
              <TouchableOpacity
                key={tab.id}
                style={[
                  styles.tab,
                  activeTab === tab.id && [
                    styles.activeTab,
                    { borderColor: theme.colors.primary },
                  ],
                ]}
                onPress={() => setActiveTab(tab.id)}
              >
                <Icon
                  name={tab.icon}
                  size={20}
                  color={
                    activeTab === tab.id
                      ? theme.colors.primary
                      : theme.colors.textSecondary
                  }
                />
                <Text
                  style={[
                    styles.tabLabel,
                    {
                      color:
                        activeTab === tab.id
                          ? theme.colors.primary
                          : theme.colors.textSecondary,
                    },
                  ]}
                >
                  {tab.label}
                </Text>
              </TouchableOpacity>
            ))}
          </View>

          {renderTabContent()}
        </View>
      )}
    </KeyboardAvoidingView>
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
    alignItems: 'flex-start',
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
  content: {
    flex: 1,
  },
  scrollContent: {
    padding: 16,
    paddingBottom: 40,
  },
  card: {
    borderRadius: 16,
    padding: 16,
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
  cardHeader: {
    marginBottom: 12,
  },
  cardTitle: {
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 12,
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  statusDot: {
    width: 12,
    height: 12,
    borderRadius: 6,
    marginRight: 8,
  },
  statusLabel: {
    fontSize: 16,
    fontWeight: '600',
  },
  userInfo: {
    paddingVertical: 12,
    borderTopWidth: 1,
    borderBottomWidth: 1,
    borderColor: 'rgba(0,0,0,0.1)',
    marginBottom: 12,
  },
  userLabel: {
    fontSize: 12,
    marginBottom: 4,
  },
  userName: {
    fontSize: 18,
    fontWeight: '600',
    marginBottom: 2,
  },
  userId: {
    fontSize: 12,
    fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
  },
  errorBox: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 12,
    borderRadius: 8,
    marginBottom: 12,
    gap: 8,
  },
  errorText: {
    flex: 1,
    fontSize: 14,
  },
  usernameSection: {
    marginBottom: 16,
    paddingTop: 12,
    borderTopWidth: 1,
    borderTopColor: 'rgba(0,0,0,0.1)',
  },
  usernameLabel: {
    fontSize: 14,
    fontWeight: '500',
    marginBottom: 8,
  },
  usernameInput: {
    borderRadius: 12,
    borderWidth: 1,
    padding: 14,
    fontSize: 16,
    fontWeight: '500',
  },
  usernameHint: {
    fontSize: 12,
    marginTop: 6,
    fontStyle: 'italic',
  },
  buttonRow: {
    flexDirection: 'row',
    gap: 12,
  },
  button: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 12,
    paddingHorizontal: 16,
    borderRadius: 12,
    gap: 8,
  },
  buttonSecondary: {
    backgroundColor: 'transparent',
    borderWidth: 1.5,
  },
  buttonText: {
    fontSize: 14,
    fontWeight: '600',
  },
  tabBar: {
    flexDirection: 'row',
    borderRadius: 16,
    padding: 4,
    marginBottom: 16,
  },
  tab: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 12,
    borderRadius: 12,
    gap: 6,
    borderWidth: 2,
    borderColor: 'transparent',
  },
  activeTab: {
    backgroundColor: 'rgba(0,0,0,0.05)',
  },
  tabLabel: {
    fontSize: 14,
    fontWeight: '500',
  },
  tabContent: {
    flex: 1,
  },
  centeredMessage: {
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 60,
  },
  centeredText: {
    marginTop: 16,
    fontSize: 16,
    textAlign: 'center',
    paddingHorizontal: 32,
  },
  recipientInput: {
    borderRadius: 12,
    padding: 12,
    marginBottom: 12,
  },
  inputLabel: {
    fontSize: 12,
    marginBottom: 6,
  },
  textInput: {
    borderRadius: 8,
    padding: 12,
    fontSize: 16,
  },
  messagesContainer: {
    minHeight: 200,
    maxHeight: 300,
    marginBottom: 12,
  },
  messagesList: {
    paddingVertical: 8,
  },
  emptyMessages: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 40,
  },
  emptyText: {
    fontSize: 16,
    marginTop: 12,
  },
  emptySubtext: {
    fontSize: 14,
    marginTop: 4,
  },
  messageBubble: {
    maxWidth: '80%',
    padding: 12,
    borderRadius: 16,
    marginVertical: 4,
  },
  myMessage: {
    alignSelf: 'flex-end',
    borderBottomRightRadius: 4,
  },
  theirMessage: {
    alignSelf: 'flex-start',
    borderBottomLeftRadius: 4,
  },
  messageSender: {
    fontSize: 12,
    marginBottom: 4,
  },
  messageText: {
    fontSize: 16,
    lineHeight: 20,
  },
  messageTime: {
    fontSize: 11,
    marginTop: 4,
    alignSelf: 'flex-end',
    opacity: 0.7,
  },
  inputContainer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    borderRadius: 16,
    padding: 8,
    gap: 8,
  },
  inputWrapper: {
    flex: 1,
    borderRadius: 12,
    paddingHorizontal: 12,
    paddingVertical: 8,
    maxHeight: 100,
  },
  messageInput: {
    fontSize: 16,
    maxHeight: 80,
  },
  sendButton: {
    width: 44,
    height: 44,
    borderRadius: 22,
    alignItems: 'center',
    justifyContent: 'center',
  },
  clearButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    padding: 12,
    borderRadius: 12,
    marginTop: 8,
    gap: 8,
  },
  clearButtonText: {
    fontSize: 14,
    fontWeight: '500',
  },
  presenceInputRow: {
    flexDirection: 'row',
    gap: 8,
  },
  presenceInput: {
    flex: 1,
  },
  checkButton: {
    width: 48,
    height: 48,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
  },
  userItem: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(0,0,0,0.05)',
  },
  userItemLeft: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
  },
  userStatusDot: {
    width: 10,
    height: 10,
    borderRadius: 5,
  },
  userItemName: {
    fontSize: 16,
    fontWeight: '500',
  },
  userItemId: {
    fontSize: 12,
    marginTop: 2,
  },
  userStatus: {
    fontSize: 12,
  },
  groupItem: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(0,0,0,0.05)',
    gap: 12,
  },
  groupItemInfo: {
    flex: 1,
  },
  groupItemName: {
    fontSize: 16,
    fontWeight: '500',
  },
  groupItemId: {
    fontSize: 12,
    marginTop: 2,
  },
  infoBox: {
    flexDirection: 'row',
    padding: 16,
    borderRadius: 12,
    gap: 12,
    alignItems: 'flex-start',
  },
  infoText: {
    flex: 1,
    fontSize: 14,
    lineHeight: 20,
  },
});

