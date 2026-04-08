import React, {useState, useEffect, useRef, useCallback} from 'react';
import {
  SafeAreaView,
  ScrollView,
  View,
  Text,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  Alert,
} from 'react-native';
import {OfflineProtocol} from '@nicegoodthings/react-native-offline-protocol';
import type {ProtocolEvent} from '@nicegoodthings/react-native-offline-protocol';

const DEFAULT_RELAYS = [
  'wss://relay.damus.io',
  'wss://nos.lol',
  'wss://relay.nostr.band',
];

const PROTOCOL_CONFIG = {
  appId: 'nostr-example',
  transports: {
    nostr: {
      enabled: true,
      relayUrls: DEFAULT_RELAYS,
      autoReconnect: true,
      maxReconnectAttempts: 0,
    },
  },
  encryption: {
    enabled: false, // Disabled for simple transport testing
  },
  network: {initialTtl: 8},
};

interface LogEntry {
  id: string;
  timestamp: Date;
  level: 'info' | 'warning' | 'error' | 'debug';
  message: string;
}

interface ChatMessage {
  id: string;
  sender: string;
  content: string;
  timestamp: Date;
  direction: 'sent' | 'received';
}

export default function App() {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [isStarted, setIsStarted] = useState(false);
  const [peerId, setPeerId] = useState('');
  const [messageText, setMessageText] = useState('');
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [showLogs, setShowLogs] = useState(false);
  const [userId] = useState(() => `user-${Math.random().toString(36).slice(2, 8)}`);
  const scrollRef = useRef<ScrollView>(null);

  const addLog = useCallback((level: LogEntry['level'], message: string) => {
    setLogs(prev => [
      ...prev.slice(-99), // Keep last 100 logs
      {
        id: Date.now().toString() + Math.random(),
        timestamp: new Date(),
        level,
        message,
      },
    ]);
  }, []);

  // Initialize protocol
  useEffect(() => {
    const proto = new OfflineProtocol({
      ...PROTOCOL_CONFIG,
      userId,
    });
    setProtocol(proto);

    return () => {
      proto.stop().catch(() => {});
    };
  }, [userId]);

  // Register event listener
  useEffect(() => {
    if (!protocol) return;

    const unsubscribe = protocol.on('all', (event: ProtocolEvent) => {
      switch (event.type) {
        case 'message_received':
          setMessages(prev => [
            ...prev,
            {
              id: event.data.message_id || Date.now().toString(),
              sender: event.data.sender_id || 'unknown',
              content: event.data.content || '',
              timestamp: new Date(),
              direction: 'received',
            },
          ]);
          addLog('info', `Message from ${event.data.sender_id}: ${event.data.content}`);
          break;

        case 'message_sent':
          addLog('info', `Message sent: ${event.data.message_id}`);
          break;

        case 'message_failed':
          addLog('error', `Message failed: ${event.data.message_id} - ${event.data.reason}`);
          break;

        case 'transport_switched':
          addLog('info', `Transport switched to: ${event.data.transport}`);
          break;

        case 'neighbor_discovered':
          addLog('info', `Peer discovered: ${event.data.peer_id}`);
          break;

        case 'neighbor_lost':
          addLog('info', `Peer lost: ${event.data.peer_id}`);
          break;

        default:
          addLog('debug', `Event: ${event.type}`);
          break;
      }
    });

    return () => {
      unsubscribe();
    };
  }, [protocol, addLog]);

  const handleStart = async () => {
    if (!protocol) return;
    try {
      await protocol.start();
      setIsStarted(true);
      setIsConnected(true);
      addLog('info', 'Protocol started with Nostr transport');
      addLog('info', `User ID: ${userId}`);
      addLog('info', `Relays: ${DEFAULT_RELAYS.join(', ')}`);
    } catch (error: any) {
      addLog('error', `Failed to start: ${error.message}`);
      Alert.alert('Error', `Failed to start protocol: ${error.message}`);
    }
  };

  const handleStop = async () => {
    if (!protocol) return;
    try {
      await protocol.stop();
      setIsStarted(false);
      setIsConnected(false);
      addLog('info', 'Protocol stopped');
    } catch (error: any) {
      addLog('error', `Failed to stop: ${error.message}`);
    }
  };

  const handleSend = async () => {
    if (!protocol || !peerId.trim() || !messageText.trim()) {
      Alert.alert('Error', 'Enter both peer ID and message');
      return;
    }
    try {
      await protocol.sendMessage({
        recipient: peerId.trim(),
        content: messageText.trim(),
      });
      setMessages(prev => [
        ...prev,
        {
          id: Date.now().toString(),
          sender: userId,
          content: messageText.trim(),
          timestamp: new Date(),
          direction: 'sent',
        },
      ]);
      addLog('info', `Sent to ${peerId}: ${messageText}`);
      setMessageText('');
    } catch (error: any) {
      addLog('error', `Send failed: ${error.message}`);
      Alert.alert('Send Error', error.message);
    }
  };

  const handleToggleTransport = async () => {
    if (!protocol) return;
    try {
      if (isConnected) {
        await protocol.disableTransport('nostr');
        setIsConnected(false);
        addLog('info', 'Nostr transport disabled');
      } else {
        await protocol.enableTransport('nostr', {
          enabled: true,
          relayUrls: DEFAULT_RELAYS,
          autoReconnect: true,
        });
        setIsConnected(true);
        addLog('info', 'Nostr transport enabled');
      }
    } catch (error: any) {
      addLog('error', `Toggle transport failed: ${error.message}`);
    }
  };

  return (
    <SafeAreaView style={styles.container}>
      {/* Header */}
      <View style={styles.header}>
        <Text style={styles.title}>Nostr Transport Example</Text>
        <Text style={styles.subtitle}>User: {userId}</Text>
        <View style={styles.statusRow}>
          <View style={[styles.statusDot, isConnected ? styles.connected : styles.disconnected]} />
          <Text style={styles.statusText}>
            {isStarted ? (isConnected ? 'Connected' : 'Disconnected') : 'Stopped'}
          </Text>
        </View>
      </View>

      {/* Controls */}
      <View style={styles.controls}>
        <TouchableOpacity
          style={[styles.button, isStarted ? styles.stopButton : styles.startButton]}
          onPress={isStarted ? handleStop : handleStart}>
          <Text style={styles.buttonText}>{isStarted ? 'Stop' : 'Start'}</Text>
        </TouchableOpacity>

        {isStarted && (
          <TouchableOpacity
            style={[styles.button, isConnected ? styles.disableButton : styles.enableButton]}
            onPress={handleToggleTransport}>
            <Text style={styles.buttonText}>
              {isConnected ? 'Disable Nostr' : 'Enable Nostr'}
            </Text>
          </TouchableOpacity>
        )}

        <TouchableOpacity
          style={[styles.button, styles.logButton]}
          onPress={() => setShowLogs(!showLogs)}>
          <Text style={styles.buttonText}>{showLogs ? 'Chat' : 'Logs'}</Text>
        </TouchableOpacity>
      </View>

      {/* Peer Input */}
      {isStarted && !showLogs && (
        <View style={styles.inputSection}>
          <TextInput
            style={styles.input}
            placeholder="Peer's User ID"
            value={peerId}
            onChangeText={setPeerId}
            autoCapitalize="none"
            autoCorrect={false}
          />
        </View>
      )}

      {/* Messages / Logs */}
      <ScrollView
        ref={scrollRef}
        style={styles.messageList}
        onContentSizeChange={() => scrollRef.current?.scrollToEnd()}>
        {showLogs ? (
          logs.map(log => (
            <View key={log.id} style={styles.logEntry}>
              <Text style={[styles.logLevel, styles[`log_${log.level}`]]}>
                [{log.level.toUpperCase()}]
              </Text>
              <Text style={styles.logMessage}>{log.message}</Text>
            </View>
          ))
        ) : (
          messages.map(msg => (
            <View
              key={msg.id}
              style={[
                styles.messageBubble,
                msg.direction === 'sent' ? styles.sentBubble : styles.receivedBubble,
              ]}>
              <Text style={styles.messageSender}>
                {msg.direction === 'sent' ? 'You' : msg.sender.slice(0, 12)}
              </Text>
              <Text style={styles.messageContent}>{msg.content}</Text>
              <Text style={styles.messageTime}>
                {msg.timestamp.toLocaleTimeString()}
              </Text>
            </View>
          ))
        )}
        {!showLogs && messages.length === 0 && (
          <Text style={styles.emptyText}>
            No messages yet. Share your User ID with a peer and start chatting!
          </Text>
        )}
      </ScrollView>

      {/* Message Input */}
      {isStarted && !showLogs && (
        <View style={styles.messageInputRow}>
          <TextInput
            style={styles.messageInput}
            placeholder="Type a message..."
            value={messageText}
            onChangeText={setMessageText}
            onSubmitEditing={handleSend}
            returnKeyType="send"
          />
          <TouchableOpacity style={styles.sendButton} onPress={handleSend}>
            <Text style={styles.sendButtonText}>Send</Text>
          </TouchableOpacity>
        </View>
      )}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  header: {
    padding: 16,
    backgroundColor: '#7B1FA2',
    alignItems: 'center',
  },
  title: {
    fontSize: 20,
    fontWeight: 'bold',
    color: '#fff',
  },
  subtitle: {
    fontSize: 12,
    color: '#E1BEE7',
    marginTop: 4,
    fontFamily: 'monospace',
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    marginTop: 8,
  },
  statusDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    marginRight: 6,
  },
  connected: {
    backgroundColor: '#4CAF50',
  },
  disconnected: {
    backgroundColor: '#F44336',
  },
  statusText: {
    color: '#E1BEE7',
    fontSize: 13,
  },
  controls: {
    flexDirection: 'row',
    padding: 12,
    gap: 8,
  },
  button: {
    flex: 1,
    paddingVertical: 10,
    borderRadius: 8,
    alignItems: 'center',
  },
  startButton: {
    backgroundColor: '#4CAF50',
  },
  stopButton: {
    backgroundColor: '#F44336',
  },
  enableButton: {
    backgroundColor: '#2196F3',
  },
  disableButton: {
    backgroundColor: '#FF9800',
  },
  logButton: {
    backgroundColor: '#607D8B',
  },
  buttonText: {
    color: '#fff',
    fontWeight: '600',
    fontSize: 14,
  },
  inputSection: {
    paddingHorizontal: 12,
    paddingBottom: 8,
  },
  input: {
    backgroundColor: '#fff',
    borderRadius: 8,
    padding: 12,
    fontSize: 14,
    borderWidth: 1,
    borderColor: '#ddd',
  },
  messageList: {
    flex: 1,
    paddingHorizontal: 12,
  },
  messageBubble: {
    maxWidth: '80%',
    padding: 10,
    borderRadius: 12,
    marginVertical: 4,
  },
  sentBubble: {
    backgroundColor: '#7B1FA2',
    alignSelf: 'flex-end',
  },
  receivedBubble: {
    backgroundColor: '#fff',
    alignSelf: 'flex-start',
    borderWidth: 1,
    borderColor: '#e0e0e0',
  },
  messageSender: {
    fontSize: 11,
    fontWeight: '600',
    color: '#E1BEE7',
    marginBottom: 2,
  },
  messageContent: {
    fontSize: 15,
    color: '#fff',
  },
  messageTime: {
    fontSize: 10,
    color: '#CE93D8',
    marginTop: 4,
    textAlign: 'right',
  },
  messageInputRow: {
    flexDirection: 'row',
    padding: 12,
    gap: 8,
  },
  messageInput: {
    flex: 1,
    backgroundColor: '#fff',
    borderRadius: 20,
    paddingHorizontal: 16,
    paddingVertical: 10,
    fontSize: 15,
    borderWidth: 1,
    borderColor: '#ddd',
  },
  sendButton: {
    backgroundColor: '#7B1FA2',
    borderRadius: 20,
    paddingHorizontal: 20,
    justifyContent: 'center',
  },
  sendButtonText: {
    color: '#fff',
    fontWeight: '600',
  },
  emptyText: {
    textAlign: 'center',
    color: '#999',
    marginTop: 40,
    paddingHorizontal: 20,
    lineHeight: 22,
  },
  logEntry: {
    flexDirection: 'row',
    paddingVertical: 3,
    gap: 6,
  },
  logLevel: {
    fontSize: 11,
    fontFamily: 'monospace',
    fontWeight: '600',
  },
  logMessage: {
    fontSize: 12,
    fontFamily: 'monospace',
    flex: 1,
    color: '#333',
  },
  log_info: {color: '#2196F3'},
  log_warning: {color: '#FF9800'},
  log_error: {color: '#F44336'},
  log_debug: {color: '#9E9E9E'},
});
