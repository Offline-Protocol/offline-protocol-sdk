import React from 'react';
import {View, Text, StyleSheet} from 'react-native';
import {getUserInitials, generateAvatarColor} from '../utils';

interface AvatarProps {
  userId: string;
  name: string;
  size?: number;
}

export function Avatar({userId, name, size = 40}: AvatarProps) {
  const initials = getUserInitials(name);
  const backgroundColor = generateAvatarColor(userId);

  return (
    <View style={[styles.container, {width: size, height: size, borderRadius: size / 2, backgroundColor}]}>
      <Text style={[styles.initials, {fontSize: size * 0.38}]}>{initials}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  initials: {
    color: '#FFFFFF',
    fontWeight: '700',
  },
});
