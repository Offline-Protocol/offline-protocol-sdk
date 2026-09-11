import React from 'react';
import {View, Text, TouchableOpacity, StyleSheet} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {transportColor, transportLabel} from '../telemetryFormat';

interface Props {
  onPress?: () => void;
}

/**
 * Persistent ambient indicator, fed by the free event stream: the transport
 * DORS currently routes over (`transport_switched`) and the neighbour count.
 * Invisible until the protocol has started so it doesn't show stale
 * defaults during onboarding.
 */
export function StatusPill({onPress}: Props) {
  const {currentTransport, neighbors, isStarted} = useProtocol();

  if (!isStarted) {
    return null;
  }

  const transport = currentTransport;
  const peers = neighbors.size;

  const dotColor = transport ? transportColor(transport) : '#C7C7CC';

  const Wrapper: any = onPress ? TouchableOpacity : View;
  const wrapperProps = onPress ? {onPress, activeOpacity: 0.7} : {};

  return (
    <Wrapper {...wrapperProps} style={styles.pill}>
      <View style={[styles.dot, {backgroundColor: dotColor}]} />
      <Text style={styles.transport}>
        {transport ? transportLabel(transport) : '—'}
      </Text>
      <View style={styles.divider} />
      <Text style={styles.peers}>{peers}</Text>
      <Text style={styles.peersIcon}>👥</Text>
    </Wrapper>
  );
}

const styles = StyleSheet.create({
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#F2F2F7',
    paddingHorizontal: 8,
    paddingVertical: 4,
    borderRadius: 999,
    gap: 6,
  },
  dot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  transport: {
    fontSize: 11,
    fontWeight: '700',
    color: '#1C1C1E',
    letterSpacing: 0.5,
  },
  divider: {
    width: StyleSheet.hairlineWidth,
    height: 12,
    backgroundColor: '#C7C7CC',
  },
  peers: {
    fontSize: 11,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  peersIcon: {
    fontSize: 10,
    marginLeft: -4,
  },
});
