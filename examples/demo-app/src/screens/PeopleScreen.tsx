import React, {useCallback} from 'react';
import {
  View,
  Text,
  SectionList,
  TouchableOpacity,
  StyleSheet,
  Alert,
} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {Avatar} from '../components/Avatar';
import {PresenceIndicator} from '../components/PresenceIndicator';
import {formatUserId} from '../utils';

interface PeopleScreenProps {
  onOpenChat: (peerId: string) => void;
}

export function PeopleScreen({onOpenChat}: PeopleScreenProps) {
  const {
    neighbors,
    contacts,
    connectionRequests,
    sendConnectionRequest,
    acceptConnectionRequest,
    rejectConnectionRequest,
    blockUser,
    unblockUser,
  } = useProtocol();

  const incomingRequests = connectionRequests.filter(r => r.direction === 'in');
  const outgoingRequests = connectionRequests.filter(r => r.direction === 'out');
  const allRequests = [...incomingRequests, ...outgoingRequests];

  // Neighbors not yet in contacts and without pending requests
  const requestedPeerIds = new Set(connectionRequests.map(r => r.peerId));
  const nearbyPeers = Array.from(neighbors.values()).filter(
    n => !contacts.has(n.peerId) && !requestedPeerIds.has(n.peerId),
  );

  const contactList = Array.from(contacts.values()).filter(c => !c.isBlocked);
  const blockedList = Array.from(contacts.values()).filter(c => c.isBlocked);

  const sections = [
    ...(allRequests.length > 0
      ? [{title: 'Connection Requests', data: allRequests.map(r => ({type: 'request' as const, ...r}))}]
      : []),
    ...(nearbyPeers.length > 0
      ? [{title: 'Nearby Peers', data: nearbyPeers.map(n => ({type: 'neighbor' as const, ...n}))}]
      : []),
    ...(contactList.length > 0
      ? [{title: 'Contacts', data: contactList.map(c => ({type: 'contact' as const, ...c}))}]
      : []),
    ...(blockedList.length > 0
      ? [{title: 'Blocked', data: blockedList.map(c => ({type: 'blocked' as const, ...c}))}]
      : []),
  ];

  const handleLongPressContact = useCallback((peerId: string, name: string) => {
    Alert.alert(name, 'Manage contact', [
      {text: 'Cancel', style: 'cancel'},
      {
        text: 'Block User',
        style: 'destructive',
        onPress: () => blockUser(peerId),
      },
    ]);
  }, [blockUser]);

  const handleLongPressBlocked = useCallback((peerId: string, name: string) => {
    Alert.alert(name, 'This user is blocked', [
      {text: 'Cancel', style: 'cancel'},
      {
        text: 'Unblock',
        onPress: () => unblockUser(peerId),
      },
    ]);
  }, [unblockUser]);

  const getRssiBars = (rssi?: number): string => {
    if (!rssi) {return '▪▪▪▪';}
    if (rssi > -60) {return '▪▪▪▪';}
    if (rssi > -70) {return '▪▪▪░';}
    if (rssi > -80) {return '▪▪░░';}
    return '▪░░░';
  };

  const renderItem = ({item}: {item: any}) => {
    if (item.type === 'request') {
      const isIncoming = item.direction === 'in';
      return (
        <View style={styles.row}>
          <Avatar userId={item.peerId} name={item.name || item.peerId} size={40} />
          <View style={styles.rowContent}>
            <Text style={styles.name} numberOfLines={1}>
              {item.name || formatUserId(item.peerId)}
            </Text>
            <Text style={styles.subtitle}>
              {isIncoming ? 'Wants to connect' : 'Pending...'}
            </Text>
          </View>
          {isIncoming && (
            <View style={styles.requestActions}>
              <TouchableOpacity
                style={[styles.actionButton, styles.acceptButton]}
                onPress={() => acceptConnectionRequest(item.peerId)}>
                <Text style={styles.acceptText}>Accept</Text>
              </TouchableOpacity>
              <TouchableOpacity
                style={[styles.actionButton, styles.rejectButton]}
                onPress={() => rejectConnectionRequest(item.peerId)}>
                <Text style={styles.rejectText}>Reject</Text>
              </TouchableOpacity>
            </View>
          )}
          {!isIncoming && (
            <Text style={styles.pendingLabel}>Pending</Text>
          )}
        </View>
      );
    }

    if (item.type === 'neighbor') {
      return (
        <View style={styles.row}>
          <Avatar userId={item.peerId} name={item.peerId} size={40} />
          <View style={styles.rowContent}>
            <Text style={styles.name} numberOfLines={1}>{formatUserId(item.peerId)}</Text>
            <Text style={styles.subtitle}>
              {getRssiBars(item.rssi)} {item.transport?.toUpperCase()}
            </Text>
          </View>
          <TouchableOpacity
            style={[styles.actionButton, styles.connectButton]}
            onPress={() => sendConnectionRequest(item.peerId)}>
            <Text style={styles.connectText}>Connect</Text>
          </TouchableOpacity>
        </View>
      );
    }

    if (item.type === 'blocked') {
      return (
        <TouchableOpacity
          style={styles.row}
          onLongPress={() => handleLongPressBlocked(item.peerId, item.name)}>
          <Avatar userId={item.peerId} name={item.name} size={40} />
          <View style={styles.rowContent}>
            <Text style={[styles.name, styles.blockedName]} numberOfLines={1}>
              {item.name || formatUserId(item.peerId)}
            </Text>
            <Text style={styles.subtitle}>Blocked</Text>
          </View>
        </TouchableOpacity>
      );
    }

    // Contact
    return (
      <TouchableOpacity
        style={styles.row}
        onPress={() => onOpenChat(item.peerId)}
        onLongPress={() => handleLongPressContact(item.peerId, item.name)}>
        <Avatar userId={item.peerId} name={item.name} size={40} />
        <View style={styles.rowContent}>
          <Text style={styles.name} numberOfLines={1}>
            {item.name || formatUserId(item.peerId)}
          </Text>
          <PresenceIndicator isNearby={item.isNearby} lastSeen={item.lastSeen} />
        </View>
        {item.hasSession && <Text style={styles.lock}>🔒</Text>}
      </TouchableOpacity>
    );
  };

  if (sections.length === 0) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyEmoji}>📡</Text>
        <Text style={styles.emptyTitle}>Searching for peers...</Text>
        <Text style={styles.emptySubtitle}>
          Make sure Bluetooth is enabled on nearby devices running the Offline Demo app.
        </Text>
      </View>
    );
  }

  return (
    <SectionList
      sections={sections}
      renderItem={renderItem}
      renderSectionHeader={({section}) => (
        <View style={styles.sectionHeader}>
          <Text style={styles.sectionTitle}>{section.title}</Text>
        </View>
      )}
      keyExtractor={(item) => `${item.type}-${item.peerId}`}
      contentContainerStyle={styles.list}
      stickySectionHeadersEnabled={false}
    />
  );
}

