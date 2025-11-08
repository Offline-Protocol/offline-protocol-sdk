import React, { useCallback, useMemo, useRef, useState, useEffect } from 'react';
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  KeyboardAvoidingView,
  Platform,
  SafeAreaView,
  FlatList,
  Keyboard,
  InputAccessoryView,
  Alert,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { MessagePriority, type ProtocolEvent } from '@offlineprotocol/react-native';

interface Message {
  id: string;
  type: 'sent' | 'received';
  content: string;
  timestamp: number;
  status: 'pending' | 'delivered' | 'failed';
  priority?: string;
}

interface ChatScreenProps {
  peerId: string;
  peerDisplayName?: string;
  currentUserId: string;
  events: ProtocolEvent[];
  isStarted: boolean;
  onSendMessage: (recipient: string, content: string, priority: MessagePriority) => Promise<void>;
  onGoBack: () => void;
}

export function ChatScreen({
  peerId,
  peerDisplayName,
  currentUserId,
  events,
  isStarted,
  onSendMessage,
  onGoBack,
}: ChatScreenProps) {
  const [message, setMessage] = useState('');
  const [sending, setSending] = useState(false);
  const [priority, setPriority] = useState<MessagePriority>(MessagePriority.Medium);
  
  const messageInputRef = useRef<TextInput | null>(null);
  const flatListRef = useRef<FlatList<Message> | null>(null);
  const insets = useSafeAreaInsets();
  
  const inputAccessoryViewID = 'chatInputAccessory';

  // Filter messages for this specific peer
  const peerMessages: Message[] = useMemo(() => {
    const messageMap = new Map<string, Message>();

    [...events].reverse().forEach((event) => {
      if (event.type === 'message_sent') {
        const e = event as any;
        if (e.recipient === peerId) {
          messageMap.set(e.message_id, {
            id: e.message_id,
            type: 'sent',
            content: e.content,
            timestamp: e.timestamp,
            status: 'delivered',
            priority: e.priority,
          });
        }
      } else if (event.type === 'message_received') {
        const e = event as any;
        if (e.sender === peerId) {
          messageMap.set(e.message_id, {
            id: e.message_id,
            type: 'received',
            content: e.content,
            timestamp: e.timestamp,
            status: 'delivered',
          });
        }
      } else if (event.type === 'message_delivered') {
        const e = event as any;
        const msg = messageMap.get(e.message_id);
        if (msg) {
          msg.status = 'delivered';
        }
      } else if (event.type === 'message_failed') {
        const e = event as any;
        const msg = messageMap.get(e.message_id);
        if (msg) {
          msg.status = 'failed';
        }
      }
    });

    return Array.from(messageMap.values()).sort((a, b) => a.timestamp - b.timestamp);
  }, [events, peerId]);

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    if (peerMessages.length > 0) {
      setTimeout(() => {
        flatListRef.current?.scrollToEnd({ animated: true });
      }, 100);
    }
  }, [peerMessages.length]);

  const handleSend = useCallback(async () => {
    const trimmedMessage = message.trim();
    if (!isStarted || sending || !trimmedMessage) {
      return;
    }

    setSending(true);
    try {
      await onSendMessage(peerId, trimmedMessage, priority);
      setMessage('');
      Keyboard.dismiss();
    } catch (error) {
      Alert.alert('Error', 'Failed to send message. Please try again.');
    } finally {
      setSending(false);
    }
  }, [isStarted, message, onSendMessage, peerId, priority, sending]);

  const formatTimestamp = (timestamp: number): string => {
    const date = new Date(timestamp);
    const now = new Date();
    const isToday = date.toDateString() === now.toDateString();
    
    if (isToday) {
      return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } else {
      return date.toLocaleDateString([], { month: 'short', day: 'numeric' }) + 
             ' ' + date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    }
  };

  const renderInputAccessory = () => (
    <InputAccessoryView nativeID={inputAccessoryViewID}>
      <View style={styles.inputAccessory}>
        <TouchableOpacity
          style={styles.accessoryButton}
          onPress={() => Keyboard.dismiss()}
        >
          <Text style={styles.accessoryButtonText}>Done</Text>
        </TouchableOpacity>
        {isStarted && message.trim() && (
          <TouchableOpacity
            style={[styles.accessoryButton, styles.accessoryButtonPrimary]}
            onPress={handleSend}
            disabled={sending}
          >
            <Text style={[styles.accessoryButtonText, styles.accessoryButtonTextPrimary]}>
              {sending ? 'Sending…' : 'Send'}
            </Text>
          </TouchableOpacity>
        )}
      </View>
    </InputAccessoryView>
  );

  const renderMessage = ({ item }: { item: Message }) => {
    const isSent = item.type === 'sent';
    
    return (
      <View style={[styles.messageContainer, isSent ? styles.sentContainer : styles.receivedContainer]}>
        <View style={[styles.messageBubble, isSent ? styles.sentBubble : styles.receivedBubble]}>
          <Text style={[styles.messageText, isSent ? styles.sentText : styles.receivedText]}>
            {item.content}
          </Text>
          <View style={styles.messageFooter}>
            <Text style={[styles.timestamp, isSent ? styles.sentTimestamp : styles.receivedTimestamp]}>
              {formatTimestamp(item.timestamp)}
            </Text>
            {isSent && (
              <View style={styles.statusContainer}>
                {item.status === 'delivered' && <Text style={styles.checkmark}>✓✓</Text>}
                {item.status === 'pending' && <Text style={styles.pending}>⏰</Text>}
                {item.status === 'failed' && <Text style={styles.failed}>⚠</Text>}
              </View>
            )}
          </View>
        </View>
      </View>
    );
  };

  const keyExtractor = (item: Message) => item.id;

  const displayName = peerDisplayName || peerId.slice(-8);
  const isSendDisabled = !isStarted || sending || !message.trim();

  return (
    <>
      {renderInputAccessory()}
      <KeyboardAvoidingView
        style={styles.container}
        behavior={Platform.OS === 'ios' ? 'padding' : 'height'}
        keyboardVerticalOffset={Platform.OS === 'ios' ? 0 : 20}
      >
        <SafeAreaView style={styles.safeArea}>
          {/* Header */}
          <View style={[styles.header, { paddingTop: Math.max(insets.top, 12) }]}>
            <TouchableOpacity
              style={styles.backButton}
              onPress={onGoBack}
            >
              <Text style={styles.backButtonText}>←</Text>
            </TouchableOpacity>
            
            <View style={styles.headerCenter}>
              <Text style={styles.peerName}>{displayName}</Text>
              <Text style={styles.peerStatus}>
                {isStarted ? 'Online' : 'Offline'} • {peerMessages.length} messages
              </Text>
            </View>
            
            <View style={styles.headerRight}>
              <View style={[styles.onlineIndicator, isStarted ? styles.online : styles.offline]} />
            </View>
          </View>

          {/* Messages */}
          <View style={styles.messagesContainer}>
            {peerMessages.length > 0 ? (
              <FlatList
                ref={flatListRef}
                data={peerMessages}
                keyExtractor={keyExtractor}
                renderItem={renderMessage}
                contentContainerStyle={[
                  styles.messagesList,
                  { paddingBottom: Math.max(insets.bottom, 20) }
                ]}
                showsVerticalScrollIndicator={false}
                keyboardShouldPersistTaps="handled"
              />
            ) : (
              <View style={styles.emptyState}>
                <Text style={styles.emptyTitle}>No messages yet</Text>
                <Text style={styles.emptySubtitle}>
                  Start the conversation with {displayName}
                </Text>
              </View>
            )}
          </View>

          {/* Composer */}
          <View style={[styles.composer, { paddingBottom: Math.max(insets.bottom, 12) }]}>
            <View style={styles.inputContainer}>
              <TextInput
                ref={messageInputRef}
                style={[styles.messageInput, !isStarted && styles.messageInputDisabled]}
                value={message}
                onChangeText={setMessage}
                placeholder={isStarted ? "Type a message..." : "Start protocol to send messages"}
                placeholderTextColor="#9ca3af"
                multiline
                maxLength={500}
                editable={isStarted}
                returnKeyType="send"
                onSubmitEditing={handleSend}
                inputAccessoryViewID={inputAccessoryViewID}
              />
              
              <TouchableOpacity
                style={[styles.sendButton, isSendDisabled && styles.sendButtonDisabled]}
                onPress={handleSend}
                disabled={isSendDisabled}
              >
                <Text style={styles.sendButtonText}>
                  {sending ? '⏳' : '→'}
                </Text>
              </TouchableOpacity>
            </View>
            
            {/* Quick priority selector */}
            <View style={styles.prioritySelector}>
              <Text style={styles.priorityLabel}>Priority:</Text>
              {[
                { label: 'L', value: MessagePriority.Low },
                { label: 'M', value: MessagePriority.Medium },
                { label: 'H', value: MessagePriority.High },
                { label: 'C', value: MessagePriority.Critical },
              ].map((option) => (
                <TouchableOpacity
                  key={option.value}
                  style={[
                    styles.priorityButton,
                    priority === option.value && styles.priorityButtonActive,
                    !isStarted && styles.priorityButtonDisabled,
                  ]}
                  onPress={() => setPriority(option.value)}
                  disabled={!isStarted}
                >
                  <Text
                    style={[
                      styles.priorityButtonText,
                      priority === option.value && styles.priorityButtonTextActive,
                    ]}
                  >
                    {option.label}
                  </Text>
                </TouchableOpacity>
              ))}
            </View>
          </View>
        </SafeAreaView>
      </KeyboardAvoidingView>
    </>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f0f2f5',
  },
  safeArea: {
    flex: 1,
  },
  header: {
    backgroundColor: '#075e54',
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 12,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.1,
    shadowRadius: 4,
    elevation: 4,
  },
  backButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    backgroundColor: 'rgba(255, 255, 255, 0.2)',
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 12,
  },
  backButtonText: {
    fontSize: 20,
    fontWeight: '600',
    color: '#ffffff',
  },
  headerCenter: {
    flex: 1,
  },
  peerName: {
    fontSize: 18,
    fontWeight: '600',
    color: '#ffffff',
    marginBottom: 2,
  },
  peerStatus: {
    fontSize: 12,
    color: 'rgba(255, 255, 255, 0.8)',
  },
  headerRight: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  onlineIndicator: {
    width: 12,
    height: 12,
    borderRadius: 6,
    borderWidth: 2,
    borderColor: '#ffffff',
  },
  online: {
    backgroundColor: '#25d366',
  },
  offline: {
    backgroundColor: '#9ca3af',
  },
  messagesContainer: {
    flex: 1,
  },
  messagesList: {
    paddingHorizontal: 16,
    paddingTop: 16,
  },
  emptyState: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
  },
  emptyTitle: {
    fontSize: 18,
    fontWeight: '600',
    color: '#374151',
    marginBottom: 8,
  },
  emptySubtitle: {
    fontSize: 14,
    color: '#6b7280',
    textAlign: 'center',
    lineHeight: 20,
  },
  messageContainer: {
    marginBottom: 12,
    maxWidth: '80%',
  },
  sentContainer: {
    alignSelf: 'flex-end',
  },
  receivedContainer: {
    alignSelf: 'flex-start',
  },
  messageBubble: {
    borderRadius: 18,
    paddingHorizontal: 16,
    paddingVertical: 10,
    maxWidth: '100%',
  },
  sentBubble: {
    backgroundColor: '#dcf8c6',
    borderBottomRightRadius: 4,
  },
  receivedBubble: {
    backgroundColor: '#ffffff',
    borderBottomLeftRadius: 4,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.1,
    shadowRadius: 2,
    elevation: 1,
  },
  messageText: {
    fontSize: 16,
    lineHeight: 22,
    marginBottom: 4,
  },
  sentText: {
    color: '#1f2937',
  },
  receivedText: {
    color: '#1f2937',
  },
  messageFooter: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 4,
  },
  timestamp: {
    fontSize: 11,
    fontWeight: '500',
  },
  sentTimestamp: {
    color: '#6b7280',
  },
  receivedTimestamp: {
    color: '#9ca3af',
  },
  statusContainer: {
    marginLeft: 4,
  },
  checkmark: {
    fontSize: 12,
    color: '#25d366',
  },
  pending: {
    fontSize: 10,
    color: '#f59e0b',
  },
  failed: {
    fontSize: 10,
    color: '#ef4444',
  },
  composer: {
    backgroundColor: '#ffffff',
    paddingHorizontal: 16,
    paddingTop: 12,
    borderTopWidth: 1,
    borderTopColor: '#e5e7eb',
    shadowColor: '#000',
    shadowOffset: { width: 0, height: -2 },
    shadowOpacity: 0.05,
    shadowRadius: 8,
    elevation: 8,
  },
  inputContainer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
    marginBottom: 12,
  },
  messageInput: {
    flex: 1,
    borderWidth: 1,
    borderColor: '#d1d5db',
    borderRadius: 20,
    paddingHorizontal: 16,
    paddingVertical: 10,
    fontSize: 16,
    color: '#1f2937',
    backgroundColor: '#f9fafb',
    maxHeight: 100,
    textAlignVertical: 'top',
  },
  messageInputDisabled: {
    backgroundColor: '#f3f4f6',
    color: '#9ca3af',
  },
  sendButton: {
    width: 44,
    height: 44,
    borderRadius: 22,
    backgroundColor: '#075e54',
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: '#075e54',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.3,
    shadowRadius: 4,
    elevation: 4,
  },
  sendButtonDisabled: {
    backgroundColor: '#d1d5db',
    shadowOpacity: 0,
    elevation: 0,
  },
  sendButtonText: {
    fontSize: 20,
    fontWeight: '600',
    color: '#ffffff',
  },
  prioritySelector: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingBottom: 4,
  },
  priorityLabel: {
    fontSize: 12,
    fontWeight: '500',
    color: '#6b7280',
  },
  priorityButton: {
    width: 28,
    height: 28,
    borderRadius: 14,
    backgroundColor: '#f3f4f6',
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: '#e5e7eb',
  },
  priorityButtonActive: {
    backgroundColor: '#075e54',
    borderColor: '#075e54',
  },
  priorityButtonDisabled: {
    backgroundColor: '#f9fafb',
    borderColor: '#f3f4f6',
  },
  priorityButtonText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#6b7280',
  },
  priorityButtonTextActive: {
    color: '#ffffff',
  },
  inputAccessory: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    alignItems: 'center',
    backgroundColor: '#f8fafc',
    borderTopWidth: 1,
    borderTopColor: '#e2e8f0',
    paddingHorizontal: 16,
    paddingVertical: 8,
    gap: 12,
  },
  accessoryButton: {
    paddingHorizontal: 16,
    paddingVertical: 8,
    borderRadius: 16,
    backgroundColor: '#e2e8f0',
  },
  accessoryButtonPrimary: {
    backgroundColor: '#075e54',
  },
  accessoryButtonText: {
    fontSize: 14,
    fontWeight: '600',
    color: '#475569',
  },
  accessoryButtonTextPrimary: {
    color: '#ffffff',
  },
});
