import React from 'react';
import {View, TouchableOpacity, Text, StyleSheet} from 'react-native';
import type {TabName} from '../types';

interface TabBarProps {
  activeTab: TabName;
  onTabChange: (tab: TabName) => void;
  unreadChats: number;
}

const TABS: {key: TabName; label: string; icon: string}[] = [
  {key: 'peers', label: 'Peers', icon: 'P'},
  {key: 'chat', label: 'Chat', icon: 'C'},
  {key: 'logs', label: 'Logs', icon: 'L'},
];

export function TabBar({activeTab, onTabChange, unreadChats}: TabBarProps) {
  return (
    <View style={styles.container}>
      {TABS.map(tab => {
        const isActive = activeTab === tab.key;
        const showBadge = tab.key === 'chat' && unreadChats > 0;

        return (
          <TouchableOpacity
            key={tab.key}
            style={[styles.tab, isActive && styles.activeTab]}
            onPress={() => onTabChange(tab.key)}>
            <View style={styles.iconContainer}>
              <View style={[styles.iconCircle, isActive && styles.activeIconCircle]}>
                <Text style={[styles.icon, isActive && styles.activeIcon]}>
                  {tab.icon}
                </Text>
              </View>
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
    borderTopColor: '#7B1FA2',
    marginTop: -1,
  },
  iconContainer: {
    position: 'relative',
  },
  iconCircle: {
    width: 28,
    height: 28,
    borderRadius: 14,
    backgroundColor: '#F2F2F7',
    alignItems: 'center',
    justifyContent: 'center',
  },
  activeIconCircle: {
    backgroundColor: '#F3E5F5',
  },
  icon: {
    fontSize: 14,
    fontWeight: '700',
    color: '#8E8E93',
  },
  activeIcon: {
    color: '#7B1FA2',
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
    color: '#7B1FA2',
    fontWeight: '600',
  },
});
