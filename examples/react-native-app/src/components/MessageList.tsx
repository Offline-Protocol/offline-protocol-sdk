import React from 'react';
import { View, Text, StyleSheet, FlatList, type ListRenderItem } from 'react-native';
import type { StyleProp, ViewStyle } from 'react-native';
import type {
  ProtocolEvent,
  MessageSentEvent,
  MessageReceivedEvent,
  MessageDeliveredEvent,
  MessageFailedEvent,
} from '@offline-protocol/mesh-sdk';

interface Message {
  id: string;
  type: 'sent' | 'received';
  content: string;
  sender?: string;
  recipient?: string;
  status: 'pending' | 'delivered' | 'failed';
  timestamp: number;
  lamportClock: number;
  transport?: string;
  hopCount?: number;
  priority?: string;
  requiresAck?: boolean;
}

type MessageListVariant = 'card' | 'full' | 'embedded' | 'mobile';

interface MessageListProps {
  events: ProtocolEvent[];
  currentUserId: string;
  isCompact?: boolean;
  contentInsetBottom?: number;
  listHeaderComponent?: React.ReactElement | React.ComponentType<any> | null;
  listFooterComponent?: React.ReactElement | React.ComponentType<any> | null;
  containerStyle?: StyleProp<ViewStyle>;
  contentContainerStyle?: StyleProp<ViewStyle>;
  variant?: MessageListVariant;
}

