import React, { useState, useMemo, useRef } from 'react';
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  ScrollView,
  KeyboardAvoidingView,
  Platform,
  Keyboard,
  TouchableWithoutFeedback,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { MessagePriority, type ProtocolEvent } from '@offlineprotocol/react-native';
import { MessageList } from '../components/MessageList';

interface MessagingScreenProps {
  events: ProtocolEvent[];
  currentUserId: string;
  onSendMessage: (recipient: string, content: string, priority: MessagePriority) => Promise<void>;
  isStarted: boolean;
}

export function MessagingScreen({
  events,
  currentUserId,
  onSendMessage,
  isStarted,
}: MessagingScreenProps) {
  const [recipient, setRecipient] = useState('');
  const [message, setMessage] = useState('');
  const [priority, setPriority] = useState<MessagePriority>(MessagePriority.Medium);
  const [sending, setSending] = useState(false);
  const messageInputRef = useRef<TextInput | null>(null);
  const keyboardVerticalOffset = Platform.OS === 'ios' ? 120 : 0;

  // Get list of discovered neighbors from events
  const discoveredPeers = useMemo(() => {
    const peers = new Set<string>();
    events.forEach((event) => {
      if (event.type === 'neighbor_discovered') {
        peers.add((event as any).peer_id);
      } else if (event.type === 'neighbor_lost') {
        peers.delete((event as any).peer_id);
      }
    });
    return Array.from(peers);
  }, [events]);

  const handleSend = async () => {
    if (!recipient.trim() || !message.trim()) {
      return;
    }

    setSending(true);
    try {
      await onSendMessage(recipient.trim(), message.trim(), priority);
      setMessage('');
      Keyboard.dismiss();
    } finally {
      setSending(false);
    }
  };

  const priorityOptions = [
    { label: 'Low', value: MessagePriority.Low },
    { label: 'Medium', value: MessagePriority.Medium },
    { label: 'High', value: MessagePriority.High },
    { label: 'Critical', value: MessagePriority.Critical },
  ];

  return (
    <KeyboardAvoidingView
      style={styles.container}
      behavior={Platform.OS === 'ios' ? 'padding' : 'height'}
      keyboardVerticalOffset={keyboardVerticalOffset}
    >
      <TouchableWithoutFeedback onPress={Keyboard.dismiss} accessible={false}>
        <SafeAreaView style={styles.inner}>
          <View style={styles.messagesSection}>
            <MessageList events={events} currentUserId={currentUserId} />
          </View>

          <View style={styles.composerWrapper}>
            <View style={styles.recipientSection}>
              <View style={styles.recipientRow}>
                <TextInput
                  style={styles.recipientInput}
                  value={recipient}
                  onChangeText={setRecipient}
                  placeholder={
                    isStarted
                      ? 'Select a nearby peer or enter their user ID'
                      : 'Start the protocol to discover peers'
                  }
                  placeholderTextColor="#999"
                  editable={isStarted}
                  autoCapitalize="none"
                  autoCorrect={false}
                  returnKeyType="next"
                  onSubmitEditing={() => messageInputRef.current?.focus()}
                />
                {recipient.length > 0 && (
                  <TouchableOpacity
                    style={styles.clearRecipientButton}
                    onPress={() => setRecipient('')}
                  >
                    <Text style={styles.clearRecipientText}>✕</Text>
                  </TouchableOpacity>
                )}
              </View>

              {isStarted && discoveredPeers.length > 0 && (
                <ScrollView
                  horizontal
                  showsHorizontalScrollIndicator={false}
                  contentContainerStyle={styles.peersScrollContent}
                  style={styles.peerScroller}
                >
                  {discoveredPeers.map((peerId) => (
                    <TouchableOpacity
                      key={peerId}
                      style={styles.peerChip}
                      onPress={() => setRecipient(peerId)}
                    >
                      <Text style={styles.peerChipText} numberOfLines={1}>
                        {peerId.substring(0, 18)}
                      </Text>
                    </TouchableOpacity>
                  ))}
                </ScrollView>
              )}

              {!isStarted && (
                <View style={styles.infoBox}>
                  <Text style={styles.infoTitle}>📱 How to Connect</Text>
                  <Text style={styles.infoText}>
                    {[
                      '1. Tap "Start Protocol" to begin',
                      '2. Others nearby will be auto-discovered',
                      '3. Check the "Network" tab to see connected peers',
                      '4. Copy their User ID to send messages',
                    ].join('\n')}
                  </Text>
                </View>
              )}
            </View>

            <View style={styles.prioritySection}>
              <Text style={styles.sectionLabel}>Priority</Text>
              <View style={styles.priorityContainer}>
                {priorityOptions.map((option) => (
                  <TouchableOpacity
                    key={option.value}
                    style={[
                      styles.priorityButton,
                      priority === option.value && styles.priorityButtonActive,
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

            <View style={styles.composeBar}>
              <TextInput
                style={styles.composeInput}
                ref={messageInputRef}
                value={message}
                onChangeText={setMessage}
                placeholder={isStarted ? 'Type a message' : 'Start the protocol to chat'}
                placeholderTextColor="#999"
                multiline
                editable={isStarted}
                textAlignVertical="top"
                returnKeyType="send"
                blurOnSubmit
                onSubmitEditing={handleSend}
              />
              <TouchableOpacity
                style={[
                  styles.sendButtonCircle,
                  ((!isStarted || sending) || !recipient.trim() || !message.trim()) &&
                    styles.sendButtonCircleDisabled,
                ]}
                onPress={handleSend}
                disabled={!isStarted || sending || !recipient.trim() || !message.trim()}
              >
                <Text style={styles.sendIcon}>{sending ? '…' : '➤'}</Text>
              </TouchableOpacity>
            </View>

            {!isStarted && (
              <Text style={styles.warningText}>Start the protocol to send messages</Text>
            )}
          </View>
        </SafeAreaView>
      </TouchableWithoutFeedback>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  inner: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  messagesSection: {
    flex: 1,
    paddingHorizontal: 16,
    paddingTop: 8,
    paddingBottom: 4,
    backgroundColor: '#f5f5f5',
  },
  composerWrapper: {
    borderTopWidth: 1,
    borderTopColor: '#e0e0e0',
    backgroundColor: '#fff',
    paddingHorizontal: 16,
    paddingTop: 12,
    paddingBottom: 12,
    gap: 12,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: -2 },
    shadowOpacity: 0.05,
    shadowRadius: 4,
    elevation: 3,
  },
  recipientSection: {
    gap: 10,
  },
  recipientRow: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  recipientInput: {
    flex: 1,
    backgroundColor: '#fff',
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 20,
    paddingHorizontal: 16,
    paddingVertical: 10,
    fontSize: 14,
    color: '#333',
  },
  clearRecipientButton: {
    marginLeft: 8,
    width: 32,
    height: 32,
    borderRadius: 16,
    backgroundColor: '#eef1f5',
    alignItems: 'center',
    justifyContent: 'center',
  },
  clearRecipientText: {
    fontSize: 12,
    color: '#555',
  },
  peerScroller: {
    maxHeight: 48,
  },
  peersScrollContent: {
    alignItems: 'center',
    paddingRight: 12,
  },
  peerChip: {
    backgroundColor: '#2196f3',
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderRadius: 18,
    marginRight: 8,
  },
  peerChipText: {
    color: '#fff',
    fontSize: 12,
    fontWeight: '600',
  },
  infoBox: {
    backgroundColor: '#e3f2fd',
    padding: 16,
    borderRadius: 8,
    borderLeftWidth: 4,
    borderLeftColor: '#2196f3',
  },
  infoTitle: {
    fontSize: 14,
    fontWeight: '600',
    color: '#333',
    marginBottom: 8,
  },
  infoText: {
    fontSize: 14,
    color: '#424242',
    lineHeight: 20,
  },
  prioritySection: {
    gap: 8,
  },
  sectionLabel: {
    fontSize: 13,
    fontWeight: '600',
    color: '#666',
  },
  priorityContainer: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
  },
  priorityButton: {
    flexGrow: 1,
    flexBasis: '30%',
    paddingVertical: 10,
    borderRadius: 20,
    borderWidth: 1,
    borderColor: '#ddd',
    backgroundColor: '#fff',
    alignItems: 'center',
  },
  priorityButtonActive: {
    backgroundColor: '#2196f3',
    borderColor: '#2196f3',
  },
  priorityButtonText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#666',
  },
  priorityButtonTextActive: {
    color: '#fff',
  },
  composeBar: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 12,
  },
  composeInput: {
    flex: 1,
    minHeight: 44,
    maxHeight: 120,
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 22,
    paddingHorizontal: 16,
    paddingVertical: 10,
    backgroundColor: '#fff',
    fontSize: 15,
    color: '#333',
  },
  sendButtonCircle: {
    width: 48,
    height: 48,
    borderRadius: 24,
    backgroundColor: '#2196f3',
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.15,
    shadowRadius: 3,
    elevation: 4,
  },
  sendButtonCircleDisabled: {
    backgroundColor: '#b0bec5',
  },
  sendIcon: {
    fontSize: 20,
    color: '#fff',
    marginTop: -2,
  },
  warningText: {
    marginTop: 4,
    fontSize: 12,
    color: '#ff9800',
    textAlign: 'center',
  },
});

