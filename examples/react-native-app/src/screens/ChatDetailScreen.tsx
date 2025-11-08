import React, { useState, useEffect, useRef } from 'react';
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
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
// import { useRoute, useNavigation } from '@react-navigation/native';
import { Icon } from '../components/Icon';
import LinearGradient from 'react-native-linear-gradient';
// import Animated, { FadeInUp, FadeInDown, SlideInRight } from 'react-native-reanimated';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { Message } from '../providers/ProtocolProvider';
import { MessagePriority } from '@offlineprotocol/react-native';
import { getUserInitials, generateAvatarColor } from '../utils/user';

interface MessageBubbleProps {
  message: Message;
  isLastInGroup: boolean;
  isFirstInGroup: boolean;
}

function MessageBubble({ message, isLastInGroup, isFirstInGroup }: MessageBubbleProps) {
  const { theme } = useTheme();
  const isFromMe = message.isFromMe;
  
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
  
  const { chats, contacts, sendMessage, currentUserId } = useProtocol();
  const [inputText, setInputText] = useState('');
  const [priority, setPriority] = useState<MessagePriority>(MessagePriority.Medium);
  const [keyboardHeight, setKeyboardHeight] = useState(0);
  
  const flatListRef = useRef<FlatList>(null);
  const inputRef = useRef<TextInput>(null);

  const chat = chats.find(c => c.peerId === peerId);
  const contact = contacts.find(c => c.id === peerId);
  const messages = chat?.messages || [];
  
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
          <Text style={[styles.headerName, { color: theme.colors.text }]}>
            {peerName}
          </Text>
          <Text style={[styles.headerStatus, { color: theme.colors.textSecondary }]}>
            {contact?.isOnline ? 'Online' : 'Offline'}
          </Text>
        </View>
      </TouchableOpacity>
      
      <View style={{ width: 40 }} />
    </View>
  );

  useEffect(() => {
    const keyboardWillShow = Keyboard.addListener(
      Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow',
      (e) => setKeyboardHeight(e.endCoordinates.height)
    );
    
    const keyboardWillHide = Keyboard.addListener(
      Platform.OS === 'ios' ? 'keyboardWillHide' : 'keyboardDidHide',
      () => setKeyboardHeight(0)
    );

    return () => {
      keyboardWillShow.remove();
      keyboardWillHide.remove();
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

  const handleSend = async () => {
    const text = inputText.trim();
    if (!text) return;

    try {
      await sendMessage(peerId, text, priority);
      setInputText('');
      inputRef.current?.blur();
    } catch (error) {
      Alert.alert('Send Failed', 'Failed to send message. Please try again.');
    }
  };

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

  return (
    <View style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {renderHeader()}
      <KeyboardAvoidingView
        style={{ flex: 1 }}
        behavior={Platform.OS === 'ios' ? 'padding' : 'height'}
        keyboardVerticalOffset={Platform.OS === 'ios' ? 90 : 0}
      >
        {/* Messages */}
        <FlatList
          ref={flatListRef}
          data={groupedMessages}
          keyExtractor={(item) => item.id}
          renderItem={renderMessage}
          contentContainerStyle={[
            styles.messagesList,
            groupedMessages.length === 0 && { flex: 1 },
          ]}
          showsVerticalScrollIndicator={false}
          ListEmptyComponent={renderEmptyState}
          onContentSizeChange={() => {
            if (groupedMessages.length > 0) {
              flatListRef.current?.scrollToEnd({ animated: false });
            }
          }}
        />

        {/* Input Area */}
        <View 
          style={[
            styles.inputContainer,
            { 
              backgroundColor: theme.colors.surface,
              borderTopColor: theme.colors.border,
              marginBottom: keyboardHeight > 0 ? keyboardHeight - 20 : 0,
            },
          ]}
        >
          {/* Priority Selector */}
          <View style={styles.priorityContainer}>
            {[MessagePriority.Low, MessagePriority.Medium, MessagePriority.High].map((p) => (
              <TouchableOpacity
                key={p}
                style={[
                  styles.priorityButton,
                  {
                    backgroundColor: priority === p 
                      ? getPriorityColor(p) 
                      : theme.colors.background,
                  },
                ]}
                onPress={() => setPriority(p)}
                activeOpacity={0.7}
              >
                <Icon
                  name={getPriorityIcon(p)}
                  size={16}
                  color={priority === p ? theme.colors.textInverse : getPriorityColor(p)}
                />
              </TouchableOpacity>
            ))}
          </View>

          {/* Input Row */}
          <View style={styles.inputRow}>
            <View style={[styles.inputWrapper, { backgroundColor: theme.colors.background }]}>
              <TextInput
                ref={inputRef}
                style={[styles.textInput, { color: theme.colors.text }]}
                value={inputText}
                onChangeText={setInputText}
                placeholder="Type a message..."
                placeholderTextColor={theme.colors.textSecondary}
                multiline
                maxLength={500}
                returnKeyType="send"
                onSubmitEditing={handleSend}
                blurOnSubmit={false}
              />
            </View>
            
            <TouchableOpacity
              style={[
                styles.sendButton,
                {
                  backgroundColor: inputText.trim() ? theme.colors.primary : theme.colors.border,
                },
              ]}
              onPress={handleSend}
              disabled={!inputText.trim()}
              activeOpacity={0.8}
            >
              <Icon
                name="send"
                size={20}
                color={inputText.trim() ? theme.colors.textInverse : theme.colors.textSecondary}
              />
            </TouchableOpacity>
          </View>
        </View>
      </KeyboardAvoidingView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(0,0,0,0.1)',
  },
  backButton: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 8,
    paddingRight: 12,
  },
  headerTitle: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  headerAvatar: {
    width: 36,
    height: 36,
    borderRadius: 18,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 12,
    position: 'relative',
  },
  headerAvatarText: {
    fontSize: 14,
    fontWeight: '600',
  },
  headerOnlineIndicator: {
    position: 'absolute',
    bottom: -1,
    right: -1,
    width: 12,
    height: 12,
    borderRadius: 6,
    borderWidth: 2,
    borderColor: 'white',
  },
  headerInfo: {
    flex: 1,
  },
  headerName: {
    fontSize: 16,
    fontWeight: '600',
  },
  headerStatus: {
    fontSize: 12,
    fontWeight: '500',
  },
  messagesList: {
    paddingHorizontal: 16,
    paddingVertical: 8,
  },
  messageContainer: {
    marginVertical: 2,
    maxWidth: '80%',
  },
  myMessageContainer: {
    alignSelf: 'flex-end',
  },
  theirMessageContainer: {
    alignSelf: 'flex-start',
  },
  messageBubble: {
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderRadius: 20,
    position: 'relative',
  },
  myMessageBubble: {
    borderBottomRightRadius: 6,
  },
  theirMessageBubble: {
    borderBottomLeftRadius: 6,
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
  firstInGroup: {
    marginTop: 8,
  },
  lastInGroup: {
    marginBottom: 8,
  },
  messageText: {
    fontSize: 16,
    lineHeight: 20,
    marginBottom: 4,
  },
  messageFooter: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
  },
  messageTime: {
    fontSize: 11,
    fontWeight: '500',
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
    width: 80,
    height: 80,
    borderRadius: 40,
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 24,
  },
  emptyAvatarText: {
    fontSize: 28,
    fontWeight: '600',
  },
  emptyTitle: {
    fontSize: 20,
    fontWeight: '600',
    marginBottom: 8,
    textAlign: 'center',
  },
  emptySubtitle: {
    fontSize: 16,
    textAlign: 'center',
    lineHeight: 22,
  },
  inputContainer: {
    borderTopWidth: 1,
    paddingHorizontal: 16,
    paddingTop: 12,
    paddingBottom: 20,
  },
  priorityContainer: {
    flexDirection: 'row',
    marginBottom: 12,
    gap: 8,
  },
  priorityButton: {
    width: 32,
    height: 32,
    borderRadius: 16,
    alignItems: 'center',
    justifyContent: 'center',
  },
  inputRow: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 12,
  },
  inputWrapper: {
    flex: 1,
    borderRadius: 20,
    paddingHorizontal: 16,
    paddingVertical: 12,
    maxHeight: 100,
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
  textInput: {
    fontSize: 16,
    lineHeight: 20,
    maxHeight: 76,
  },
  sendButton: {
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
});
