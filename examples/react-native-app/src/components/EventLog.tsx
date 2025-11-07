import React from 'react';
import { View, Text, StyleSheet, ScrollView, TouchableOpacity } from 'react-native';
import type { ProtocolEvent } from '@offlineprotocol/react-native';

interface EventLogProps {
  events: ProtocolEvent[];
  onClear: () => void;
}

export function EventLog({ events, onClear }: EventLogProps) {
  const getEventColor = (eventType: string): string => {
    switch (eventType) {
      case 'message_sent':
        return '#2196f3';
      case 'message_received':
        return '#4caf50';
      case 'message_delivered':
        return '#8bc34a';
      case 'message_failed':
        return '#f44336';
      case 'transport_switched':
        return '#ff9800';
      case 'relay_promoted':
        return '#9c27b0';
      case 'relay_demoted':
        return '#673ab7';
      case 'neighbor_discovered':
        return '#00bcd4';
      case 'neighbor_lost':
        return '#607d8b';
      case 'network_metrics':
        return '#3f51b5';
      case 'file_progress':
        return '#ffc107';
      case 'file_received':
        return '#cddc39';
      case 'diagnostic':
        return '#607d8b';
      default:
        return '#9e9e9e';
    }
  };

  const formatEventData = (event: ProtocolEvent): string => {
    const eventCopy = { ...event };
    delete (eventCopy as any).type;
    return JSON.stringify(eventCopy, null, 2);
  };

  return (
    <View style={styles.container}>
      <View style={styles.header}>
        <Text style={styles.title}>Event Log ({events.length})</Text>
        <TouchableOpacity onPress={onClear} style={styles.clearButton}>
          <Text style={styles.clearButtonText}>Clear</Text>
        </TouchableOpacity>
      </View>
      <ScrollView style={styles.eventList}>
        {events.length === 0 ? (
          <Text style={styles.emptyText}>No events yet</Text>
        ) : (
          events.map((event, index) => (
            <View key={index} style={styles.eventItem}>
              <View style={[styles.eventIndicator, { backgroundColor: getEventColor(event.type) }]} />
              <View style={styles.eventContent}>
                <Text style={styles.eventType}>{event.type}</Text>
                <Text style={styles.eventData}>{formatEventData(event)}</Text>
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
  },
  header: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 12,
  },
  title: {
    fontSize: 18,
    fontWeight: 'bold',
    color: '#333',
  },
  clearButton: {
    paddingHorizontal: 16,
    paddingVertical: 8,
    backgroundColor: '#f44336',
    borderRadius: 6,
  },
  clearButtonText: {
    color: '#fff',
    fontSize: 14,
    fontWeight: '600',
  },
  eventList: {
    flex: 1,
  },
  emptyText: {
    textAlign: 'center',
    color: '#999',
    fontSize: 14,
    marginTop: 24,
  },
  eventItem: {
    flexDirection: 'row',
    padding: 12,
    backgroundColor: '#f9f9f9',
    borderRadius: 8,
    marginBottom: 8,
  },
  eventIndicator: {
    width: 4,
    borderRadius: 2,
    marginRight: 12,
  },
  eventContent: {
    flex: 1,
  },
  eventType: {
    fontSize: 14,
    fontWeight: '600',
    color: '#333',
    marginBottom: 4,
  },
  eventData: {
    fontSize: 12,
    color: '#666',
    fontFamily: 'monospace',
  },
});

