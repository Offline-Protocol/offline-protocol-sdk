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
  priority?: string;
  requiresAck?: boolean;
}

interface MessageListProps {
  events: ProtocolEvent[];
  currentUserId: string;
}

export function MessageList({ events, currentUserId }: MessageListProps) {
  const scrollRef = React.useRef<ScrollView>(null);

  // Process events into messages
  const messages: Message[] = React.useMemo(() => {
    const messageMap = new Map<string, Message>();

    events.forEach((event) => {
      if (event.type === 'message_sent') {
        const e = event as MessageSentEvent;
        messageMap.set(e.message_id, {
          id: e.message_id,
          type: 'sent',
          content: e.content,
          sender: e.sender,
          recipient: e.recipient,
          status: e.requires_ack ? 'pending' : 'delivered',
          timestamp: e.timestamp,
          priority: e.priority,
          requiresAck: e.requires_ack,
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
          priority: undefined,
          requiresAck: undefined,
        });
      } else if (event.type === 'message_delivered') {
        const e = event as MessageDeliveredEvent;
        const msg = messageMap.get(e.message_id);
        if (msg) {
          msg.status = 'delivered';
          msg.transport = e.transport;
          msg.hopCount = e.hop_count;
        } else {
          messageMap.set(e.message_id, {
            id: e.message_id,
            type: 'sent',
            content: '',
            status: 'delivered',
            timestamp: Date.now(),
            transport: e.transport,
            hopCount: e.hop_count,
          });
        }
      } else if (event.type === 'message_failed') {
        const e = event as MessageFailedEvent;
        const msg = messageMap.get(e.message_id);
        if (msg) {
          msg.status = 'failed';
        } else {
          messageMap.set(e.message_id, {
            id: e.message_id,
            type: 'sent',
            content: '',
            status: 'failed',
            timestamp: Date.now(),
          });
        }
      }
    });

    return Array.from(messageMap.values()).sort((a, b) => a.timestamp - b.timestamp);
  }, [events]);

  React.useEffect(() => {
    scrollRef.current?.scrollToEnd({ animated: true });
  }, [messages.length]);

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
      <ScrollView
        ref={scrollRef}
        style={styles.messageList}
        contentContainerStyle={styles.messageListContent}
        showsVerticalScrollIndicator={false}
      >
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
              <View style={styles.metaRow}>
                <Text style={styles.messageTimestamp}>{formatTimestamp(message.timestamp)}</Text>
                {message.type === 'sent' && (
                  <Text
                    style={[styles.statusBadgeText,
                      message.status === 'delivered' && styles.statusDelivered,
                      message.status === 'failed' && styles.statusFailed,
                    ]}
                  >
                    {message.status === 'delivered'
                      ? '✓✓'
                      : message.status === 'failed'
                      ? '⚠️'
                      : '⏳'}
                  </Text>
                )}
              </View>
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
    backgroundColor: '#fff',
    borderRadius: 16,
    paddingVertical: 12,
    paddingHorizontal: 8,
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
  messageListContent: {
    paddingBottom: 16,
  },
  emptyText: {
    textAlign: 'center',
    color: '#999',
    fontSize: 14,
    marginTop: 24,
  },
  messageItem: {
    paddingVertical: 8,
    paddingHorizontal: 12,
    borderRadius: 16,
    marginBottom: 10,
    maxWidth: '75%',
  },
  sentMessage: {
    backgroundColor: '#e3f2fd',
    alignSelf: 'flex-end',
    borderBottomRightRadius: 4,
  },
  receivedMessage: {
    backgroundColor: '#f1f8e9',
    alignSelf: 'flex-start',
    borderBottomLeftRadius: 4,
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
    marginBottom: 6,
  },
  metaRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    alignItems: 'center',
    gap: 6,
  },
  messageTimestamp: {
    fontSize: 10,
    color: '#777',
  },
  statusBadgeText: {
    fontSize: 10,
    color: '#ff9800',
  },
  statusDelivered: {
    color: '#4caf50',
  },
  statusFailed: {
    color: '#f44336',
  },
});

