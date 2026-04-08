import React from 'react';
import {View, Text, StyleSheet} from 'react-native';
import {formatMessageTime} from '../utils';
import type {ChatMessage} from '../types';

interface MessageBubbleProps {
  message: ChatMessage;
  isOwnMessage: boolean;
}

const STATUS_LABELS: Record<ChatMessage['status'], string> = {
  sending: 'Sending...',
  sent: 'Sent',
  delivered: 'Delivered',
  failed: 'Failed',
};

export function MessageBubble({message, isOwnMessage}: MessageBubbleProps) {
  return (
    <View
      style={[
        styles.bubble,
        isOwnMessage ? styles.ownBubble : styles.otherBubble,
      ]}>
      {!isOwnMessage && (
        <Text style={styles.senderName}>
          {message.senderId.length > 12
            ? message.senderId.slice(0, 12) + '...'
            : message.senderId}
        </Text>
      )}
      <Text style={[styles.content, isOwnMessage ? styles.ownContent : styles.otherContent]}>
        {message.content}
      </Text>
      <View style={styles.metaRow}>
        <Text style={[styles.time, isOwnMessage ? styles.ownTime : styles.otherTime]}>
          {formatMessageTime(message.timestamp)}
        </Text>
        {isOwnMessage && (
          <Text
            style={[
              styles.status,
              message.status === 'failed' && styles.statusFailed,
            ]}>
            {STATUS_LABELS[message.status]}
          </Text>
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  bubble: {
    maxWidth: '80%',
    padding: 10,
    borderRadius: 16,
    marginVertical: 3,
  },
  ownBubble: {
    backgroundColor: '#7B1FA2',
    alignSelf: 'flex-end',
    borderBottomRightRadius: 4,
  },
  otherBubble: {
    backgroundColor: '#FFFFFF',
    alignSelf: 'flex-start',
    borderBottomLeftRadius: 4,
    borderWidth: 1,
    borderColor: '#E5E5E5',
  },
  senderName: {
    fontSize: 11,
    fontWeight: '600',
    color: '#7B1FA2',
    marginBottom: 2,
  },
  content: {
    fontSize: 15,
    lineHeight: 20,
  },
  ownContent: {
    color: '#FFFFFF',
  },
  otherContent: {
    color: '#1C1C1E',
  },
  metaRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    alignItems: 'center',
    marginTop: 4,
    gap: 6,
  },
  time: {
    fontSize: 10,
  },
  ownTime: {
    color: '#CE93D8',
  },
  otherTime: {
    color: '#8E8E93',
  },
  status: {
    fontSize: 10,
    color: '#CE93D8',
  },
  statusFailed: {
    color: '#FF6B6B',
  },
});
