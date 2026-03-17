import React from 'react';
import {View, Text, StyleSheet} from 'react-native';
import {formatMessageTime} from '../utils';
import type {ChatMessage} from '../types';

interface MessageBubbleProps {
  message: ChatMessage;
  showSender?: boolean;
  senderName?: string;
}

const STATUS_ICONS: Record<string, string> = {
  sending: '⏳',
  sent: '✓',
  delivered: '✓✓',
  failed: '✗',
};

export function MessageBubble({message, showSender, senderName}: MessageBubbleProps) {
  const isOutgoing = message.isOutgoing;

  return (
    <View style={[styles.container, isOutgoing ? styles.outgoing : styles.incoming]}>
      {showSender && !isOutgoing && senderName && (
        <Text style={styles.senderName}>{senderName}</Text>
      )}
      <View style={[styles.bubble, isOutgoing ? styles.bubbleOutgoing : styles.bubbleIncoming]}>
        <Text style={[styles.text, isOutgoing ? styles.textOutgoing : styles.textIncoming]}>
          {message.content}
        </Text>
        <View style={styles.meta}>
          <Text style={[styles.time, isOutgoing ? styles.timeOutgoing : styles.timeIncoming]}>
            {formatMessageTime(message.timestamp)}
          </Text>
          {isOutgoing && (
            <>
              <Text style={[styles.status, message.status === 'failed' && styles.statusFailed]}>
                {STATUS_ICONS[message.status] || ''}
              </Text>
              <Text style={styles.lock}>🔒</Text>
            </>
          )}
        </View>
      </View>
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
  lock: {
    fontSize: 9,
  },
});
