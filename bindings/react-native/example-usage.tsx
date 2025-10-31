/**
 * Example usage of @offlineprotocol/react-native in a React Native app
 * 
 * This demonstrates all major features of the SDK.
 */

import React, { useEffect, useState } from 'react';
import { View, Text, Button, TextInput, StyleSheet, FlatList } from 'react-native';
import { OfflineProtocol, MessagePriority } from '@offlineprotocol/react-native';

// Initialize protocol (typically done once at app startup)
const protocol = new OfflineProtocol({
  appId: 'example-chat-app',
  userId: 'user123', // Replace with actual user ID from auth
  transport: {
    bleEnabled: true,
    wifiDirectEnabled: true,
    internetEnabled: true,
  },
  dors: {
    preferOnline: false, // Offline-first mode
  },
  relay: {
    allowRelay: true,
    minBatteryForRelay: 30,
  },
});

export default function App() {
  const [isStarted, setIsStarted] = useState(false);
  const [messages, setMessages] = useState<any[]>([]);
  const [recipient, setRecipient] = useState('');
  const [messageText, setMessageText] = useState('');
  const [currentTransport, setCurrentTransport] = useState<string>('Unknown');
  const [isRelay, setIsRelay] = useState(false);

  useEffect(() => {
    // Setup event listeners
    protocol.on('message:received', (event) => {
      console.log('Received message:', event);
      setMessages(prev => [...prev, {
        id: event.messageId,
        from: event.sender,
        content: event.content,
        hopCount: event.hopCount,
        transport: event.transport,
      }]);
    });

    protocol.on('message:delivered', (event) => {
      console.log('Message delivered:', event.messageId);
      console.log(`Latency: ${event.latencyMs}ms, Hops: ${event.hopCount}`);
    });

    protocol.on('transport:switched', (event) => {
      console.log(`Transport switched: ${event.from} → ${event.to}`);
      setCurrentTransport(event.to);
    });

    protocol.on('relay:promoted', (event) => {
      console.log('Promoted to relay:', event);
      setIsRelay(true);
    });

    protocol.on('relay:demoted', (event) => {
      console.log('Demoted from relay:', event);
      setIsRelay(false);
    });

    // Start protocol on mount
    protocol.start()
      .then(() => {
        console.log('Protocol started successfully');
        setIsStarted(true);
      })
      .catch(err => console.error('Failed to start protocol:', err));

    // Cleanup on unmount
    return () => {
      protocol.stop().catch(console.error);
    };
  }, []);

  const handleSendMessage = async () => {
    if (!recipient || !messageText) {
      alert('Please enter recipient and message');
      return;
    }

    try {
      const messageId = await protocol.sendMessage({
        recipient,
        content: messageText,
        priority: MessagePriority.Medium,
      });

      console.log('Message sent:', messageId);
      setMessageText('');
    } catch (error) {
      console.error('Failed to send message:', error);
      alert('Failed to send message');
    }
  };

  return (
    <View style={styles.container}>
      <View style={styles.status}>
        <Text style={styles.statusText}>
          Status: {isStarted ? '🟢 Online' : '🔴 Offline'}
        </Text>
        <Text style={styles.statusText}>
          Transport: {currentTransport}
        </Text>
        <Text style={styles.statusText}>
          Relay: {isRelay ? 'Yes' : 'No'}
        </Text>
      </View>

      <View style={styles.sendBox}>
        <TextInput
          style={styles.input}
          placeholder="Recipient"
          value={recipient}
          onChangeText={setRecipient}
        />
        <TextInput
          style={styles.input}
          placeholder="Message"
          value={messageText}
          onChangeText={setMessageText}
          multiline
        />
        <Button
          title="Send Message"
          onPress={handleSendMessage}
          disabled={!isStarted}
        />
      </View>

      <FlatList
        data={messages}
        keyExtractor={item => item.id}
        renderItem={({ item }) => (
          <View style={styles.message}>
            <Text style={styles.messageSender}>From: {item.from}</Text>
            <Text style={styles.messageContent}>{item.content}</Text>
            <Text style={styles.messageInfo}>
              {item.transport} • {item.hopCount} hops
            </Text>
          </View>
        )}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    padding: 16,
  },
  status: {
    padding: 12,
    backgroundColor: '#f0f0f0',
    borderRadius: 8,
    marginBottom: 16,
  },
  statusText: {
    fontSize: 14,
    marginBottom: 4,
  },
  sendBox: {
    marginBottom: 16,
  },
  input: {
    borderWidth: 1,
    borderColor: '#ccc',
    borderRadius: 8,
    padding: 12,
    marginBottom: 8,
  },
  message: {
    padding: 12,
    backgroundColor: '#e3f2fd',
    borderRadius: 8,
    marginBottom: 8,
  },
  messageSender: {
    fontWeight: 'bold',
    marginBottom: 4,
  },
  messageContent: {
    fontSize: 16,
    marginBottom: 4,
  },
  messageInfo: {
    fontSize: 12,
    color: '#666',
  },
});

