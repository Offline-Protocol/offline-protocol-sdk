import React from 'react';
import {View, TouchableOpacity, Text, StyleSheet} from 'react-native';
import type {TabName} from '../types';

interface TabBarProps {
  activeTab: TabName;
  onTabChange: (tab: TabName) => void;
  unreadChats: number;
}

const TABS: {key: TabName; label: string; icon: string}[] = [
  {key: 'people', label: 'People', icon: '👥'},
  {key: 'chats', label: 'Chats', icon: '💬'},
  {key: 'groups', label: 'Groups', icon: '👨‍👩‍👧‍👦'},
  {key: 'services', label: 'Services', icon: '⚡'},
];

export function TabBar({activeTab, onTabChange, unreadChats}: TabBarProps) {
  return (
    <View style={styles.container}>
      {TABS.map(tab => {
        const isActive = activeTab === tab.key;
        const showBadge = tab.key === 'chats' && unreadChats > 0;

        return (
          <TouchableOpacity
            key={tab.key}
            style={[styles.tab, isActive && styles.activeTab]}
            onPress={() => onTabChange(tab.key)}>
            <View style={styles.iconContainer}>
              <Text style={styles.icon}>{tab.icon}</Text>
              {showBadge && (
                <View style={styles.badge}>
                  <Text style={styles.badgeText}>
                    {unreadChats > 9 ? '9+' : unreadChats}
                  </Text>
                </View>
              )}
            </View>
            <Text style={[styles.label, isActive && styles.activeLabel]}>
              {tab.label}
            </Text>
          </TouchableOpacity>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flexDirection: 'row',
    backgroundColor: '#FFFFFF',
    borderTopWidth: 1,
    borderTopColor: '#E5E5E5',
    paddingBottom: 20,
    paddingTop: 8,
  },
  tab: {
    flex: 1,
    alignItems: 'center',
    paddingVertical: 4,
  },
  activeTab: {
    borderTopWidth: 2,
    borderTopColor: '#007AFF',
    marginTop: -1,
  },
  iconContainer: {
    position: 'relative',
  },
  icon: {
    fontSize: 22,
  },
  badge: {
    position: 'absolute',
    top: -4,
    right: -10,
    backgroundColor: '#FF3B30',
    borderRadius: 8,
    minWidth: 16,
    height: 16,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 4,
  },
  badgeText: {
    color: '#FFFFFF',
    fontSize: 10,
    fontWeight: '700',
  },
  label: {
    fontSize: 11,
    color: '#8E8E93',
    marginTop: 2,
  },
  activeLabel: {
    color: '#007AFF',
    fontWeight: '600',
  },
});
