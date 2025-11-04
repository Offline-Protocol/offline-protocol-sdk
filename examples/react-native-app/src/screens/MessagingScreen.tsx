import React, { useState, useMemo } from 'react';
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  ScrollView,
  KeyboardAvoidingView,
  Platform,
} from 'react-native';
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
      keyboardVerticalOffset={Platform.OS === 'ios' ? 100 : 0}
    >
      <ScrollView 
        style={styles.scrollView}
        contentContainerStyle={styles.scrollContent}
        keyboardShouldPersistTaps="handled"
      >
        <View style={styles.formContainer}>
          <Text style={styles.sectionTitle}>Send Message</Text>
          
          {!isStarted && (
            <View style={styles.infoBox}>
              <Text style={styles.infoTitle}>📱 How to Connect</Text>
              <Text style={styles.infoText}>
                1. Tap "Start Protocol" to begin{'\n'}
                2. Others nearby will be auto-discovered{'\n'}
                3. Check the "Network" tab to see connected peers{'\n'}
                4. Copy their User ID to send messages
              </Text>
            </View>
          )}

          {isStarted && discoveredPeers.length > 0 && (
            <View style={styles.peersBox}>
              <Text style={styles.peersTitle}>📡 Nearby Peers ({discoveredPeers.length})</Text>
              <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                {discoveredPeers.map((peerId) => (
                  <TouchableOpacity
                    key={peerId}
                    style={styles.peerChip}
                    onPress={() => setRecipient(peerId)}
                  >
                    <Text style={styles.peerChipText} numberOfLines={1}>
                      {peerId.substring(0, 12)}...
                    </Text>
                  </TouchableOpacity>
                ))}
              </ScrollView>
            </View>
          )}
          
          <Text style={styles.label}>Recipient ID</Text>
          <TextInput
            style={styles.input}
            value={recipient}
            onChangeText={setRecipient}
            placeholder="Enter recipient user ID"
            placeholderTextColor="#999"
            editable={isStarted}
            autoCapitalize="none"
            autoCorrect={false}
          />

          <Text style={styles.label}>Message</Text>
          <TextInput
            style={[styles.input, styles.messageInput]}
            value={message}
            onChangeText={setMessage}
            placeholder="Enter your message"
            placeholderTextColor="#999"
            multiline
            numberOfLines={3}
            editable={isStarted}
            textAlignVertical="top"
          />

          <Text style={styles.label}>Priority</Text>
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

          <TouchableOpacity
            style={[styles.sendButton, (!isStarted || sending) && styles.sendButtonDisabled]}
            onPress={handleSend}
            disabled={!isStarted || sending}
          >
            <Text style={styles.sendButtonText}>
              {sending ? 'Sending...' : 'Send Message'}
            </Text>
          </TouchableOpacity>

          {!isStarted && (
            <Text style={styles.warningText}>
              Start the protocol to send messages
            </Text>
          )}
        </View>

        <View style={styles.messagesContainer}>
          <Text style={styles.messagesTitle}>Messages</Text>
          <MessageList events={events} currentUserId={currentUserId} />
        </View>
      </ScrollView>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  scrollView: {
    flex: 1,
  },
  scrollContent: {
    flexGrow: 1,
  },
  formContainer: {
    padding: 16,
    backgroundColor: '#fff',
    borderBottomWidth: 1,
    borderBottomColor: '#e0e0e0',
  },
  sectionTitle: {
    fontSize: 20,
    fontWeight: 'bold',
    color: '#333',
    marginBottom: 16,
  },
  infoBox: {
    backgroundColor: '#e3f2fd',
    padding: 16,
    borderRadius: 8,
    marginBottom: 16,
    borderLeftWidth: 4,
    borderLeftColor: '#2196f3',
  },
  infoTitle: {
    fontSize: 16,
    fontWeight: 'bold',
    color: '#1976d2',
    marginBottom: 8,
  },
  infoText: {
    fontSize: 14,
    color: '#424242',
    lineHeight: 20,
  },
  peersBox: {
    backgroundColor: '#f0f4f8',
    padding: 12,
    borderRadius: 8,
    marginBottom: 16,
  },
  peersTitle: {
    fontSize: 14,
    fontWeight: '600',
    color: '#333',
    marginBottom: 8,
  },
  peerChip: {
    backgroundColor: '#2196f3',
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 16,
    marginRight: 8,
    maxWidth: 150,
  },
  peerChipText: {
    color: '#fff',
    fontSize: 12,
    fontWeight: '600',
  },
  label: {
    fontSize: 14,
    fontWeight: '600',
    color: '#666',
    marginBottom: 8,
  },
  input: {
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    padding: 12,
    fontSize: 14,
    color: '#333',
    backgroundColor: '#fff',
    marginBottom: 16,
  },
  messageInput: {
    height: 80,
    textAlignVertical: 'top',
  },
  priorityContainer: {
    flexDirection: 'row',
    marginBottom: 16,
    gap: 8,
  },
  priorityButton: {
    flex: 1,
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 8,
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
  sendButton: {
    backgroundColor: '#2196f3',
    paddingVertical: 14,
    borderRadius: 8,
    alignItems: 'center',
  },
  sendButtonDisabled: {
    backgroundColor: '#ccc',
  },
  sendButtonText: {
    color: '#fff',
    fontSize: 16,
    fontWeight: '600',
  },
  warningText: {
    marginTop: 8,
    fontSize: 12,
    color: '#ff9800',
    textAlign: 'center',
  },
  messagesContainer: {
    flex: 1,
    padding: 16,
    backgroundColor: '#f5f5f5',
    minHeight: 200,
  },
  messagesTitle: {
    fontSize: 16,
    fontWeight: '600',
    color: '#333',
    marginBottom: 12,
  },
});

