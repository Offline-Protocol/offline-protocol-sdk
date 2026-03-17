import React, {useState, useRef, useEffect} from 'react';
import {
  View,
  Text,
  FlatList,
  TouchableOpacity,
  TextInput,
  StyleSheet,
  Alert,
} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {Avatar} from '../components/Avatar';
import {MessageBubble} from '../components/MessageBubble';
import {QuickMessages} from '../components/QuickMessages';
import {formatUserId} from '../utils';

type ViewState = 'list' | 'detail' | 'create';

export function GroupsScreen() {
  const [viewState, setViewState] = useState<ViewState>('list');
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [showMembers, setShowMembers] = useState(false);
  const [createName, setCreateName] = useState('');
  const [selectedMembers, setSelectedMembers] = useState<Set<string>>(new Set());
  const {groups, contacts, createGroup, sendGroupMessage, leaveGroup, userId} = useProtocol();
  const listRef = useRef<FlatList>(null);

  // Reset to list if the selected group was deleted (e.g. after leaving)
  useEffect(() => {
    if (viewState === 'detail' && selectedGroupId && !groups.has(selectedGroupId)) {
      setViewState('list');
    }
  }, [viewState, selectedGroupId, groups]);

  // ─── Create Group Modal ──────────────────────────────────

  const handleCreate = async () => {
    if (!createName.trim()) {
      Alert.alert('Name Required', 'Please enter a group name.');
      return;
    }
    try {
      await createGroup(createName.trim(), Array.from(selectedMembers));
      setCreateName('');
      setSelectedMembers(new Set());
      setViewState('list');
    } catch (error) {
      console.warn('Failed to create group:', error);
      Alert.alert('Error', 'Failed to create group.');
    }
  };

  const toggleMember = (peerId: string) => {
    setSelectedMembers(prev => {
      const next = new Set(prev);
      if (next.has(peerId)) {
        next.delete(peerId);
      } else {
        next.add(peerId);
      }
      return next;
    });
  };

  if (viewState === 'create') {
    const contactList = Array.from(contacts.values()).filter(c => c.hasSession && !c.isBlocked);

    return (
      <View style={styles.container}>
        <View style={styles.header}>
          <TouchableOpacity onPress={() => setViewState('list')}>
            <Text style={styles.backText}>{'‹ Back'}</Text>
          </TouchableOpacity>
          <Text style={styles.headerTitle}>New Group</Text>
          <TouchableOpacity onPress={handleCreate}>
            <Text style={[styles.backText, !createName.trim() && styles.disabledText]}>
              Create
            </Text>
          </TouchableOpacity>
        </View>

        <View style={styles.createForm}>
          <Text style={styles.formLabel}>Group Name</Text>
          <TextInput
            style={styles.formInput}
            value={createName}
            onChangeText={setCreateName}
            placeholder="Enter group name"
            maxLength={30}
          />
        </View>

        <Text style={styles.formLabel2}>
          Add Members ({selectedMembers.size} selected)
        </Text>

        {contactList.length === 0 ? (
          <Text style={styles.noContacts}>
            No contacts with secure sessions available.
          </Text>
        ) : (
          <FlatList
            data={contactList}
            renderItem={({item}) => {
              const isSelected = selectedMembers.has(item.peerId);
              return (
                <TouchableOpacity
                  style={styles.memberRow}
                  onPress={() => toggleMember(item.peerId)}>
                  <Avatar userId={item.peerId} name={item.name} size={36} />
                  <Text style={styles.memberName} numberOfLines={1}>
                    {item.name || formatUserId(item.peerId)}
                  </Text>
                  <View style={[styles.checkbox, isSelected && styles.checkboxSelected]}>
                    {isSelected && <Text style={styles.checkmark}>✓</Text>}
                  </View>
                </TouchableOpacity>
              );
            }}
            keyExtractor={item => item.peerId}
          />
        )}
      </View>
    );
  }

  // ─── Group Detail ────────────────────────────────────────

  if (viewState === 'detail' && selectedGroupId) {
    const group = groups.get(selectedGroupId);
    if (!group) {
      // useEffect above will reset viewState on next tick
      return null;
    }

    const handleSend = (text: string, priority: 'medium' | 'critical') => {
      sendGroupMessage(selectedGroupId, text, priority);
      setTimeout(() => {
        listRef.current?.scrollToEnd({animated: true});
      }, 100);
    };

    const handleLeave = () => {
      Alert.alert('Leave Group', `Leave "${group.name}"?`, [
        {text: 'Cancel', style: 'cancel'},
        {
          text: 'Leave',
          style: 'destructive',
          onPress: async () => {
            try {
              await leaveGroup(selectedGroupId);
              setViewState('list');
            } catch {
              Alert.alert('Error', 'Failed to leave group.');
            }
          },
        },
      ]);
    };

    const getContactName = (peerId: string): string => {
      if (peerId === userId) {return 'You';}
      return contacts.get(peerId)?.name || formatUserId(peerId);
    };

    return (
      <View style={styles.container}>
        {/* Header */}
        <View style={styles.header}>
          <TouchableOpacity onPress={() => setViewState('list')}>
            <Text style={styles.backText}>{'‹ Back'}</Text>
          </TouchableOpacity>
          <View style={styles.headerCenter}>
            <Text style={styles.headerTitle} numberOfLines={1}>{group.name}</Text>
            <Text style={styles.headerSubtitle}>
              {group.members.length} member{group.members.length !== 1 ? 's' : ''} 🔒
            </Text>
          </View>
          <TouchableOpacity onPress={handleLeave}>
            <Text style={styles.leaveText}>Leave</Text>
          </TouchableOpacity>
        </View>

        {/* Members (collapsible) */}
        <TouchableOpacity
          style={styles.membersToggle}
          onPress={() => setShowMembers(!showMembers)}>
          <Text style={styles.membersToggleText}>
            {showMembers ? '▼' : '▶'} Members ({group.members.length})
          </Text>
        </TouchableOpacity>
        {showMembers && (
          <View style={styles.membersList}>
            {group.members.map(memberId => (
              <View key={memberId} style={styles.memberChip}>
                <Text style={styles.memberChipText}>
                  {getContactName(memberId)}
                </Text>
              </View>
            ))}
          </View>
        )}

        {/* Messages */}
        <FlatList
          ref={listRef}
          data={group.messages}
          renderItem={({item}) => (
            <MessageBubble
              message={item}
              showSender
              senderName={getContactName(item.senderId)}
            />
          )}
          keyExtractor={item => item.id}
          contentContainerStyle={styles.messageList}
          onContentSizeChange={() => {
            if (group.messages.length > 0) {
              listRef.current?.scrollToEnd({animated: false});
            }
          }}
          ListEmptyComponent={
            <View style={styles.emptyChat}>
              <Text style={styles.emptyChatText}>
                Send the first message in {group.name}
              </Text>
            </View>
          }
        />

        {/* Quick Messages */}
        <QuickMessages onSend={handleSend} />
      </View>
    );
  }

  // ─── Group List ──────────────────────────────────────────

  const groupList = Array.from(groups.values());

  return (
    <View style={styles.container}>
      <View style={styles.listHeader}>
        <Text style={styles.listTitle}>Groups</Text>
        <TouchableOpacity
          style={styles.newGroupButton}
          onPress={() => setViewState('create')}>
          <Text style={styles.newGroupText}>+ New</Text>
        </TouchableOpacity>
      </View>

      {groupList.length === 0 ? (
        <View style={styles.empty}>
          <Text style={styles.emptyEmoji}>👨‍👩‍👧‍👦</Text>
          <Text style={styles.emptyTitle}>No Groups Yet</Text>
          <Text style={styles.emptySubtitle}>
            Create a group and invite your connected peers for encrypted group messaging.
          </Text>
        </View>
      ) : (
        <FlatList
          data={groupList}
          renderItem={({item}) => {
            const lastMsg = item.messages.length > 0
              ? item.messages[item.messages.length - 1]
              : null;

            return (
              <TouchableOpacity
                style={styles.groupRow}
                onPress={() => {
                  setSelectedGroupId(item.id);
                  setViewState('detail');
                }}>
                <View style={styles.groupIcon}>
                  <Text style={styles.groupIconText}>
                    {item.name.charAt(0).toUpperCase()}
                  </Text>
                </View>
                <View style={styles.groupRowContent}>
                  <Text style={styles.groupName} numberOfLines={1}>{item.name}</Text>
                  <Text style={styles.groupMeta} numberOfLines={1}>
                    {item.members.length} members
                    {lastMsg ? ` · ${lastMsg.content}` : ''}
                  </Text>
                </View>
                <Text style={styles.lock}>🔒</Text>
              </TouchableOpacity>
            );
          }}
          keyExtractor={item => item.id}
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#F2F2F7',
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 8,
  },
  headerCenter: {
    flex: 1,
    alignItems: 'center',
  },
  headerTitle: {
    fontSize: 16,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  headerSubtitle: {
    fontSize: 12,
    color: '#8E8E93',
  },
  backText: {
    fontSize: 17,
    color: '#007AFF',
    fontWeight: '500',
  },
  disabledText: {
    color: '#C7C7CC',
  },
  leaveText: {
    fontSize: 15,
    color: '#FF3B30',
    fontWeight: '500',
  },
  listHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 12,
    backgroundColor: '#FFFFFF',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  listTitle: {
    fontSize: 18,
    fontWeight: '700',
    color: '#1C1C1E',
  },
  newGroupButton: {
    backgroundColor: '#007AFF',
    paddingHorizontal: 14,
    paddingVertical: 7,
    borderRadius: 16,
  },
  newGroupText: {
    color: '#FFFFFF',
    fontSize: 14,
    fontWeight: '600',
  },
  groupRow: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 12,
  },
  groupIcon: {
    width: 48,
    height: 48,
    borderRadius: 24,
    backgroundColor: '#5856D6',
    alignItems: 'center',
    justifyContent: 'center',
  },
  groupIconText: {
    color: '#FFFFFF',
    fontSize: 20,
    fontWeight: '700',
  },
  groupRowContent: {
    flex: 1,
    gap: 2,
  },
  groupName: {
    fontSize: 16,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  groupMeta: {
    fontSize: 13,
    color: '#8E8E93',
  },
  lock: {
    fontSize: 14,
  },
  // Create group
  createForm: {
    backgroundColor: '#FFFFFF',
    padding: 16,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  formLabel: {
    fontSize: 14,
    fontWeight: '600',
    color: '#3C3C43',
    marginBottom: 8,
    paddingHorizontal: 16,
  },
  formLabel2: {
    fontSize: 14,
    fontWeight: '600',
    color: '#3C3C43',
    paddingHorizontal: 16,
    paddingTop: 16,
    paddingBottom: 8,
  },
  formInput: {
    backgroundColor: '#F2F2F7',
    borderRadius: 10,
    paddingHorizontal: 14,
    paddingVertical: 12,
    fontSize: 16,
    color: '#1C1C1E',
  },
  noContacts: {
    fontSize: 14,
    color: '#8E8E93',
    textAlign: 'center',
    padding: 32,
  },
  memberRow: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 12,
  },
  memberName: {
    flex: 1,
    fontSize: 16,
    color: '#1C1C1E',
  },
  checkbox: {
    width: 24,
    height: 24,
    borderRadius: 12,
    borderWidth: 2,
    borderColor: '#C7C7CC',
    alignItems: 'center',
    justifyContent: 'center',
  },
  checkboxSelected: {
    backgroundColor: '#007AFF',
    borderColor: '#007AFF',
  },
  checkmark: {
    color: '#FFFFFF',
    fontSize: 14,
    fontWeight: '700',
  },
  // Members section
  membersToggle: {
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  membersToggleText: {
    fontSize: 14,
    color: '#8E8E93',
    fontWeight: '500',
  },
  membersList: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 12,
    paddingVertical: 8,
    gap: 6,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  memberChip: {
    backgroundColor: '#F2F2F7',
    paddingHorizontal: 10,
    paddingVertical: 4,
    borderRadius: 12,
  },
  memberChipText: {
    fontSize: 13,
    color: '#3C3C43',
  },
  // Messages
  messageList: {
    flexGrow: 1,
    paddingVertical: 8,
  },
  emptyChat: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
  },
  emptyChatText: {
    fontSize: 14,
    color: '#8E8E93',
    textAlign: 'center',
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
