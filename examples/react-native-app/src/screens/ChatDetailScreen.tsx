import React, { useState, useEffect, useRef, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  FlatList,
  TextInput,
  TouchableOpacity,
  KeyboardAvoidingView,
  Platform,
  Alert,
  Keyboard,
  GestureResponderEvent,
  PanResponder,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Icon } from '../components/Icon';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { Message } from '../providers/ProtocolProvider';
import { MessagePriority } from '@offline-protocol/mesh-sdk';
import { getUserInitials, generateAvatarColor } from '../utils/user';

interface MessageBubbleProps {
  message: Message;
  isLastInGroup: boolean;
  isFirstInGroup: boolean;
  onSwipeRight?: (message: Message) => void;
  allMessages?: Message[]; // For finding replied-to message
}

function MessageBubble({ message, isLastInGroup, isFirstInGroup, onSwipeRight, allMessages }: MessageBubbleProps) {
  const { theme } = useTheme();
  const isFromMe = message.isFromMe;
  const isEncrypted = message.isEncrypted ?? false;
  
  // Find the message this is replying to
  const repliedToMessage = message.replyToMsg && allMessages
    ? allMessages.find(m => m.id === message.replyToMsg)
    : undefined;
  
  // Pan responder for swipe gesture
  const panResponder = useRef(
    PanResponder.create({
      onStartShouldSetPanResponder: () => false,
      onMoveShouldSetPanResponder: (_, gestureState) => {
        // Only respond to horizontal swipes (right swipe)
        return Math.abs(gestureState.dx) > 10 && Math.abs(gestureState.dx) > Math.abs(gestureState.dy);
      },
      onPanResponderRelease: (_, gestureState) => {
        // Right swipe (positive dx) to select for reply
        if (gestureState.dx > 50 && onSwipeRight) {
          onSwipeRight(message);
        }
      },
    })
  ).current;
  
  const formatTime = (timestamp: number) => {
    return new Date(timestamp).toLocaleTimeString([], { 
      hour: '2-digit', 
      minute: '2-digit' 
    });
  };

  const getPriorityColor = (priority: MessagePriority) => {
    switch (priority) {
      case MessagePriority.High:
        return theme.colors.error;
      case MessagePriority.Medium:
        return theme.colors.primary;
      case MessagePriority.Low:
        return theme.colors.textSecondary;
      default:
        return theme.colors.primary;
    }
  };

  const getStatusIcon = (status: Message['status']) => {
    switch (status) {
      case 'sending':
        return 'time-outline';
      case 'sent':
        return 'checkmark';
      case 'delivered':
        return 'checkmark-done';
      case 'failed':
        return 'alert-circle-outline';
      default:
        return 'checkmark';
    }
  };

  return (
    <View 
      style={[
        styles.messageContainer,
        isFromMe ? styles.myMessageContainer : styles.theirMessageContainer,
      ]}
      {...panResponder.panHandlers}
    >
      <View
        style={[
          styles.messageBubble,
          isFromMe 
            ? [styles.myMessageBubble, { backgroundColor: theme.colors.primary }]
            : [styles.theirMessageBubble, { backgroundColor: theme.colors.surface }],
          isFirstInGroup && styles.firstInGroup,
          isLastInGroup && styles.lastInGroup,
        ]}
      >
        {/* Reply preview */}
        {repliedToMessage && (
          <View style={[
            styles.replyPreview,
            {
              backgroundColor: isFromMe 
                ? 'rgba(255,255,255,0.2)' 
                : theme.colors.background,
              borderLeftColor: theme.colors.primary,
            }
          ]}>
            <Text
              style={[
                styles.replyPreviewSender,
                { color: isFromMe ? theme.colors.textInverse : theme.colors.primary }
              ]}
              numberOfLines={1}
            >
              {repliedToMessage.isFromMe ? 'You' : 'They'}
            </Text>
            <Text
              style={[
                styles.replyPreviewText,
                { color: isFromMe ? theme.colors.textInverse : theme.colors.textSecondary }
              ]}
              numberOfLines={1}
            >
              {repliedToMessage.content}
            </Text>
          </View>
        )}
        
        <Text
          style={[
            styles.messageText,
            { 
              color: isFromMe ? theme.colors.textInverse : theme.colors.text 
            },
          ]}
        >
          {message.content}
        </Text>
        
        <View style={styles.messageFooter}>
          {isEncrypted && (
            <Icon
              name="lock-closed"
              size={10}
              color={isFromMe ? theme.colors.textInverse : theme.colors.textSecondary}
              style={{ marginRight: 4, opacity: 0.7 }}
            />
          )}
          <Text
            style={[
              styles.messageTime,
              { 
                color: isFromMe 
                  ? theme.colors.textInverse 
                  : theme.colors.textSecondary,
                opacity: 0.7,
              },
            ]}
          >
            {formatTime(message.timestamp)}
          </Text>
          
          {isFromMe && (
            <Icon
              name={getStatusIcon(message.status)}
              size={12}
              color={theme.colors.textInverse}
              style={{ marginLeft: 4, opacity: 0.7 }}
            />
          )}
        </View>
      </View>
      
      {message.priority === MessagePriority.High && (
        <View style={[styles.priorityIndicator, { backgroundColor: getPriorityColor(message.priority) }]} />
      )}
    </View>
  );
}

