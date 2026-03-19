import React from 'react';
import {View, Text, TouchableOpacity, StyleSheet} from 'react-native';
import {formatMessageTime, formatUserId} from '../utils';
import type {ChatMessage} from '../types';

interface MessageBubbleProps {
  message: ChatMessage;
  showSender?: boolean;
  senderName?: string;
  onLongPress?: () => void;
}

const STATUS_ICONS: Record<string, string> = {
  sending: '⏳',
  sent: '✓',
  delivered: '✓✓',
  read: '✓✓',
  failed: '✗',
};

export function MessageBubble({message, showSender, senderName, onLongPress}: MessageBubbleProps) {
  const isOutgoing = message.isOutgoing;
  const fwd = message.forwardInfo;

  const Wrapper = onLongPress ? TouchableOpacity : View;
  const wrapperProps = onLongPress ? {onLongPress, activeOpacity: 0.7} : {};

  return (
    <View style={[styles.container, isOutgoing ? styles.outgoing : styles.incoming]}>
      {showSender && !isOutgoing && senderName && (
        <Text style={styles.senderName}>{senderName}</Text>
      )}
      <Wrapper {...wrapperProps} style={[styles.bubble, isOutgoing ? styles.bubbleOutgoing : styles.bubbleIncoming]}>
        {fwd && (
          <Text style={[styles.forwardLabel, isOutgoing ? styles.forwardLabelOutgoing : styles.forwardLabelIncoming]}>
            Forwarded from {formatUserId(fwd.originalSender)}
          </Text>
        )}
        <Text style={[styles.text, isOutgoing ? styles.textOutgoing : styles.textIncoming]}>
          {message.content}
        </Text>
        <View style={styles.meta}>
          <Text style={[styles.time, isOutgoing ? styles.timeOutgoing : styles.timeIncoming]}>
            {formatMessageTime(message.timestamp)}
          </Text>
          {isOutgoing && (
            <>
              <Text style={[styles.status, message.status === 'failed' && styles.statusFailed, message.status === 'read' && styles.statusRead]}>
                {STATUS_ICONS[message.status] || ''}
              </Text>
              <Text style={styles.lock}>🔒</Text>
            </>
          )}
        </View>
      </Wrapper>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    marginVertical: 2,
    paddingHorizontal: 12,
  },
  outgoing: {
    alignItems: 'flex-end',
  },
  incoming: {
    alignItems: 'flex-start',
  },
  bubble: {
    maxWidth: '78%',
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 16,
  },
  bubbleOutgoing: {
    backgroundColor: '#007AFF',
    borderBottomRightRadius: 4,
  },
  bubbleIncoming: {
    backgroundColor: '#E9E9EB',
    borderBottomLeftRadius: 4,
  },
  senderName: {
    fontSize: 12,
    color: '#007AFF',
    fontWeight: '600',
    marginBottom: 2,
    marginLeft: 12,
  },
  forwardLabel: {
    fontSize: 11,
    fontStyle: 'italic',
    marginBottom: 4,
  },
  forwardLabelOutgoing: {
    color: 'rgba(255,255,255,0.7)',
  },
  forwardLabelIncoming: {
    color: '#8E8E93',
  },
  text: {
    fontSize: 16,
    lineHeight: 20,
  },
  textOutgoing: {
    color: '#FFFFFF',
  },
  textIncoming: {
    color: '#000000',
  },
  meta: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    marginTop: 2,
    gap: 3,
  },
  time: {
    fontSize: 11,
  },
  timeOutgoing: {
    color: 'rgba(255,255,255,0.7)',
  },
  timeIncoming: {
    color: '#8E8E93',
  },
  status: {
    fontSize: 11,
    color: 'rgba(255,255,255,0.7)',
  },
  statusFailed: {
    color: '#FF3B30',
  },
  statusRead: {
    color: '#34C759',
  },
  lock: {
    fontSize: 9,
  },
});
