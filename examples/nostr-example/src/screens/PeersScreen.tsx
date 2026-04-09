import React, {useState} from 'react';
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  FlatList,
  StyleSheet,
  Alert,
} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {formatUserId, generateAvatarColor, getUserInitials} from '../utils';
import type {Neighbor} from '../types';

export function PeersScreen() {
  const {
    neighbors,
    userId,
    sendConnectionRequest,
    acceptConnection,
    rejectConnection,
    cancelConnectionRequest,
  } = useProtocol();
  const [peerInput, setPeerInput] = useState('');

  const peerList = Array.from(neighbors.values());

  const handleConnect = async () => {
    const peerId = peerInput.trim();
    if (!peerId) {
      Alert.alert('Error', 'Enter a User ID to connect.');
      return;
    }
    if (peerId === userId) {
      Alert.alert('Error', 'You cannot connect to yourself.');
      return;
    }
    await sendConnectionRequest(peerId);
    setPeerInput('');
  };

  const renderPeer = ({item}: {item: Neighbor}) => {
    const color = generateAvatarColor(item.peerId);
    const initials = getUserInitials(item.displayName || item.peerId);
    const displayName = item.displayName || formatUserId(item.peerId);

    return (
      <View style={styles.peerRow}>
        <View style={[styles.avatar, {backgroundColor: color}]}>
          <Text style={styles.avatarText}>{initials}</Text>
        </View>
        <View style={styles.peerInfo}>
          <Text style={styles.peerName} numberOfLines={1}>
            {displayName}
          </Text>
          <Text style={styles.peerId} numberOfLines={1}>
            {formatUserId(item.peerId)}
          </Text>
          <Text style={styles.peerMeta}>
            {item.transport} · {statusLabel(item.connectionStatus)}
          </Text>
        </View>
        <View style={styles.actions}>
          {renderActions(item)}
        </View>
      </View>
    );
  };

  const renderActions = (peer: Neighbor) => {
    switch (peer.connectionStatus) {
      case 'none':
        return (
          <TouchableOpacity
            style={styles.connectBtn}
            onPress={() => sendConnectionRequest(peer.peerId)}>
            <Text style={styles.connectBtnText}>Connect</Text>
          </TouchableOpacity>
        );
      case 'pending_sent':
        return (
          <TouchableOpacity
            style={styles.cancelBtn}
            onPress={() => cancelConnectionRequest(peer.peerId)}>
            <Text style={styles.cancelBtnText}>Cancel</Text>
          </TouchableOpacity>
        );
      case 'pending_received':
        return (
          <View style={styles.requestActions}>
            <TouchableOpacity
              style={styles.acceptBtn}
              onPress={() => acceptConnection(peer.peerId)}>
              <Text style={styles.acceptBtnText}>Accept</Text>
            </TouchableOpacity>
            <TouchableOpacity
              style={styles.rejectBtn}
              onPress={() => rejectConnection(peer.peerId)}>
              <Text style={styles.rejectBtnText}>Reject</Text>
            </TouchableOpacity>
          </View>
        );
      case 'accepted':
        return (
          <View style={styles.connectedBadge}>
            <Text style={styles.connectedText}>Connected</Text>
          </View>
        );
      case 'rejected':
        return (
          <View style={styles.rejectedBadge}>
            <Text style={styles.rejectedText}>Rejected</Text>
          </View>
        );
      default:
        return null;
    }
  };

  return (
    <View style={styles.container}>
      {/* Connect Input */}
      <View style={styles.inputSection}>
        <TextInput
          style={styles.input}
          placeholder="Enter peer's User ID to connect"
          value={peerInput}
          onChangeText={setPeerInput}
          autoCapitalize="none"
          autoCorrect={false}
          onSubmitEditing={handleConnect}
          returnKeyType="send"
        />
        <TouchableOpacity style={styles.connectInputBtn} onPress={handleConnect}>
          <Text style={styles.connectInputBtnText}>Connect</Text>
        </TouchableOpacity>
      </View>

      {/* Your ID */}
      <View style={styles.yourIdSection}>
        <Text style={styles.yourIdLabel}>Your ID:</Text>
        <Text style={styles.yourIdValue} selectable>{userId}</Text>
      </View>

      {/* Peer List */}
      {peerList.length === 0 ? (
        <View style={styles.emptyContainer}>
          <Text style={styles.emptyTitle}>No Peers</Text>
          <Text style={styles.emptySubtitle}>
            Enter a peer's User ID above to send a connection request.
            Once accepted, you can start chatting.
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

function statusLabel(status: Neighbor['connectionStatus']): string {
  switch (status) {
    case 'none': return 'Not connected';
    case 'pending_sent': return 'Request sent';
    case 'pending_received': return 'Wants to connect';
    case 'accepted': return 'Connected';
    case 'rejected': return 'Rejected';
    default: return status;
  }
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  inputSection: {
    flexDirection: 'row',
    padding: 12,
    gap: 8,
    backgroundColor: '#FFFFFF',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  input: {
    flex: 1,
    backgroundColor: '#F2F2F7',
    borderRadius: 10,
    paddingHorizontal: 14,
    paddingVertical: 10,
    fontSize: 14,
  },
  connectInputBtn: {
    backgroundColor: '#7B1FA2',
    borderRadius: 10,
    paddingHorizontal: 16,
    justifyContent: 'center',
  },
  connectInputBtnText: {
    color: '#FFFFFF',
    fontWeight: '600',
    fontSize: 14,
  },
  yourIdSection: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 8,
    backgroundColor: '#F2F2F7',
    gap: 6,
  },
  yourIdLabel: {
    fontSize: 12,
    color: '#8E8E93',
    fontWeight: '600',
  },
  yourIdValue: {
    fontSize: 12,
    color: '#7B1FA2',
    fontFamily: 'monospace',
    flex: 1,
  },
  list: {
    paddingVertical: 4,
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
  peerName: {
    fontSize: 15,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  peerId: {
    fontSize: 11,
    color: '#8E8E93',
    fontFamily: 'monospace',
    marginTop: 1,
  },
  peerMeta: {
    fontSize: 11,
    color: '#8E8E93',
    marginTop: 2,
  },
  actions: {
    marginLeft: 8,
  },
  connectBtn: {
    backgroundColor: '#7B1FA2',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 6,
  },
  connectBtnText: {
    color: '#FFFFFF',
    fontSize: 12,
    fontWeight: '600',
  },
  cancelBtn: {
    backgroundColor: '#F2F2F7',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 6,
  },
  cancelBtnText: {
    color: '#8E8E93',
    fontSize: 12,
    fontWeight: '600',
  },
  requestActions: {
    flexDirection: 'column',
    gap: 4,
  },
  acceptBtn: {
    backgroundColor: '#4CAF50',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 6,
    alignItems: 'center',
  },
  acceptBtnText: {
    color: '#FFFFFF',
    fontSize: 12,
    fontWeight: '600',
  },
  rejectBtn: {
    backgroundColor: '#F2F2F7',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 6,
    alignItems: 'center',
  },
  rejectBtnText: {
    color: '#FF3B30',
    fontSize: 12,
    fontWeight: '600',
  },
  connectedBadge: {
    backgroundColor: '#E8F5E9',
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 4,
  },
  connectedText: {
    color: '#4CAF50',
    fontSize: 11,
    fontWeight: '600',
  },
  rejectedBadge: {
    backgroundColor: '#FFEBEE',
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 4,
  },
  rejectedText: {
    color: '#FF3B30',
    fontSize: 11,
    fontWeight: '600',
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
