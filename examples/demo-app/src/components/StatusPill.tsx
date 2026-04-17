import React from 'react';
import {View, Text, TouchableOpacity, StyleSheet} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {transportColor, transportLabel} from '../telemetryFormat';

interface Props {
  onPress?: () => void;
}

/**
 * Persistent ambient indicator wired to the live TelemetrySink stream.
 * Reads `latestMetrics.currentTransport` and `deviceCapability` (battery,
 * charging, relay role). Invisible until the first MetricsFrame arrives so it
 * doesn't show stale defaults during onboarding.
 */
export function StatusPill({onPress}: Props) {
  const {latestMetrics, deviceCapability, neighbors} = useProtocol();

  if (!latestMetrics) {
    return null;
  }

  const transport = latestMetrics.currentTransport;
  const battery = deviceCapability?.batteryLevel;
  const charging = deviceCapability?.isCharging ?? false;
  const isRelay = latestMetrics.isLocalRelay;
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
      {battery !== undefined && battery !== null && (
        <>
          <View style={styles.divider} />
          <BatteryGlyph level={battery} charging={charging} />
          <Text style={styles.battery}>{battery}%</Text>
        </>
      )}
      {isRelay && (
        <>
          <View style={styles.divider} />
          <Text style={styles.relay}>RELAY</Text>
        </>
      )}
    </Wrapper>
  );
}

function BatteryGlyph({level, charging}: {level: number; charging: boolean}) {
  const fill = Math.max(0, Math.min(100, level));
  const color =
    charging ? '#34C759' : fill <= 20 ? '#FF3B30' : fill <= 40 ? '#FF9500' : '#34C759';
  return (
    <View style={styles.batteryGlyph}>
      <View style={styles.batteryBody}>
        <View style={[styles.batteryFill, {width: `${fill}%`, backgroundColor: color}]} />
      </View>
      <View style={styles.batteryTip} />
      {charging && <Text style={styles.bolt}>⚡</Text>}
    </View>
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
  battery: {
    fontSize: 11,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  batteryGlyph: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  batteryBody: {
    width: 18,
    height: 9,
    borderWidth: 1,
    borderColor: '#1C1C1E',
    borderRadius: 2,
    overflow: 'hidden',
    justifyContent: 'center',
  },
  batteryFill: {
    height: '100%',
  },
  batteryTip: {
    width: 2,
    height: 4,
    backgroundColor: '#1C1C1E',
    marginLeft: 1,
    borderTopRightRadius: 1,
    borderBottomRightRadius: 1,
  },
  bolt: {
    position: 'absolute',
    fontSize: 8,
    left: 5,
    top: -2,
  },
  relay: {
    fontSize: 9,
    fontWeight: '800',
    color: '#FFFFFF',
    backgroundColor: '#5856D6',
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 4,
    overflow: 'hidden',
    letterSpacing: 0.5,
  },
});