export function MessageList({
  events,
  currentUserId,
  isCompact = false,
  contentInsetBottom,
  listHeaderComponent,
  listFooterComponent,
  containerStyle,
  contentContainerStyle,
  variant: variantProp,
}: MessageListProps) {
  const listRef = React.useRef<FlatList<Message>>(null);
  const variant = variantProp ?? 'card';

  const messages: Message[] = React.useMemo(() => {
    const messageMap = new Map<string, Message>();

    [...events].reverse().forEach((event) => {
      if (event.type === 'message_sent') {
        const e = event as MessageSentEvent;
        messageMap.set(e.message_id, {
          id: e.message_id,
          type: 'sent',
          content: e.content,
          sender: e.sender,
          recipient: e.recipient,
          status: 'delivered',
          timestamp: e.timestamp,
          lamportClock: (e as any).lamport_clock ?? 0,
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
          lamportClock: (e as any).lamport_clock ?? 0,
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
        } else {
          messageMap.set(e.message_id, {
            id: e.message_id,
            type: 'sent',
            content: '',
            status: 'delivered',
            timestamp: Date.now(),
            lamportClock: 0,
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
            lamportClock: 0,
          });
        }
      }
    });

    // Sort by Lamport clock for causal ordering; fall back to wall-clock
    // for legacy messages (lamportClock === 0), tiebreak by sender ID.
    return Array.from(messageMap.values()).sort((a, b) => {
      if (a.lamportClock === 0 && b.lamportClock === 0) {
        return a.timestamp - b.timestamp;
      }
      const clockDiff = a.lamportClock - b.lamportClock;
      if (clockDiff !== 0) return clockDiff;
      return (a.sender ?? '').localeCompare(b.sender ?? '');
    });
  }, [events]);

  React.useEffect(() => {
    if (messages.length > 0) {
      requestAnimationFrame(() => {
        listRef.current?.scrollToEnd({ animated: true });
      });
    }
  }, [messages.length]);

  const bottomPadding = contentInsetBottom ?? (isCompact ? 80 : 160);

  const formatTimestamp = (timestamp: number): string => {
    const date = new Date(timestamp);
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  };

  const getStatusColor = (status: Message['status']): string => {
    switch (status) {
      case 'delivered':
        return '#16a34a';
      case 'failed':
        return '#dc2626';
      default:
        return '#f59e0b';
    }
  };

  const getStatusLabel = (status: Message['status']): string => {
    switch (status) {
      case 'delivered':
        return 'Delivered';
      case 'failed':
        return 'Failed';
      default:
        return 'Pending';
    }
  };

  const getStatusGlyph = (status: Message['status']): string => {
    switch (status) {
      case 'delivered':
        return '✓';
      case 'failed':
        return '⚠';
      default:
        return '…';
    }
  };

  const keyExtractor = React.useCallback((item: Message) => item.id, []);

  const renderItem: ListRenderItem<Message> = React.useCallback(
    ({ item }) => {
      const isSent = item.type === 'sent';
      const statusColor = getStatusColor(item.status);
      const statusLabel = getStatusLabel(item.status);
      const statusGlyph = getStatusGlyph(item.status);
      const actorLabel = isSent || item.sender === currentUserId ? 'You' : item.sender ?? 'Peer';
      const priorityStyle = (() => {
        switch (item.priority) {
          case 'low':
            return styles.priority_low;
          case 'medium':
            return styles.priority_medium;
          case 'high':
            return styles.priority_high;
          case 'critical':
            return styles.priority_critical;
          default:
            return styles.priority_default;
        }
      })();

      return (
        <View
          style={[
            styles.messageRow,
            isSent ? styles.messageRowSent : styles.messageRowReceived,
          ]}
        >
          <View
            style={[
              styles.messageBubble,
              isSent ? styles.messageBubbleSent : styles.messageBubbleReceived,
              isCompact && styles.messageBubbleCompact,
            ]}
          >
            <View style={styles.messageHeader}>
              <Text
                style={[
                  styles.actor,
                  isSent ? styles.actorSent : styles.actorReceived,
                ]}
                numberOfLines={1}
              >
                {actorLabel}
              </Text>
              <View style={[styles.statusBadge, { backgroundColor: statusColor }]}>
                <Text style={styles.statusText}>{statusLabel}</Text>
              </View>
            </View>

            {item.content ? (
              <Text
                style={[
                  styles.messageBody,
                  isCompact && styles.messageBodyCompact,
                  isSent && styles.messageBodySent,
                ]}
              >
                {item.content}
              </Text>
            ) : null}

            <View style={styles.footerRow}>
              {item.priority ? (
                <View
                  style={[
                    styles.priorityBadge,
                    priorityStyle ?? styles.priority_default,
                  ]}
                >
                  <Text style={styles.priorityText}>{item.priority.toUpperCase()}</Text>
                </View>
              ) : null}

              <View style={styles.footerSpacer} />

              {item.transport ? (
                <Text
                  style={[
                    styles.metaText,
                    isSent && styles.metaTextSent,
                  ]}
                >
                  {item.transport}
                </Text>
              ) : null}

              {typeof item.hopCount === 'number' ? (
                <Text
                  style={[
                    styles.metaText,
                    isSent && styles.metaTextSent,
                  ]}
                >
                  {`${item.hopCount} hop${item.hopCount === 1 ? '' : 's'}`}
                </Text>
              ) : null}

              <Text
                style={[styles.metaText, isSent && styles.metaTextSent]}
              >
                {formatTimestamp(item.timestamp)}
              </Text>
            </View>
          </View>

          <Text
            style={[
              styles.statusGlyph,
              item.status === 'delivered' && styles.statusGlyphDelivered,
              item.status === 'failed' && styles.statusGlyphFailed,
            ]}
          >
            {statusGlyph}
          </Text>
        </View>
      );
    },
    [currentUserId, isCompact]
  );

  return (
    <View
      style={[
        styles.container,
        isCompact && styles.containerCompact,
        variant === 'full' && styles.containerFull,
        variant === 'embedded' && styles.containerEmbedded,
        variant === 'mobile' && styles.containerMobile,
        containerStyle,
      ]}
    >
      {variant !== 'embedded' && variant !== 'mobile' && (
        <View
          style={[
            styles.headingRow,
            variant === 'full' && styles.headingRowFull,
          ]}
        >
          <Text style={styles.title}>Conversation</Text>
          <View style={styles.counterPill}>
            <Text style={styles.counterText}>{messages.length}</Text>
          </View>
        </View>
      )}
      <FlatList
        ref={listRef}
        data={messages}
        keyExtractor={keyExtractor}
        renderItem={renderItem}
        showsVerticalScrollIndicator={variant !== 'embedded'}
        keyboardShouldPersistTaps="handled"
        nestedScrollEnabled={variant === 'embedded'}
        contentContainerStyle={[
          styles.listContent,
          variant === 'full' && styles.listContentFull,
          variant === 'embedded' && styles.listContentEmbedded,
          variant === 'mobile' && styles.listContentMobile,
          { paddingBottom: bottomPadding },
          messages.length === 0 && styles.listContentEmpty,
          contentContainerStyle,
        ]}
        ListHeaderComponent={listHeaderComponent ?? undefined}
        ListFooterComponent={listFooterComponent ?? undefined}
        ListEmptyComponent={
          <View style={styles.emptyState}>
            <Text style={styles.emptyTitle}>No messages yet</Text>
            <Text style={styles.emptySubtitle}>
              Start the conversation by sending a note to a nearby peer.
            </Text>
          </View>
        }
      />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#ffffff',
    borderRadius: 18,
    paddingHorizontal: 16,
    paddingVertical: 14,
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.08,
    shadowRadius: 12,
    elevation: 2,
  },
  containerCompact: {
    borderRadius: 14,
    paddingHorizontal: 12,
    paddingVertical: 12,
  },
  containerEmbedded: {
    backgroundColor: 'transparent',
    borderRadius: 0,
    padding: 0,
    shadowOpacity: 0,
    elevation: 0,
  },
  containerMobile: {
    backgroundColor: 'transparent',
    borderRadius: 0,
    padding: 0,
    shadowOpacity: 0,
    elevation: 0,
    flex: 1,
  },
  containerFull: {
    backgroundColor: '#eaf1ff',
    borderRadius: 0,
    paddingHorizontal: 0,
    paddingVertical: 0,
    shadowOpacity: 0,
    elevation: 0,
  },
  headingRow: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 8,
  },
  headingRowFull: {
    paddingHorizontal: 4,
  },
  title: {
    flex: 1,
    fontSize: 18,
    fontWeight: '700',
    color: '#1f2937',
  },
  counterPill: {
    minWidth: 32,
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderRadius: 999,
    backgroundColor: '#e2e8f0',
    alignItems: 'center',
  },
  counterText: {
    fontSize: 12,
    fontWeight: '700',
    color: '#475569',
  },
  listContent: {},
  listContentFull: {
    paddingHorizontal: 16,
    backgroundColor: '#ffffff',
    marginHorizontal: 12,
    borderRadius: 20,
    marginVertical: 6,
    paddingVertical: 20,
    minHeight: 400,
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.08,
    shadowRadius: 12,
    elevation: 2,
  },
  listContentEmbedded: {
    paddingHorizontal: 0,
    paddingTop: 0,
  },
  listContentMobile: {
    paddingHorizontal: 16,
    paddingVertical: 16,
    flexGrow: 1,
  },
  listContentEmpty: {
    flexGrow: 1,
    justifyContent: 'center',
  },
  emptyState: {
    alignItems: 'center',
    paddingHorizontal: 16,
  },
  emptyTitle: {
    fontSize: 16,
    fontWeight: '600',
    color: '#475569',
    marginBottom: 6,
  },
  emptySubtitle: {
    fontSize: 13,
    color: '#94a3b8',
    textAlign: 'center',
    lineHeight: 18,
  },
  messageRow: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    width: '100%',
    marginBottom: 12,
  },
  messageRowSent: {
    justifyContent: 'flex-end',
  },
  messageRowReceived: {
    justifyContent: 'flex-start',
  },
  messageBubble: {
    maxWidth: '85%',
    borderRadius: 24,
    borderWidth: 1,
    paddingHorizontal: 18,
    paddingVertical: 14,
    backgroundColor: '#f8fafc',
    borderColor: '#e2e8f0',
    marginVertical: 4,
  },
  messageBubbleCompact: {
    maxWidth: '88%',
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  messageBubbleSent: {
    backgroundColor: '#1d4ed8',
    borderColor: '#1e3a8a',
  },
  messageBubbleReceived: {
    backgroundColor: '#f1f5f9',
  },
  messageHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 8,
  },
  actor: {
    flex: 1,
    fontSize: 12,
    fontWeight: '700',
    color: '#1f2937',
    marginRight: 8,
  },
  actorSent: {
    color: '#e0e7ff',
  },
  actorReceived: {
    color: '#1f2937',
  },
  statusBadge: {
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderRadius: 12,
  },
  statusText: {
    fontSize: 10,
    fontWeight: '700',
    color: '#ffffff',
    textTransform: 'uppercase',
    letterSpacing: 0.4,
  },
  messageBody: {
    fontSize: 16,
    lineHeight: 22,
    color: '#0f172a',
  },
  messageBodyCompact: {
    fontSize: 15,
    lineHeight: 21,
  },
  messageBodySent: {
    color: '#e2e8f0',
  },
  footerRow: {
    flexDirection: 'row',
    alignItems: 'center',
    marginTop: 10,
  },
  footerSpacer: {
    flex: 1,
  },
  priorityBadge: {
    borderRadius: 12,
    paddingHorizontal: 8,
    paddingVertical: 2,
    marginRight: 8,
  },
  priorityText: {
    fontSize: 10,
    fontWeight: '700',
    color: '#0f172a',
    letterSpacing: 0.4,
  },
  priority_low: {
    backgroundColor: '#dbeafe',
  },
  priority_medium: {
    backgroundColor: '#bfdbfe',
  },
  priority_high: {
    backgroundColor: '#fecdd3',
  },
  priority_critical: {
    backgroundColor: '#f87171',
  },
  priority_default: {
    backgroundColor: '#e2e8f0',
  },
  metaText: {
    marginLeft: 8,
    fontSize: 11,
    color: '#475569',
  },
  metaTextSent: {
    color: '#c7d2fe',
  },
  statusGlyph: {
    marginLeft: 8,
    fontSize: 16,
    color: '#94a3b8',
  },
  statusGlyphDelivered: {
    color: '#22c55e',
  },
  statusGlyphFailed: {
    color: '#ef4444',
  },
});

