import React from 'react';
import {View, TouchableOpacity, Text, StyleSheet} from 'react-native';
import {QUICK_MESSAGES} from '../constants';

interface QuickMessagesProps {
  onSend: (text: string, priority: 'medium' | 'critical') => void;
}

export function QuickMessages({onSend}: QuickMessagesProps) {
  return (
    <View style={styles.container}>
      <View style={styles.grid}>
        {QUICK_MESSAGES.map((msg, index) => {
          const isEmergency = msg.priority === 'critical';
          return (
            <TouchableOpacity
              key={index}
              style={[styles.pill, isEmergency && styles.emergencyPill]}
              onPress={() => onSend(msg.text, msg.priority)}
              activeOpacity={0.7}>
              <Text style={styles.emoji}>{msg.emoji}</Text>
              <Text
                style={[styles.pillText, isEmergency && styles.emergencyText]}
                numberOfLines={1}>
                {msg.text}
              </Text>
            </TouchableOpacity>
          );
        })}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    backgroundColor: '#F2F2F7',
    paddingHorizontal: 8,
    paddingVertical: 8,
    borderTopWidth: 1,
    borderTopColor: '#E5E5E5',
  },
  grid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 6,
  },
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    borderRadius: 18,
    paddingHorizontal: 12,
    paddingVertical: 8,
    width: '48%',
    borderWidth: 1,
    borderColor: '#E5E5E5',
    gap: 6,
  },
  emergencyPill: {
    backgroundColor: '#FFF0F0',
    borderColor: '#FF3B30',
  },
  emoji: {
    fontSize: 16,
  },
  pillText: {
    fontSize: 13,
    color: '#1C1C1E',
    flex: 1,
  },
  emergencyText: {
    color: '#FF3B30',
    fontWeight: '600',
  },
});
