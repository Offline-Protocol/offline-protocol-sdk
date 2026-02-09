import React, { useState, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TextInput,
  TouchableOpacity,
  ScrollView,
  Alert,
  ActivityIndicator,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { useTheme } from '../hooks/useTheme';
import { useWebSocketRelayContext } from '../hooks/useWebSocketRelayContext';
import { useProtocol } from '../hooks/useProtocol';
import { Icon } from '../components/Icon';

interface CreateGroupModalProps {
  onClose: () => void;
  onGroupCreated: () => void;
}

export function CreateGroupModal({ onClose, onGroupCreated }: CreateGroupModalProps) {
  const { theme } = useTheme();
  const { send, authenticatedUser, status } = useWebSocketRelayContext();
  const { protocol, isInitialized } = useProtocol();
  const [groupName, setGroupName] = useState('');
  const [membersToAdd, setMembersToAdd] = useState<string[]>([]);
  const [usernameInput, setUsernameInput] = useState('');
  const [isCreating, setIsCreating] = useState(false);

  const handleAddMember = useCallback(() => {
    const username = usernameInput.trim();
    if (!username) {
      Alert.alert('Error', 'Please enter a username');
      return;
    }
    if (membersToAdd.includes(username)) {
      Alert.alert('Error', 'User already added');
      return;
    }
    setMembersToAdd([...membersToAdd, username]);
    setUsernameInput('');
  }, [usernameInput, membersToAdd]);

  const handleRemoveMember = useCallback((username: string) => {
    setMembersToAdd(membersToAdd.filter((u) => u !== username));
  }, [membersToAdd]);

  const handleCreateGroup = useCallback(async () => {
    if (!groupName.trim()) {
      Alert.alert('Error', 'Please enter a group name');
      return;
    }

    if (!isInitialized || !protocol) {
      Alert.alert(
        'Protocol Not Ready',
        'The protocol is not initialized yet. Please wait a moment and try again.',
      );
      return;
    }

    if (status !== 'authenticated' && status !== 'connected') {
      Alert.alert(
        'WebSocket Not Connected',
        `WebSocket status: ${status}. Please wait for the connection to be established.`,
      );
      return;
    }

    setIsCreating(true);
    try {
      // Create group using SDK
      const createGroupJson = await protocol.groupCreate(groupName.trim());
      const createGroupPayload = JSON.parse(createGroupJson);

      // Send via WebSocket
      const sent = send(createGroupPayload);
      if (!sent) {
        throw new Error('Failed to send group creation request');
      }

      // Wait for GroupCreated response (handled by WebSocketRelayProvider)
      // Then add members if any
      if (membersToAdd.length > 0) {
        // We'll need to wait for the group_id from the response
        // For now, we'll add members after a short delay
        setTimeout(async () => {
          // This will be handled by the GroupCreated event handler
          // which should trigger a refresh
        }, 1000);
      }

      // Close modal and refresh groups list
      onGroupCreated();
    } catch (error: any) {
      console.error('Failed to create group:', error);
      Alert.alert('Error', error.message || 'Failed to create group');
    } finally {
      setIsCreating(false);
    }
  }, [groupName, membersToAdd, protocol, send, status, onGroupCreated]);

  return (
    <SafeAreaView style={[styles.container, { backgroundColor: theme.colors.background }]}>
      <View style={[styles.header, { backgroundColor: theme.colors.surface }]}>
        <TouchableOpacity onPress={onClose} style={styles.closeButton}>
          <Icon name="close" size={24} color={theme.colors.text} />
        </TouchableOpacity>
        <Text style={[styles.title, { color: theme.colors.text }]}>Create Group</Text>
        <View style={styles.closeButton} />
      </View>

      <ScrollView style={styles.content} contentContainerStyle={styles.contentContainer}>
        {/* Connection Status Indicator */}
        {status !== 'authenticated' && (
          <View style={[styles.statusBanner, { backgroundColor: theme.colors.warning + '20' }]}>
            <Text style={[styles.statusText, { color: theme.colors.warning }]}>
              {status === 'connecting' && 'Connecting to server...'}
              {status === 'connected' && 'Connected, authenticating...'}
              {status === 'disconnected' && 'Disconnected. Please wait...'}
              {status === 'error' && 'Connection error. Retrying...'}
            </Text>
          </View>
        )}

        <View style={styles.section}>
          <Text style={[styles.label, { color: theme.colors.text }]}>Group Name</Text>
          <TextInput
            style={[
              styles.input,
              {
                backgroundColor: theme.colors.surface,
                color: theme.colors.text,
                borderColor: theme.colors.border,
              },
            ]}
            placeholder="Enter group name"
            placeholderTextColor={theme.colors.textSecondary}
            value={groupName}
            onChangeText={setGroupName}
            editable={!isCreating}
          />
        </View>

        <View style={styles.section}>
          <Text style={[styles.label, { color: theme.colors.text }]}>Add Members</Text>
          <View style={styles.addMemberContainer}>
            <TextInput
              style={[
                styles.input,
                {
                  backgroundColor: theme.colors.surface,
                  color: theme.colors.text,
                  borderColor: theme.colors.border,
                  flex: 1,
                  marginRight: 8,
                },
              ]}
              placeholder="Enter username"
              placeholderTextColor={theme.colors.textSecondary}
              value={usernameInput}
              onChangeText={setUsernameInput}
              onSubmitEditing={handleAddMember}
              editable={!isCreating}
            />
            <TouchableOpacity
              style={[styles.addButton, { backgroundColor: theme.colors.primary }]}
              onPress={handleAddMember}
              disabled={isCreating}
            >
              <Icon name="add" size={20} color={theme.colors.textInverse} />
            </TouchableOpacity>
          </View>

          {membersToAdd.length > 0 && (
            <View style={styles.membersList}>
              {membersToAdd.map((username) => (
                <View
                  key={username}
                  style={[styles.memberTag, { backgroundColor: theme.colors.primary + '20' }]}
                >
                  <Text style={[styles.memberTagText, { color: theme.colors.primary }]}>
                    {username}
                  </Text>
                  <TouchableOpacity
                    onPress={() => handleRemoveMember(username)}
                    disabled={isCreating}
                  >
                    <Icon name="close-circle" size={20} color={theme.colors.primary} />
                  </TouchableOpacity>
                </View>
              ))}
            </View>
          )}
        </View>

        <TouchableOpacity
          style={[
            styles.createButton,
            {
              backgroundColor: isCreating ? theme.colors.disabled : theme.colors.primary,
            },
          ]}
          onPress={handleCreateGroup}
          disabled={isCreating}
        >
          {isCreating ? (
            <ActivityIndicator color={theme.colors.textInverse} />
          ) : (
            <Text style={[styles.createButtonText, { color: theme.colors.textInverse }]}>
              Create Group
            </Text>
          )}
        </TouchableOpacity>
      </ScrollView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(0,0,0,0.1)',
  },
  closeButton: {
    width: 40,
    height: 40,
    alignItems: 'center',
    justifyContent: 'center',
  },
  title: {
    fontSize: 20,
    fontWeight: 'bold',
  },
  content: {
    flex: 1,
  },
  contentContainer: {
    padding: 16,
  },
  section: {
    marginBottom: 24,
  },
  label: {
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 8,
  },
  input: {
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 12,
    fontSize: 16,
  },
  addMemberContainer: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  addButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  membersList: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    marginTop: 12,
    gap: 8,
  },
  memberTag: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 16,
    marginRight: 8,
    marginBottom: 8,
  },
  memberTagText: {
    fontSize: 14,
    fontWeight: '500',
    marginRight: 6,
  },
  createButton: {
    paddingVertical: 16,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: 8,
  },
  createButtonText: {
    fontSize: 16,
    fontWeight: '600',
  },
  statusBanner: {
    padding: 12,
    borderRadius: 8,
    marginBottom: 16,
    alignItems: 'center',
  },
  statusText: {
    fontSize: 14,
    fontWeight: '500',
  },
});
