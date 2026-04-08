import React from 'react';
import {View, Text, FlatList, StyleSheet} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {formatRelativeTime, generateAvatarColor, getUserInitials} from '../utils';
import type {Neighbor} from '../types';

export function PeersScreen() {
  const {neighbors} = useProtocol();
  const peerList = Array.from(neighbors.values());

  const renderPeer = ({item}: {item: Neighbor}) => {
    const color = generateAvatarColor(item.peerId);
    const initials = getUserInitials(item.peerId);

    return (
      <View style={styles.peerRow}>
        <View style={[styles.avatar, {backgroundColor: color}]}>
          <Text style={styles.avatarText}>{initials}</Text>
        </View>
        <View style={styles.peerInfo}>
          <Text style={styles.peerId} numberOfLines={1}>{item.peerId}</Text>
          <Text style={styles.peerMeta}>
            {item.transport} - discovered {formatRelativeTime(item.discoveredAt)}
          </Text>
        </View>
        <View style={styles.statusDot} />
      </View>
    );
  };

  return (
    <View style={styles.container}>
      {peerList.length === 0 ? (
        <View style={styles.emptyContainer}>
          <Text style={styles.emptyTitle}>No Peers Found</Text>
          <Text style={styles.emptySubtitle}>
            Peers on the same Nostr relays will appear here when discovered.
          </Text>
        </View>
      ) : (
        <FlatList
          data={peerList}
          keyExtractor={item => item.peerId}
          renderItem={renderPeer}
          contentContainerStyle={styles.list}
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  list: {
    paddingVertical: 8,
  },
  peerRow: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 12,
    backgroundColor: '#FFFFFF',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  avatar: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  avatarText: {
    color: '#FFFFFF',
    fontSize: 14,
    fontWeight: '700',
  },
  peerInfo: {
    flex: 1,
    marginLeft: 12,
  },
  peerId: {
    fontSize: 15,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  peerMeta: {
    fontSize: 12,
    color: '#8E8E93',
    marginTop: 2,
  },
  statusDot: {
    width: 10,
    height: 10,
    borderRadius: 5,
    backgroundColor: '#4CAF50',
  },
  emptyContainer: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
  },
  emptyTitle: {
    fontSize: 18,
    fontWeight: '600',
    color: '#1C1C1E',
    marginBottom: 8,
  },
  emptySubtitle: {
    fontSize: 14,
    color: '#8E8E93',
    textAlign: 'center',
    lineHeight: 20,
  },
});