const styles = StyleSheet.create({
  list: {
    paddingBottom: 16,
  },
  sectionHeader: {
    paddingHorizontal: 16,
    paddingTop: 20,
    paddingBottom: 8,
    backgroundColor: '#F2F2F7',
  },
  sectionTitle: {
    fontSize: 13,
    fontWeight: '600',
    color: '#8E8E93',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 12,
  },
  rowContent: {
    flex: 1,
    gap: 2,
  },
  name: {
    fontSize: 16,
    fontWeight: '500',
    color: '#1C1C1E',
  },
  blockedName: {
    color: '#8E8E93',
  },
  subtitle: {
    fontSize: 13,
    color: '#8E8E93',
  },
  requestActions: {
    flexDirection: 'row',
    gap: 8,
  },
  actionButton: {
    paddingHorizontal: 14,
    paddingVertical: 7,
    borderRadius: 16,
  },
  acceptButton: {
    backgroundColor: '#34C759',
  },
  acceptText: {
    color: '#FFFFFF',
    fontSize: 13,
    fontWeight: '600',
  },
  rejectButton: {
    backgroundColor: '#F2F2F7',
  },
  rejectText: {
    color: '#FF3B30',
    fontSize: 13,
    fontWeight: '600',
  },
  connectButton: {
    backgroundColor: '#007AFF',
  },
  connectText: {
    color: '#FFFFFF',
    fontSize: 13,
    fontWeight: '600',
  },
  pendingLabel: {
    fontSize: 13,
    color: '#8E8E93',
    fontStyle: 'italic',
  },
  lock: {
    fontSize: 14,
  },
  empty: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
  },
  emptyEmoji: {
    fontSize: 48,
    marginBottom: 16,
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
