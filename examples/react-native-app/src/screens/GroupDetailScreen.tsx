import React, { useState, useEffect, useCallback, useRef } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ScrollView,
  FlatList,
  TextInput,
  Alert,
  ActivityIndicator,
  PanResponder,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { useTheme } from '../hooks/useTheme';
import { useWebSocketRelay } from '../hooks/useWebSocketRelay';
import { useProtocol } from '../hooks/useProtocol';
import { Icon } from '../components/Icon';

type Tab = 'info' | 'members' | 'chat';

interface GroupDetailScreenProps {
  groupId: string;
  groupName: string;
  onBack: () => void;
}

interface GroupMember {
  username: string;
  role: 'admin' | 'member';
  joined_at: string;
}

interface GroupInfo {
  group_id: string;
  name: string;
  created_by: string;
  created_at: string;
  members: GroupMember[];
}

interface GroupMessage {
  id: string;
  sender: string;
  content: string;
  timestamp: Date;
  isFromMe: boolean;
  replyToMsg?: string;
}

export function GroupDetailScreen({ groupId, groupName, onBack }: GroupDetailScreenProps) {
  const { theme } = useTheme();
  const {
    send,
    authenticatedUser,
    status,
    groupDetails,
    getGroupInfo,
    groupMessages,
  } = useWebSocketRelay();
  const { protocol, isInitialized } = useProtocol();
  const [activeTab, setActiveTab] = useState<Tab>('info');
  const [loading, setLoading] = useState(true);
  const [messageInput, setMessageInput] = useState('');
  const [usernameInput, setUsernameInput] = useState('');
  const [addingMember, setAddingMember] = useState(false);
  const [replyingTo, setReplyingTo] = useState<GroupMessage | null>(null);
  const flatListRef = useRef<FlatList>(null);
  const hasRequestedInfo = useRef(false);

  // Get group info from context
  const contextGroupDetails = groupDetails.get(groupId);

  // Get messages from context - all messages now use server-provided IDs
  const messages: GroupMessage[] = React.useMemo(() => {
    const contextMessages = groupMessages.get(groupId) || [];
    // Convert context messages to local format
    // All messages now have server-provided IDs from GroupMessageSent/GroupMessageReceived
    return contextMessages
      .map(m => ({
        id: m.id, // Server-provided message_id
        sender: m.sender,
        content: m.content,
        timestamp: m.timestamp,
        isFromMe: m.isFromMe,
        replyToMsg: m.replyToMsg,
      }))
      .sort((a, b) => a.timestamp.getTime() - b.timestamp.getTime());
  }, [groupMessages, groupId]);

  // Convert context GroupDetails to local GroupInfo format for rendering
  const groupInfo: GroupInfo | null = React.useMemo(() => {
    if (!contextGroupDetails) return null;
    return {
      group_id: contextGroupDetails.groupId,
      name: contextGroupDetails.name,
      created_by: contextGroupDetails.createdBy,
      created_at: contextGroupDetails.createdAt.toISOString(),
      members: contextGroupDetails.members.map(m => ({
        username: m.userId,
        role: m.role,
        joined_at: m.joinedAt.toISOString(),
      })),
    };
  }, [contextGroupDetails]);

  const loadGroupInfo = useCallback(() => {
    if (status !== 'authenticated') {
      console.warn('[GroupDetailScreen] Not authenticated, status:', status);
      setLoading(false);
      return;
    }

    console.log('[GroupDetailScreen] Requesting group info for:', groupId);
    setLoading(true);
    const sent = getGroupInfo(groupId);
    if (!sent) {
      console.error('[GroupDetailScreen] Failed to send group info request');
    }
  }, [groupId, status, getGroupInfo]);

  const handleAddMember = useCallback(async () => {
    const username = usernameInput.trim();
    if (!username) {
      Alert.alert('Error', 'Please enter a username');
      return;
    }

    if (!isInitialized || !protocol) {
      Alert.alert(
        'Protocol Not Ready',
        'The protocol is not initialized yet. Please wait a moment and try again.',
      );
      return;
    }

    if (status !== 'authenticated') {
      Alert.alert(
        'WebSocket Not Connected',
        `WebSocket status: ${status}. Please wait for the connection to be established.`,
      );
      return;
    }

    setAddingMember(true);
    try {
      const addMemberJson = await protocol.groupAddMember(groupId, username);
      const addMemberPayload = JSON.parse(addMemberJson);
      const sent = send(addMemberPayload);
      if (!sent) {
        throw new Error('Failed to send add member request');
      }
      setUsernameInput('');
      // Reload group info after adding member
      setTimeout(() => getGroupInfo(groupId), 500);
    } catch (error: any) {
      console.error('Failed to add member:', error);
      Alert.alert('Error', error.message || 'Failed to add member');
    } finally {
      setAddingMember(false);
    }
  }, [
    groupId,
    usernameInput,
    protocol,
    send,
    status,
    isInitialized,
    getGroupInfo,
  ]);

  const handleSendMessage = useCallback(async () => {
    if (!messageInput.trim()) return;
    if (!isInitialized || !protocol) {
      Alert.alert(
        'Protocol Not Ready',
        'The protocol is not initialized yet. Please wait a moment and try again.',
      );
      return;
    }
    if (status !== 'authenticated' && status !== 'connected') {
      Alert.alert(
        'WebSocket Not Connected',
        `WebSocket status: ${status}. Please wait for the connection to be established.`,
      );
      return;
    }

    const content = messageInput.trim();
    const replyToMsgId = replyingTo?.id;
    setMessageInput(''); // Clear input immediately for better UX
    setReplyingTo(null); // Clear reply state

    try {
      // Use the updated groupSendMessage with reply_to_msg parameter
      const sendMessageJson = await protocol.groupSendMessage(
        groupId,
        content,
        replyToMsgId || undefined,
      );
      console.warn('[GroupDetailScreen] sendMessageJson:', sendMessageJson);
      const sendMessagePayload = JSON.parse(sendMessageJson);
      const sent = send(sendMessagePayload);
      if (!sent) {
        throw new Error('Failed to send group message');
      }

      // Don't add message here - wait for GroupMessageSent response which provides
      // the actual server message_id. The message will be added to groupMessages
      // in WebSocketRelayProvider when GroupMessageSent is received with the server's message_id.
    } catch (error: any) {
      console.error('Failed to send message:', error);
      Alert.alert('Error', error.message || 'Failed to send message');
      setMessageInput(content); // Restore input on error
      if (replyToMsgId) {
        setReplyingTo(replyingTo); // Restore reply state on error
      }
    }
  }, [
    groupId,
    messageInput,
    protocol,
    send,
    status,
    isInitialized,
    replyingTo,
  ]);

  // Load group info once on mount
  useEffect(() => {
    if (!hasRequestedInfo.current && status === 'authenticated') {
      hasRequestedInfo.current = true;
      loadGroupInfo();
    }
  }, [status, loadGroupInfo]);

  // Update loading state when group info is received
  useEffect(() => {
    if (groupInfo) {
      setLoading(false);
    }
  }, [groupInfo]);

  // Listen for group messages via WebSocket
  // Note: GroupMessageReceived events will need to be handled by a callback
  // For now, messages sent by this user are added immediately
  // Received messages would need to be added via a callback from useWebSocketRelay

  // Auto-scroll chat to bottom
  useEffect(() => {
    if (activeTab === 'chat' && messages.length > 0) {
      setTimeout(() => {
        flatListRef.current?.scrollToEnd({ animated: true });
      }, 100);
    }
  }, [messages.length, activeTab]);

  const renderInfoTab = () => {
    if (loading) {
      return (
        <View style={styles.centerContainer}>
          <ActivityIndicator size="large" color={theme.colors.primary} />
        </View>
      );
    }

    if (!groupInfo) {
      return (
        <View style={styles.centerContainer}>
          <Text style={[styles.errorText, { color: theme.colors.error }]}>
            Failed to load group info
          </Text>
        </View>
      );
    }

    return (
      <ScrollView
        style={styles.tabContent}
        contentContainerStyle={styles.tabContentContainer}
      >
        <View
          style={[styles.infoCard, { backgroundColor: theme.colors.surface }]}
        >
          <Text
            style={[styles.infoLabel, { color: theme.colors.textSecondary }]}
          >
            Group Name
          </Text>
          <Text style={[styles.infoValue, { color: theme.colors.text }]}>
            {groupInfo.name}
          </Text>
        </View>

        <View
          style={[styles.infoCard, { backgroundColor: theme.colors.surface }]}
        >
          <Text
            style={[styles.infoLabel, { color: theme.colors.textSecondary }]}
          >
            Created By
          </Text>
          <Text style={[styles.infoValue, { color: theme.colors.text }]}>
            {groupInfo.created_by}
          </Text>
        </View>

        <View
          style={[styles.infoCard, { backgroundColor: theme.colors.surface }]}
        >
          <Text
            style={[styles.infoLabel, { color: theme.colors.textSecondary }]}
          >
            Created At
          </Text>
          <Text style={[styles.infoValue, { color: theme.colors.text }]}>
            {new Date(groupInfo.created_at).toLocaleString()}
          </Text>
        </View>

        <View
          style={[styles.infoCard, { backgroundColor: theme.colors.surface }]}
        >
          <Text
            style={[styles.infoLabel, { color: theme.colors.textSecondary }]}
          >
            Members
          </Text>
          <Text style={[styles.infoValue, { color: theme.colors.text }]}>
            {groupInfo.members.length}
          </Text>
        </View>
      </ScrollView>
    );
  };

  const renderMembersTab = () => {
    if (loading) {
      return (
        <View style={styles.centerContainer}>
          <ActivityIndicator size="large" color={theme.colors.primary} />
        </View>
      );
    }

    if (!groupInfo) {
      return (
        <View style={styles.centerContainer}>
          <Text style={[styles.errorText, { color: theme.colors.error }]}>
            Failed to load group info
          </Text>
        </View>
      );
    }

    const isAdmin = groupInfo.members.some(
      m => m.username === authenticatedUser?.username && m.role === 'admin',
    );

    return (
      <ScrollView
        style={styles.tabContent}
        contentContainerStyle={styles.tabContentContainer}
      >
        {isAdmin && (
          <View
            style={[
              styles.addMemberCard,
              { backgroundColor: theme.colors.surface },
            ]}
          >
            <Text style={[styles.sectionTitle, { color: theme.colors.text }]}>
              Add Member
            </Text>
            <View style={styles.addMemberInputContainer}>
              <TextInput
                style={[
                  styles.addMemberInput,
                  {
                    backgroundColor: theme.colors.background,
                    color: theme.colors.text,
                    borderColor: theme.colors.border,
                  },
                ]}
                placeholder="Enter username"
                placeholderTextColor={theme.colors.textSecondary}
                value={usernameInput}
                onChangeText={setUsernameInput}
                onSubmitEditing={handleAddMember}
                editable={!addingMember}
              />
              <TouchableOpacity
                style={[
                  styles.addMemberButton,
                  {
                    backgroundColor: addingMember
                      ? theme.colors.textSecondary
                      : theme.colors.primary,
                  },
                ]}
                onPress={handleAddMember}
                disabled={addingMember}
              >
                {addingMember ? (
                  <ActivityIndicator
                    size="small"
                    color={theme.colors.textInverse}
                  />
                ) : (
                  <Icon name="add" size={20} color={theme.colors.textInverse} />
                )}
              </TouchableOpacity>
            </View>
          </View>
        )}

        <View style={styles.membersList}>
          {groupInfo.members.map((member, index) => {
            const displayName = member.username || 'Unknown';
            return (
              <View
                key={index}
                style={[
                  styles.memberCard,
                  { backgroundColor: theme.colors.surface },
                ]}
              >
                <View
                  style={[
                    styles.memberAvatar,
                    { backgroundColor: theme.colors.primary + '20' },
                  ]}
                >
                  <Text
                    style={[
                      styles.memberAvatarText,
                      { color: theme.colors.primary },
                    ]}
                  >
                    {displayName.charAt(0).toUpperCase()}
                  </Text>
                </View>
                <View style={styles.memberInfo}>
                  <Text
                    style={[styles.memberName, { color: theme.colors.text }]}
                  >
                    {displayName}
                  </Text>
                  <Text
                    style={[
                      styles.memberRole,
                      { color: theme.colors.textSecondary },
                    ]}
                  >
                    {member.role || 'member'}
                  </Text>
                </View>
                {member.role === 'admin' && (
                  <View
                    style={[
                      styles.adminBadge,
                      { backgroundColor: theme.colors.primary },
                    ]}
                  >
                    <Text
                      style={[
                        styles.adminBadgeText,
                        { color: theme.colors.textInverse },
                      ]}
                    >
                      Admin
                    </Text>
                  </View>
                )}
              </View>
            );
          })}
        </View>
      </ScrollView>
    );
  };

  // Helper function to find replied-to message
  const findRepliedMessage = useCallback(
    (replyToMsgId: string | undefined): GroupMessage | null => {
      if (!replyToMsgId) return null;
      return messages.find(m => m.id === replyToMsgId) || null;
    },
    [messages],
  );

  // Create pan responder for swipe gestures
  const createPanResponder = useCallback((item: GroupMessage) => {
    return PanResponder.create({
      onStartShouldSetPanResponder: () => true,
      onMoveShouldSetPanResponder: (_, gestureState) => {
        // Only respond to horizontal swipes
        return (
          Math.abs(gestureState.dx) > Math.abs(gestureState.dy) &&
          Math.abs(gestureState.dx) > 10
        );
      },
      onPanResponderGrant: () => {
        // Could add visual feedback here
      },
      onPanResponderMove: () => {
        // Visual feedback could be added here
      },
      onPanResponderRelease: (_evt, gestureState) => {
        const swipeThreshold = 50;
        const dx = gestureState.dx;

        // Swipe right on sender's message OR swipe left on own message
        if (
          (!item.isFromMe && dx > swipeThreshold) ||
          (item.isFromMe && dx < -swipeThreshold)
        ) {
          setReplyingTo(item);
          // Scroll to input
          setTimeout(() => {
            flatListRef.current?.scrollToEnd({ animated: true });
          }, 100);
        }
      },
    });
  }, []);

  const renderChatTab = () => {
    return (
      <View style={styles.chatContainer}>
        <FlatList
          ref={flatListRef}
          data={messages}
          keyExtractor={item => item.id}
          renderItem={({ item }) => {
            const panResponder = createPanResponder(item);
            const repliedMessage = findRepliedMessage(item.replyToMsg);

            return (
              <View
                {...panResponder.panHandlers}
                style={[
                  styles.messageBubble,
                  item.isFromMe
                    ? [
                        styles.messageBubbleMe,
                        { backgroundColor: theme.colors.primary },
                      ]
                    : [
                        styles.messageBubbleOther,
                        { backgroundColor: theme.colors.surface },
                      ],
                ]}
              >
                {repliedMessage && (
                  <View
                    style={[
                      styles.replyIndicator,
                      {
                        borderLeftColor: item.isFromMe
                          ? theme.colors.textInverse + '80'
                          : theme.colors.primary,
                        backgroundColor: item.isFromMe
                          ? theme.colors.textInverse + '20'
                          : theme.colors.background,
                      },
                    ]}
                  >
                    <Text
                      style={[
                        styles.replySender,
                        {
                          color: item.isFromMe
                            ? theme.colors.textInverse + 'CC'
                            : theme.colors.textSecondary,
                        },
                      ]}
                      numberOfLines={1}
                    >
                      {repliedMessage.isFromMe ? 'You' : repliedMessage.sender}
                    </Text>
                    <Text
                      style={[
                        styles.replyContent,
                        {
                          color: item.isFromMe
                            ? theme.colors.textInverse + 'AA'
                            : theme.colors.text,
                        },
                      ]}
                      numberOfLines={1}
                    >
                      {repliedMessage.content}
                    </Text>
                  </View>
                )}
                {!item.isFromMe && (
                  <Text
                    style={[
                      styles.messageSender,
                      { color: theme.colors.textSecondary },
                    ]}
                  >
                    {item.sender}
                  </Text>
                )}
                <Text
                  style={[
                    styles.messageText,
                    {
                      color: item.isFromMe
                        ? theme.colors.textInverse
                        : theme.colors.text,
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
                        ? theme.colors.textInverse + 'CC'
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
          }}
          contentContainerStyle={styles.chatContent}
          ListEmptyComponent={
            <View style={styles.emptyChat}>
              <Text
                style={[
                  styles.emptyChatText,
                  { color: theme.colors.textSecondary },
                ]}
              >
                No messages yet. Start the conversation!
              </Text>
            </View>
          }
        />

        {replyingTo && (
          <View
            style={[
              styles.replyBar,
              {
                backgroundColor: theme.colors.surface,
                borderTopColor: theme.colors.border,
              },
            ]}
          >
            <View style={styles.replyBarContent}>
              <View
                style={[
                  styles.replyBarIndicator,
                  { backgroundColor: theme.colors.primary },
                ]}
              />
              <View style={styles.replyBarText}>
                <Text
                  style={[
                    styles.replyBarLabel,
                    { color: theme.colors.textSecondary },
                  ]}
                >
                  Replying to{' '}
                  {replyingTo.isFromMe ? 'yourself' : replyingTo.sender}
                </Text>
                <Text
                  style={[styles.replyBarPreview, { color: theme.colors.text }]}
                  numberOfLines={1}
                >
                  {replyingTo.content}
                </Text>
              </View>
              <TouchableOpacity
                onPress={() => setReplyingTo(null)}
                style={styles.replyBarClose}
              >
                <Icon
                  name="close"
                  size={20}
                  color={theme.colors.textSecondary}
                />
              </TouchableOpacity>
            </View>
          </View>
        )}

        <View
          style={[
            styles.chatInputContainer,
            { backgroundColor: theme.colors.surface },
          ]}
        >
          <TextInput
            style={[
              styles.chatInput,
              {
                backgroundColor: theme.colors.background,
                color: theme.colors.text,
                borderColor: theme.colors.border,
              },
            ]}
            placeholder={
              replyingTo
                ? `Reply to ${
                    replyingTo.isFromMe ? 'yourself' : replyingTo.sender
                  }...`
                : 'Type a message...'
            }
            placeholderTextColor={theme.colors.textSecondary}
            value={messageInput}
            onChangeText={setMessageInput}
            multiline
          />
          <TouchableOpacity
            style={[
              styles.sendButton,
              {
                backgroundColor: messageInput.trim()
                  ? theme.colors.primary
                  : theme.colors.textSecondary,
              },
            ]}
            onPress={handleSendMessage}
            disabled={!messageInput.trim()}
          >
            <Icon
              name="send"
              size={20}
              color={
                messageInput.trim()
                  ? theme.colors.textInverse
                  : theme.colors.textSecondary
              }
            />
          </TouchableOpacity>
        </View>
      </View>
    );
  };

  return (
    <SafeAreaView
      style={[styles.container, { backgroundColor: theme.colors.background }]}
    >
      <View style={[styles.header, { backgroundColor: theme.colors.surface }]}>
        <TouchableOpacity onPress={onBack} style={styles.backButton}>
          <Icon name="arrow-back" size={24} color={theme.colors.primary} />
        </TouchableOpacity>
        <Text
          style={[styles.headerTitle, { color: theme.colors.text }]}
          numberOfLines={1}
        >
          {groupName}
        </Text>
        <View style={styles.backButton} />
      </View>

      <View style={[styles.tabBar, { backgroundColor: theme.colors.surface }]}>
        {(['info', 'members', 'chat'] as Tab[]).map(tab => (
          <TouchableOpacity
            key={tab}
            style={[
              styles.tab,
              activeTab === tab && [
                styles.tabActive,
                { borderBottomColor: theme.colors.primary },
              ],
            ]}
            onPress={() => setActiveTab(tab)}
          >
            <Text
              style={[
                styles.tabText,
                {
                  color:
                    activeTab === tab
                      ? theme.colors.primary
                      : theme.colors.textSecondary,
                  fontWeight: activeTab === tab ? '600' : '400',
                },
              ]}
            >
              {tab.charAt(0).toUpperCase() + tab.slice(1)}
            </Text>
          </TouchableOpacity>
        ))}
      </View>

      <View style={styles.content}>
        {activeTab === 'info' && renderInfoTab()}
        {activeTab === 'members' && renderMembersTab()}
        {activeTab === 'chat' && renderChatTab()}
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(0,0,0,0.1)',
  },
  backButton: {
    width: 40,
    height: 40,
    alignItems: 'center',
    justifyContent: 'center',
  },
  headerTitle: {
    flex: 1,
    fontSize: 18,
    fontWeight: '600',
    textAlign: 'center',
    marginHorizontal: 16,
  },
  tabBar: {
    flexDirection: 'row',
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(0,0,0,0.1)',
  },
  tab: {
    flex: 1,
    paddingVertical: 12,
    alignItems: 'center',
    borderBottomWidth: 2,
    borderBottomColor: 'transparent',
  },
  tabActive: {
    borderBottomWidth: 2,
  },
  tabText: {
    fontSize: 14,
  },
  content: {
    flex: 1,
  },
  tabContent: {
    flex: 1,
  },
  tabContentContainer: {
    padding: 16,
  },
  centerContainer: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  errorText: {
    fontSize: 16,
  },
  infoCard: {
    padding: 16,
    borderRadius: 12,
    marginBottom: 12,
  },
  infoLabel: {
    fontSize: 12,
    marginBottom: 4,
  },
  infoValue: {
    fontSize: 16,
    fontWeight: '600',
  },
  sectionTitle: {
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 12,
  },
  addMemberCard: {
    padding: 16,
    borderRadius: 12,
    marginBottom: 16,
  },
  addMemberInputContainer: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  addMemberInput: {
    flex: 1,
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 14,
    marginRight: 8,
  },
  addMemberButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  membersList: {
    gap: 12,
  },
  memberCard: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 16,
    borderRadius: 12,
  },
  memberAvatar: {
    width: 48,
    height: 48,
    borderRadius: 24,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 12,
  },
  memberAvatarText: {
    fontSize: 20,
    fontWeight: '600',
  },
  memberInfo: {
    flex: 1,
  },
  memberName: {
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 4,
  },
  memberRole: {
    fontSize: 12,
    textTransform: 'capitalize',
  },
  adminBadge: {
    paddingHorizontal: 8,
    paddingVertical: 4,
    borderRadius: 12,
  },
  adminBadgeText: {
    fontSize: 10,
    fontWeight: '600',
  },
  chatContainer: {
    flex: 1,
  },
  chatContent: {
    padding: 16,
  },
  emptyChat: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 64,
  },
  emptyChatText: {
    fontSize: 14,
  },
  messageBubble: {
    maxWidth: '80%',
    padding: 12,
    borderRadius: 16,
    marginBottom: 8,
  },
  messageBubbleMe: {
    alignSelf: 'flex-end',
  },
  messageBubbleOther: {
    alignSelf: 'flex-start',
  },
  messageSender: {
    fontSize: 12,
    marginBottom: 4,
  },
  messageText: {
    fontSize: 16,
    marginBottom: 4,
  },
  messageTime: {
    fontSize: 10,
    alignSelf: 'flex-end',
  },
  chatInputContainer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    padding: 12,
    borderTopWidth: 1,
    borderTopColor: 'rgba(0,0,0,0.1)',
  },
  chatInput: {
    flex: 1,
    borderWidth: 1,
    borderRadius: 20,
    paddingHorizontal: 16,
    paddingVertical: 10,
    fontSize: 16,
    maxHeight: 100,
    marginRight: 8,
  },
  sendButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  replyIndicator: {
    padding: 8,
    marginBottom: 8,
    borderRadius: 8,
    borderLeftWidth: 3,
  },
  replySender: {
    fontSize: 12,
    fontWeight: '600',
    marginBottom: 2,
  },
  replyContent: {
    fontSize: 13,
  },
  replyBar: {
    borderTopWidth: 1,
    paddingVertical: 8,
    paddingHorizontal: 12,
  },
  replyBarContent: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  replyBarIndicator: {
    width: 3,
    height: 40,
    borderRadius: 2,
    marginRight: 12,
  },
  replyBarText: {
    flex: 1,
  },
  replyBarLabel: {
    fontSize: 12,
    marginBottom: 2,
  },
  replyBarPreview: {
    fontSize: 14,
  },
  replyBarClose: {
    padding: 8,
  },
});
