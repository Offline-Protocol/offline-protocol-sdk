import React, { useState, useEffect, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  FlatList,
  TouchableOpacity,
  RefreshControl,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';
import { Icon } from '../components/Icon';
import { CreateGroupModal } from './CreateGroupModal';
import { GroupDetailScreen } from './GroupDetailScreen';

export interface Group {
  groupId: string;
  name: string;
  createdAt: Date;
}

type Screen = 'list' | 'detail' | 'create';

interface GroupsScreenProps {
  onNavigateToGroupDetail?: (groupId: string, groupName: string) => void;
}

export function GroupsScreen({ onNavigateToGroupDetail }: GroupsScreenProps) {
  const { theme } = useTheme();
  const {
    groups,
    isInitialized,
    getUserGroups,
  } = useProtocol();
  const [currentScreen, setCurrentScreen] = useState<Screen>('list');
  const [selectedGroup, setSelectedGroup] = useState<{ groupId: string; name: string } | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [showCreateModal, setShowCreateModal] = useState(false);

  const handleRefresh = useCallback(() => {
    setRefreshing(true);
    if (getUserGroups && isInitialized) {
      getUserGroups();
    }
    setTimeout(() => setRefreshing(false), 1000);
  }, [getUserGroups, isInitialized]);

  const handleGroupPress = useCallback((group: Group) => {
    if (onNavigateToGroupDetail) {
      onNavigateToGroupDetail(group.groupId, group.name);
    } else {
      setSelectedGroup({ groupId: group.groupId, name: group.name });
      setCurrentScreen('detail');
    }
  }, [onNavigateToGroupDetail]);

  const handleCreateGroup = useCallback(() => {
    setShowCreateModal(true);
  }, []);

  const handleGroupCreated = useCallback(() => {
    setShowCreateModal(false);
    handleRefresh();
  }, [handleRefresh]);

  const handleBack = useCallback(() => {
    setCurrentScreen('list');
    setSelectedGroup(null);
  }, []);

  // Auto-refresh groups when protocol is initialized
  useEffect(() => {
    if (isInitialized && getUserGroups) {
      getUserGroups();
    }
  }, [isInitialized, getUserGroups]);

  if (currentScreen === 'detail' && selectedGroup) {
    return (
      <GroupDetailScreen
        groupId={selectedGroup.groupId}
        groupName={selectedGroup.name}
        onBack={handleBack}
      />
    );
  }

  if (showCreateModal) {
    return (
      <CreateGroupModal
        onClose={() => setShowCreateModal(false)}
        onGroupCreated={handleGroupCreated}
      />
    );
  }

  return (
    <SafeAreaView style={[styles.container, { backgroundColor: theme.colors.background }]}>
      <View style={[styles.header, { backgroundColor: theme.colors.surface }]}>
        <Text style={[styles.title, { color: theme.colors.text }]}>Groups</Text>
        <TouchableOpacity
          style={[styles.createButton, { backgroundColor: theme.colors.primary }]}
          onPress={handleCreateGroup}
        >
          <Icon name="add" size={24} color={theme.colors.textInverse} />
        </TouchableOpacity>
      </View>

      {groups.length === 0 ? (
        <View style={styles.emptyContainer}>
          <Icon name="people-outline" size={64} color={theme.colors.textSecondary} />
          <Text style={[styles.emptyText, { color: theme.colors.textSecondary }]}>
            No groups yet
          </Text>
          <Text style={[styles.emptySubtext, { color: theme.colors.textSecondary }]}>
            Create a group to start chatting
          </Text>
        </View>
      ) : (
        <FlatList
          data={groups}
          keyExtractor={(item) => item.groupId}
          renderItem={({ item }) => (
            <TouchableOpacity
              style={[styles.groupItem, { backgroundColor: theme.colors.surface }]}
              onPress={() => handleGroupPress(item)}
              activeOpacity={0.7}
            >
              <View style={[styles.groupIcon, { backgroundColor: theme.colors.primary + '20' }]}>
                <Icon name="people" size={24} color={theme.colors.primary} />
              </View>
              <View style={styles.groupInfo}>
                <Text style={[styles.groupName, { color: theme.colors.text }]}>
                  {item.name}
                </Text>
                <Text style={[styles.groupMeta, { color: theme.colors.textSecondary }]}>
                  Created {new Date(item.createdAt).toLocaleDateString()}
                </Text>
              </View>
              <Icon name="chevron-forward" size={20} color={theme.colors.textSecondary} />
            </TouchableOpacity>
          )}
          contentContainerStyle={styles.listContent}
          refreshControl={
            <RefreshControl
              refreshing={refreshing}
              onRefresh={handleRefresh}
              tintColor={theme.colors.primary}
            />
          }
        />
      )}
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
  title: {
    fontSize: 24,
    fontWeight: 'bold',
  },
  createButton: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  emptyContainer: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
  },
  emptyText: {
    fontSize: 18,
    fontWeight: '600',
    marginTop: 16,
  },
  emptySubtext: {
    fontSize: 14,
    marginTop: 8,
    textAlign: 'center',
  },
  listContent: {
    padding: 16,
  },
  groupItem: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 16,
    borderRadius: 12,
    marginBottom: 12,
  },
  groupIcon: {
    width: 48,
    height: 48,
    borderRadius: 24,
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 12,
  },
  groupInfo: {
    flex: 1,
  },
  groupName: {
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 4,
  },
  groupMeta: {
    fontSize: 12,
  },
});