interface ChatDetailScreenProps {
  peerId: string;
  peerName: string;
  onBack: () => void;
  onNavigateToProfile: (userId: string) => void;
}

export function ChatDetailScreen({ peerId, peerName, onBack, onNavigateToProfile }: ChatDetailScreenProps) {
  const { theme } = useTheme();
  
  const { chats, contacts, sendMessage, currentUserId, isOnline, connectedPeersCount, isMlsInitialized, encryptedPeers } = useProtocol();
  const insets = useSafeAreaInsets();
  const [inputText, setInputText] = useState('');
  const [priority, setPriority] = useState<MessagePriority>(MessagePriority.Medium);
  const [showPriorityPicker, setShowPriorityPicker] = useState(false);
  const [isKeyboardVisible, setIsKeyboardVisible] = useState(false);
  const [inputHeight, setInputHeight] = useState(44);
  const [replyingToMessage, setReplyingToMessage] = useState<Message | null>(null);
  
  const flatListRef = useRef<FlatList>(null);
  const inputRef = useRef<TextInput>(null);
  const scrollTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const chat = chats.find(c => c.peerId === peerId);
  const contact = contacts.find(c => c.id === peerId);
  const messages = chat?.messages || [];
  const isPeerOnline = contact?.isOnline ?? false;
  const isEncryptedChat = chat?.isEncrypted || encryptedPeers.has(peerId);
  
  const avatarColor = generateAvatarColor(peerId);
  const initials = getUserInitials(peerName);

  // Header component for chat detail
  const renderHeader = () => (
    <View style={[styles.header, { backgroundColor: theme.colors.surface }]}>
      <TouchableOpacity
        style={styles.backButton}
        onPress={onBack}
        activeOpacity={0.7}
      >
        <Icon name="arrow-back" size={24} color={theme.colors.primary} />
      </TouchableOpacity>
      
      <TouchableOpacity
        style={styles.headerTitle}
        onPress={() => onNavigateToProfile(peerId)}
        activeOpacity={0.7}
      >
        <View style={[styles.headerAvatar, { backgroundColor: avatarColor }]}>
          <Text style={[styles.headerAvatarText, { color: theme.colors.textInverse }]}>
            {initials}
          </Text>
          {contact?.isOnline && (
            <View style={[styles.headerOnlineIndicator, { backgroundColor: theme.colors.online }]} />
          )}
        </View>
        <View style={styles.headerInfo}>
          <View style={styles.headerNameRow}>
            <Text style={[styles.headerName, { color: theme.colors.text }]}>
              {peerName}
            </Text>
            {isEncryptedChat && (
              <View style={[styles.encryptedBadge, { backgroundColor: theme.colors.success + '20' }]}>
                <Icon name="lock-closed" size={10} color={theme.colors.success} />
              </View>
            )}
          </View>
          <Text style={[styles.headerStatus, { color: theme.colors.textSecondary }]}>
            {contact?.isOnline ? 'Online' : 'Offline'}
            {isEncryptedChat ? ' • Encrypted' : ''}
          </Text>
        </View>
      </TouchableOpacity>
      
      <View style={{ width: 40 }} />
    </View>
  );

  useEffect(() => {
    const keyboardWillShow = Keyboard.addListener(
      Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow',
      () => {
        setIsKeyboardVisible(true);
        // Scroll to end with a slight delay to ensure layout is updated
        if (scrollTimeoutRef.current) {
          clearTimeout(scrollTimeoutRef.current);
        }
        scrollTimeoutRef.current = setTimeout(() => {
          flatListRef.current?.scrollToEnd({ animated: true });
        }, 100);
      }
    );
    
    const keyboardWillHide = Keyboard.addListener(
      Platform.OS === 'ios' ? 'keyboardWillHide' : 'keyboardDidHide',
      () => {
        setIsKeyboardVisible(false);
        setShowPriorityPicker(false);
      }
    );

    return () => {
      keyboardWillShow.remove();
      keyboardWillHide.remove();
      if (scrollTimeoutRef.current) {
        clearTimeout(scrollTimeoutRef.current);
      }
    };
  }, []);

  useEffect(() => {
    // Scroll to bottom when new messages arrive
    if (messages.length > 0) {
      setTimeout(() => {
        flatListRef.current?.scrollToEnd({ animated: true });
      }, 100);
    }
  }, [messages.length]);

  const handleSend = useCallback(async () => {
    const text = inputText.trim();
    if (!text) return;

    try {
      await sendMessage(peerId, text, priority, replyingToMessage?.id);
      setInputText('');
      setInputHeight(44);
      setReplyingToMessage(null); // Clear reply selection after sending
      // Keep keyboard open for quick follow-up messages
      setTimeout(() => {
        flatListRef.current?.scrollToEnd({ animated: true });
      }, 50);
    } catch (error) {
      Alert.alert('Send Failed', 'Failed to send message. Please try again.');
    }
  }, [inputText, peerId, priority, sendMessage, replyingToMessage]);
  
  const handleSwipeRight = useCallback((message: Message) => {
    setReplyingToMessage(message);
    // Focus input and scroll to bottom
    inputRef.current?.focus();
    setTimeout(() => {
      flatListRef.current?.scrollToEnd({ animated: true });
    }, 100);
  }, []);

  const handleContentSizeChange = useCallback((event: any) => {
    const newHeight = Math.min(Math.max(44, event.nativeEvent.contentSize.height + 20), 120);
    setInputHeight(newHeight);
  }, []);

  const togglePriorityPicker = useCallback(() => {
    setShowPriorityPicker(prev => !prev);
  }, []);

  const selectPriority = useCallback((p: MessagePriority) => {
    setPriority(p);
    setShowPriorityPicker(false);
  }, []);

  const groupMessages = (messages: Message[]) => {
    const grouped: (Message & { isFirstInGroup: boolean; isLastInGroup: boolean })[] = [];
    
    messages.forEach((message, index) => {
      const prevMessage = messages[index - 1];
      const nextMessage = messages[index + 1];
      
      const isFirstInGroup = !prevMessage || 
        prevMessage.isFromMe !== message.isFromMe ||
        message.timestamp - prevMessage.timestamp > 300000; // 5 minutes
      
      const isLastInGroup = !nextMessage || 
        nextMessage.isFromMe !== message.isFromMe ||
        nextMessage.timestamp - message.timestamp > 300000; // 5 minutes
      
      grouped.push({
        ...message,
        isFirstInGroup,
        isLastInGroup,
      });
    });
    
    return grouped;
  };

  const groupedMessages = groupMessages(messages);

  const renderMessage = ({ item }: { item: Message & { isFirstInGroup: boolean; isLastInGroup: boolean } }) => (
    <MessageBubble
      message={item}
      isFirstInGroup={item.isFirstInGroup}
      isLastInGroup={item.isLastInGroup}
      onSwipeRight={handleSwipeRight}
      allMessages={messages}
    />
  );

  const renderEmptyState = () => (
    <View  style={styles.emptyState}>
      <View style={[styles.emptyAvatar, { backgroundColor: avatarColor }]}>
        <Text style={[styles.emptyAvatarText, { color: theme.colors.textInverse }]}>
          {initials}
        </Text>
      </View>
      <Text style={[styles.emptyTitle, { color: theme.colors.text }]}>
        Start a conversation with {peerName}
      </Text>
      <Text style={[styles.emptySubtitle, { color: theme.colors.textSecondary }]}>
        {contact?.isOnline 
          ? 'They\'re online and ready to chat!'
          : 'Your message will be delivered when they come online.'
        }
      </Text>
    </View>
  );

  const getPriorityIcon = (priority: MessagePriority) => {
    switch (priority) {
      case MessagePriority.High:
        return 'flash';
      case MessagePriority.Medium:
        return 'remove';
      case MessagePriority.Low:
        return 'ellipsis-horizontal';
      default:
        return 'remove';
    }
  };

  const getPriorityColor = (priority: MessagePriority) => {
    switch (priority) {
      case MessagePriority.High:
        return theme.colors.error;
      case MessagePriority.Medium:
        return theme.colors.primary;
      case MessagePriority.Low:
        return theme.colors.textSecondary;
      default:
        return theme.colors.primary;
    }
  };

  const renderStatusBanner = () => {
    if (!isOnline) {
      return (
        <View style={[
          styles.statusBanner,
          { backgroundColor: `${theme.colors.error}22`, borderColor: theme.colors.error }
        ]}>
          <Icon name="alert-circle" size={16} color={theme.colors.error} style={{ marginRight: 8 }} />
          <View style={{ flex: 1 }}>
            <Text style={[styles.statusBannerTitle, { color: theme.colors.error }]}>
              Messenger offline
            </Text>
            <Text style={[styles.statusBannerSubtitle, { color: theme.colors.textSecondary }]}>
              Messages will send automatically when service restarts.
            </Text>
          </View>
        </View>
      );
    }

    if (!isPeerOnline) {
      return (
        <View style={[
          styles.statusBanner,
          { backgroundColor: `${theme.colors.warning}22`, borderColor: theme.colors.warning }
        ]}>
          <Icon name="time" size={16} color={theme.colors.warning} style={{ marginRight: 8 }} />
          <View style={{ flex: 1 }}>
            <Text style={[styles.statusBannerTitle, { color: theme.colors.warning }]}>
              Waiting for {peerName}
            </Text>
            <Text style={[styles.statusBannerSubtitle, { color: theme.colors.textSecondary }]}>
              {connectedPeersCount > 0
                ? 'We will deliver this message once they come back online.'
                : 'Keep the app open so nearby peers can relay your message.'}
            </Text>
          </View>
        </View>
      );
    }

    return null;
  };

  const renderPriorityPicker = () => {
    if (!showPriorityPicker) return null;
    
    return (
      <View style={[styles.priorityPickerContainer, { backgroundColor: theme.colors.surface }]}>
        <Text style={[styles.priorityPickerTitle, { color: theme.colors.textSecondary }]}>
          Message Priority
        </Text>
        <View style={styles.priorityPickerOptions}>
          {[
            { p: MessagePriority.Low, label: 'Low', desc: 'Delivered when convenient' },
            { p: MessagePriority.Medium, label: 'Normal', desc: 'Standard delivery' },
            { p: MessagePriority.High, label: 'Urgent', desc: 'Prioritized delivery' },
          ].map(({ p, label, desc }) => (
            <TouchableOpacity
              key={p}
              style={[
                styles.priorityPickerOption,
                {
                  backgroundColor: priority === p ? getPriorityColor(p) + '15' : 'transparent',
                  borderColor: priority === p ? getPriorityColor(p) : theme.colors.border,
                },
              ]}
              onPress={() => selectPriority(p)}
              activeOpacity={0.7}
            >
              <View style={[styles.priorityPickerIcon, { backgroundColor: getPriorityColor(p) }]}>
                <Icon name={getPriorityIcon(p)} size={14} color={theme.colors.textInverse} />
              </View>
              <View style={styles.priorityPickerText}>
                <Text style={[styles.priorityPickerLabel, { color: theme.colors.text }]}>
                  {label}
                </Text>
                <Text style={[styles.priorityPickerDesc, { color: theme.colors.textSecondary }]}>
                  {desc}
                </Text>
              </View>
              {priority === p && (
                <Icon name="checkmark" size={18} color={getPriorityColor(p)} />
              )}
            </TouchableOpacity>
          ))}
        </View>
      </View>
    );
  };

  const renderInputArea = () => (
    <View 
      style={[
        styles.inputContainer,
        { 
          backgroundColor: theme.colors.surface,
          borderTopColor: theme.colors.border,
          paddingBottom: Platform.OS === 'ios' ? Math.max(insets.bottom, 8) : 12,
        },
      ]}
    >
      {renderPriorityPicker()}
      
      {/* Reply Preview */}
      {replyingToMessage && (
        <View style={[styles.replyPreviewContainer, { backgroundColor: theme.colors.background }]}>
          <View style={[styles.replyPreviewContent, { borderLeftColor: theme.colors.primary }]}>
            <View style={styles.replyPreviewInfo}>
              <Text style={[styles.replyPreviewLabel, { color: theme.colors.primary }]}>
                Replying to {replyingToMessage.isFromMe ? 'yourself' : peerName}
              </Text>
              <Text
                style={[styles.replyPreviewMessage, { color: theme.colors.textSecondary }]}
                numberOfLines={1}
              >
                {replyingToMessage.content}
              </Text>
            </View>
            <TouchableOpacity
              onPress={() => setReplyingToMessage(null)}
              hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
            >
              <Icon name="close" size={18} color={theme.colors.textSecondary} />
            </TouchableOpacity>
          </View>
        </View>
      )}
      
      {/* Input Row */}
      <View style={styles.inputRow}>
        {/* Priority Toggle Button */}
        <TouchableOpacity
          style={[
            styles.priorityToggle,
            {
              backgroundColor: showPriorityPicker ? getPriorityColor(priority) : theme.colors.background,
            },
          ]}
          onPress={togglePriorityPicker}
          activeOpacity={0.7}
          hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
        >
          <Icon
            name={getPriorityIcon(priority)}
            size={18}
            color={showPriorityPicker ? theme.colors.textInverse : getPriorityColor(priority)}
          />
        </TouchableOpacity>

        {/* Text Input */}
        <View style={[styles.inputWrapper, { backgroundColor: theme.colors.background }]}>
          <TextInput
            ref={inputRef}
            style={[
              styles.textInput,
              { 
                color: theme.colors.text,
                minHeight: 24,
                maxHeight: 100,
              },
            ]}
            value={inputText}
            onChangeText={setInputText}
            placeholder="Type a message..."
            placeholderTextColor={theme.colors.textSecondary}
            multiline
            maxLength={500}
            textAlignVertical="center"
            onContentSizeChange={handleContentSizeChange}
            onFocus={() => {
              setTimeout(() => {
                flatListRef.current?.scrollToEnd({ animated: true });
              }, 150);
            }}
          />
          {isEncryptedChat && (
            <View style={styles.encryptedIndicator}>
              <Icon name="lock-closed" size={12} color={theme.colors.success} />
            </View>
          )}
        </View>
        
        {/* Send Button */}
        <TouchableOpacity
          style={[
            styles.sendButton,
            {
              backgroundColor: !isOnline
                ? theme.colors.border
                : inputText.trim()
                  ? theme.colors.primary
                  : theme.colors.border,
            },
          ]}
          onPress={handleSend}
          disabled={!inputText.trim() || !isOnline}
          activeOpacity={0.7}
          hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
        >
          <Icon
            name="send"
            size={18}
            color={
              !isOnline
                ? theme.colors.textSecondary
                : inputText.trim()
                  ? theme.colors.textInverse
                  : theme.colors.textSecondary
            }
          />
        </TouchableOpacity>
      </View>
    </View>
  );

  return (
    <View style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {renderHeader()}
      <KeyboardAvoidingView
        style={styles.keyboardAvoidingView}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
        keyboardVerticalOffset={Platform.OS === 'ios' ? 0 : 0}
      >
        <View style={styles.contentContainer}>
          {renderStatusBanner()}
          
          {/* Messages List */}
          <FlatList
            ref={flatListRef}
            data={groupedMessages}
            keyExtractor={(item) => item.id}
            renderItem={renderMessage}
            contentContainerStyle={[
              styles.messagesList,
              groupedMessages.length === 0 && styles.emptyMessagesList,
            ]}
            showsVerticalScrollIndicator={false}
            ListEmptyComponent={renderEmptyState}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode="interactive"
            onContentSizeChange={() => {
              if (groupedMessages.length > 0) {
                flatListRef.current?.scrollToEnd({ animated: false });
              }
            }}
            onLayout={() => {
              if (groupedMessages.length > 0) {
                flatListRef.current?.scrollToEnd({ animated: false });
              }
            }}
            maintainVisibleContentPosition={{
              minIndexForVisible: 0,
              autoscrollToTopThreshold: 100,
            }}
          />
        </View>

        {/* Input Area */}
        {renderInputArea()}
      </KeyboardAvoidingView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  keyboardAvoidingView: {
    flex: 1,
  },
  contentContainer: {
    flex: 1,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: 'rgba(0,0,0,0.1)',
  },
  backButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    width: 40,
    height: 40,
    borderRadius: 20,
    marginRight: 4,
  },
  headerTitle: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
  },
  headerAvatar: {
    width: 38,
    height: 38,
    borderRadius: 19,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 10,
    position: 'relative',
  },
  headerAvatarText: {
    fontSize: 14,
    fontWeight: '600',
  },
  headerOnlineIndicator: {
    position: 'absolute',
    bottom: 0,
    right: 0,
    width: 11,
    height: 11,
    borderRadius: 6,
    borderWidth: 2,
    borderColor: 'white',
  },
  headerInfo: {
    flex: 1,
    justifyContent: 'center',
  },
  headerNameRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  headerName: {
    fontSize: 16,
    fontWeight: '600',
  },
  encryptedBadge: {
    width: 18,
    height: 18,
    borderRadius: 9,
    alignItems: 'center',
    justifyContent: 'center',
  },
  headerStatus: {
    fontSize: 12,
    fontWeight: '500',
    marginTop: 1,
  },
  messagesList: {
    paddingHorizontal: 12,
    paddingTop: 8,
    paddingBottom: 8,
  },
  emptyMessagesList: {
    flex: 1,
  },
  messageContainer: {
    marginVertical: 1,
    maxWidth: '78%',
  },
  myMessageContainer: {
    alignSelf: 'flex-end',
  },
  theirMessageContainer: {
    alignSelf: 'flex-start',
  },
  messageBubble: {
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 18,
    position: 'relative',
  },
  myMessageBubble: {
    borderBottomRightRadius: 4,
  },
  theirMessageBubble: {
    borderBottomLeftRadius: 4,
    ...Platform.select({
      ios: {
        shadowColor: '#000',
        shadowOffset: { width: 0, height: 1 },
        shadowOpacity: 0.04,
        shadowRadius: 2,
      },
      android: {
        elevation: 1,
      },
    }),
  },
  firstInGroup: {
    marginTop: 6,
  },
  lastInGroup: {
    marginBottom: 6,
  },
  messageText: {
    fontSize: 16,
    lineHeight: 21,
    marginBottom: 3,
  },
  messageFooter: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    marginTop: -1,
  },
  messageTime: {
    fontSize: 11,
    fontWeight: '400',
  },
  priorityIndicator: {
    position: 'absolute',
    top: -2,
    right: -2,
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  emptyState: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
  },
  emptyAvatar: {
    width: 72,
    height: 72,
    borderRadius: 36,
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 20,
  },
  emptyAvatarText: {
    fontSize: 26,
    fontWeight: '600',
  },
  emptyTitle: {
    fontSize: 18,
    fontWeight: '600',
    marginBottom: 8,
    textAlign: 'center',
  },
  emptySubtitle: {
    fontSize: 15,
    textAlign: 'center',
    lineHeight: 21,
  },
  statusBanner: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    marginHorizontal: 12,
    marginTop: 8,
    marginBottom: 4,
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderRadius: 10,
    borderWidth: 1,
  },
  statusBannerTitle: {
    fontSize: 13,
    fontWeight: '600',
    marginBottom: 2,
  },
  statusBannerSubtitle: {
    fontSize: 12,
    lineHeight: 16,
  },
  inputContainer: {
    borderTopWidth: StyleSheet.hairlineWidth,
    paddingHorizontal: 12,
    paddingTop: 10,
  },
  priorityPickerContainer: {
    marginBottom: 10,
    paddingBottom: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: 'rgba(0,0,0,0.08)',
  },
  priorityPickerTitle: {
    fontSize: 12,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    marginBottom: 10,
  },
  priorityPickerOptions: {
    gap: 8,
  },
  priorityPickerOption: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 10,
    borderWidth: 1,
  },
  priorityPickerIcon: {
    width: 28,
    height: 28,
    borderRadius: 14,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 12,
  },
  priorityPickerText: {
    flex: 1,
  },
  priorityPickerLabel: {
    fontSize: 15,
    fontWeight: '600',
  },
  priorityPickerDesc: {
    fontSize: 12,
    marginTop: 1,
  },
  inputRow: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
  },
  priorityToggle: {
    width: 36,
    height: 36,
    borderRadius: 18,
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 2,
  },
  inputWrapper: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    borderRadius: 20,
    paddingHorizontal: 14,
    paddingVertical: Platform.OS === 'ios' ? 10 : 6,
    minHeight: 40,
    maxHeight: 120,
  },
  textInput: {
    flex: 1,
    fontSize: 16,
    lineHeight: 20,
    paddingTop: 0,
    paddingBottom: 0,
  },
  encryptedIndicator: {
    marginLeft: 6,
    opacity: 0.8,
  },
  sendButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 2,
  },
  replyPreview: {
    marginBottom: 8,
    paddingLeft: 10,
    paddingRight: 8,
    paddingVertical: 6,
    borderRadius: 8,
    borderLeftWidth: 3,
  },
  replyPreviewSender: {
    fontSize: 12,
    fontWeight: '600',
    marginBottom: 2,
    opacity: 0.9,
  },
  replyPreviewText: {
    fontSize: 13,
    opacity: 0.8,
  },
  replyPreviewContainer: {
    marginBottom: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 10,
  },
  replyPreviewContent: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingLeft: 10,
    borderLeftWidth: 3,
  },
  replyPreviewInfo: {
    flex: 1,
    marginRight: 8,
  },
  replyPreviewLabel: {
    fontSize: 12,
    fontWeight: '600',
    marginBottom: 2,
  },
  replyPreviewMessage: {
    fontSize: 13,
  },
});
