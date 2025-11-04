import React from 'react';
import { View, Text, StyleSheet, ScrollView } from 'react-native';
import type {
  ProtocolEvent,
  MessageSentEvent,
  MessageReceivedEvent,
  MessageDeliveredEvent,
  MessageFailedEvent,
} from '@offlineprotocol/react-native';

interface Message {
  id: string;
  type: 'sent' | 'received';
  content: string;
  sender?: string;
  recipient?: string;
  status: 'pending' | 'delivered' | 'failed';
  timestamp: number;
  transport?: string;
  hopCount?: number;
}

interface MessageListProps {
  events: ProtocolEvent[];
  currentUserId: string;
}

export function MessageList({ events, currentUserId }: MessageListProps) {
  // Process events into messages
  const messages: Message[] = React.useMemo(() => {
    const messageMap = new Map<string, Message>();

    events.forEach((event) => {
      if (event.type === 'message_sent') {
        const e = event as MessageSentEvent;
        messageMap.set(e.message_id, {
          id: e.message_id,
          type: 'sent',
          content: '',
          status: 'pending',
          timestamp: e.timestamp,
        });
      } else if (event.type === 'message_received') {
        const e = event as MessageReceivedEvent;
        messageMap.set(e.message_id, {
          id: e.message_id,
          type: 'received',
          content: e.content,
          sender: e.sender,
          recipient: e.recipient,
          status: 'delivered',
          timestamp: e.timestamp,
          transport: e.transport,
          hopCount: e.hop_count,
        });
      } else if (event.type === 'message_delivered') {
        const e = event as MessageDeliveredEvent;
        const msg = messageMap.get(e.message_id);
        if (msg) {
          msg.status = 'delivered';
          msg.transport = e.transport;
          msg.hopCount = e.hop_count;
        }
      } else if (event.type === 'message_failed') {
        const e = event as MessageFailedEvent;
        const msg = messageMap.get(e.message_id);
        if (msg) {
          msg.status = 'failed';
        }
      }
    });

    return Array.from(messageMap.values()).sort((a, b) => b.timestamp - a.timestamp);
  }, [events]);

  const formatTimestamp = (timestamp: number): string => {
    const date = new Date(timestamp);
    return date.toLocaleTimeString();
  };

  const getStatusColor = (status: Message['status']): string => {
    switch (status) {
      case 'delivered':
        return '#4caf50';
      case 'pending':
        return '#ff9800';
      case 'failed':
        return '#f44336';
    }
  };

  return (
    <View style={styles.container}>
      <Text style={styles.title}>Messages ({messages.length})</Text>
      <ScrollView style={styles.messageList}>
        {messages.length === 0 ? (
          <Text style={styles.emptyText}>No messages yet</Text>
        ) : (
          messages.map((message) => (
            <View
              key={message.id}
              style={[
                styles.messageItem,
                message.type === 'sent' ? styles.sentMessage : styles.receivedMessage,
              ]}
            >
              <View style={styles.messageHeader}>
                <Text style={styles.messageType}>
                  {message.type === 'sent' ? '→ Sent' : '← Received'}
                </Text>
                <View style={[styles.statusBadge, { backgroundColor: getStatusColor(message.status) }]}>
                  <Text style={styles.statusText}>{message.status}</Text>
                </View>
              </View>
              {message.content && <Text style={styles.messageContent}>{message.content}</Text>}
              {message.sender && (
                <Text style={styles.messageInfo}>From: {message.sender}</Text>
              )}
              {message.recipient && (
                <Text style={styles.messageInfo}>To: {message.recipient}</Text>
              )}
              {message.transport && (
                <Text style={styles.messageInfo}>
                  Via: {message.transport} ({message.hopCount} hops)
                </Text>
              )}
              <Text style={styles.messageTimestamp}>{formatTimestamp(message.timestamp)}</Text>
            </View>
          ))
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  title: {
    fontSize: 18,
    fontWeight: 'bold',
    color: '#333',
    marginBottom: 12,
  },
  messageList: {
    flex: 1,
  },
  emptyText: {
    textAlign: 'center',
    color: '#999',
    fontSize: 14,
    marginTop: 24,
  },
  messageItem: {
    padding: 12,
    borderRadius: 8,
    marginBottom: 8,
  },
  sentMessage: {
    backgroundColor: '#e3f2fd',
    alignSelf: 'flex-end',
    maxWidth: '80%',
  },
  receivedMessage: {
    backgroundColor: '#f1f8e9',
    alignSelf: 'flex-start',
    maxWidth: '80%',
  },
  messageHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 8,
  },
  messageType: {
    fontSize: 12,
    fontWeight: '600',
    color: '#666',
  },
  statusBadge: {
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderRadius: 4,
  },
  statusText: {
    fontSize: 10,
    color: '#fff',
    fontWeight: '600',
  },
  messageContent: {
    fontSize: 14,
    color: '#333',
    marginBottom: 8,
  },
  messageInfo: {
    fontSize: 12,
    color: '#666',
    marginBottom: 4,
  },
  messageTimestamp: {
    fontSize: 11,
    color: '#999',
    marginTop: 4,
  },
});

