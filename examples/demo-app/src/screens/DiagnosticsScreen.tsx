import React, {useCallback, useEffect, useState} from 'react';
import {View, Text, ScrollView, StyleSheet, TouchableOpacity} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {MetricCard, StatTile, TileRow, KeyValueRow, TransportBadge} from '../components/TelemetryViz';
import {transportLabel, formatRelative} from '../telemetryFormat';
import type {
  BleDiagnostics,
  TelemetryStats,
  TransportMetrics,
  TransportType,
} from '@offline-protocol/mesh-sdk';

/**
 * Diagnostics screen.
 *
 * Two kinds of card. The telemetry card shows the SDK's own pipe: what it
 * has sent to the ingest, what the ingest accepted, what was dropped, and
 * the last error. Nothing here is a telemetry record; the pipe hands no
 * record to JavaScript, and the counters come from `telemetryStats()`.
 *
 * The other cards are fed by the free API (`getTransportMetrics`,
 * `getBleDiagnostics`, the neighbour map and `transport_switched`), which is
 * per-event and unaggregated by design and available whether or not
 * telemetry is enabled.
 */
const POLL_MS = 2000;
const POLLED_TRANSPORTS: TransportType[] = ['ble', 'wifiDirect', 'internet'];

export function DiagnosticsScreen() {
  const {protocol, isStarted, neighbors, currentTransport, telemetryEnabled} = useProtocol();

  const [stats, setStats] = useState<TelemetryStats | null>(null);
  const [metrics, setMetrics] = useState<Map<TransportType, TransportMetrics>>(new Map());
  const [ble, setBle] = useState<BleDiagnostics | null>(null);
  const [tick, setTick] = useState(0);

  const poll = useCallback(async () => {
    if (!protocol || !isStarted) {
      return;
    }
    try {
      setStats(await protocol.telemetryStats());
    } catch {
      /* not enabled, or the native instance is gone */
    }
    const next = new Map<TransportType, TransportMetrics>();
    for (const transport of POLLED_TRANSPORTS) {
      try {
        const m = await protocol.getTransportMetrics(transport);
        if (m) {
          next.set(transport, m);
        }
      } catch {
        /* transport not configured */
      }
    }
    setMetrics(next);
    try {
      setBle(await protocol.getBleDiagnostics());
    } catch {
      /* BLE not configured */
    }
    setTick(t => t + 1);
  }, [protocol, isStarted]);

  useEffect(() => {
    poll();
    const interval = setInterval(poll, POLL_MS);
    return () => clearInterval(interval);
  }, [poll]);

  return (
    <ScrollView
      style={styles.scroll}
      contentContainerStyle={styles.content}
      showsVerticalScrollIndicator={false}>
      <TelemetryCard stats={stats} enabled={telemetryEnabled} protocol={protocol} />
      <MeshHealthCard neighbors={neighbors.size} currentTransport={currentTransport} />
      <TransportsCard metrics={metrics} currentTransport={currentTransport} />
      <BleCard diagnostics={ble} />
      <View style={styles.footer}>
        <Text style={styles.footerText}>
          Polled every {POLL_MS / 1000}s from the free API · tick {tick}
        </Text>
      </View>
    </ScrollView>
  );
}

// ─── Telemetry pipe ─────────────────────────────────────────

function TelemetryCard({
  stats,
  enabled,
  protocol,
}: {
  stats: TelemetryStats | null;
  enabled: boolean;
  protocol: ReturnType<typeof useProtocol>['protocol'];
}) {
  if (!enabled || !stats) {
    return (
      <MetricCard title="Telemetry" subtitle="off">
        <Text style={styles.empty}>
          Paste the portal key and app id into src/constants.ts to enable the SDK's telemetry.
          Nothing leaves the device until then.
        </Text>
      </MetricCard>
    );
  }
  const healthy = !stats.lastError;
  return (
    <MetricCard
      title="Telemetry"
      subtitle={stats.lastFlushAtMs ? `last accepted ${formatRelative(stats.lastFlushAtMs)}` : 'nothing accepted yet'}>
      <TileRow>
        <StatTile label="Sent" value={stats.sentEvents} hint="events in 2xx batches" />
        <StatTile label="Accepted" value={stats.acceptedEvents} hint="what the ingest metered" accent="#34C759" />
        <StatTile
          label="Dropped"
          value={stats.dropped}
          accent={stats.dropped > 0 ? '#FF9500' : undefined}
          hint="caps, expiry, rejection"
        />
      </TileRow>
      <KeyValueRow k="Buffered" v={stats.buffered} />
      <KeyValueRow k="Session" v={stats.sessionId.slice(0, 8)} />
      <KeyValueRow
        k="Status"
        v={healthy ? 'healthy' : stats.lastError ?? ''}
        accent={healthy ? '#34C759' : '#FF3B30'}
      />
      <View style={styles.actions}>
        <TouchableOpacity style={styles.action} onPress={() => protocol?.flushTelemetry().catch(() => {})}>
          <Text style={styles.actionText}>Flush now</Text>
        </TouchableOpacity>
        <TouchableOpacity
          style={styles.action}
          onPress={() => protocol?.endTelemetrySession().catch(() => {})}>
          <Text style={styles.actionText}>End session</Text>
        </TouchableOpacity>
      </View>
    </MetricCard>
  );
}

// ─── Mesh health ────────────────────────────────────────────

function MeshHealthCard({neighbors, currentTransport}: {neighbors: number; currentTransport: string | null}) {
  const connected = neighbors > 0;
  return (
    <MetricCard title="Mesh Health" subtitle={currentTransport ? `routing over ${transportLabel(currentTransport)}` : 'no transport selected'}>
      <TileRow>
        <StatTile label="Neighbors" value={neighbors} />
        <StatTile
          label="Reachability"
          value={connected ? 'CONNECTED' : 'PARTITIONED'}
          accent={connected ? '#34C759' : '#FF3B30'}
        />
      </TileRow>
    </MetricCard>
  );
}

// ─── Transports ─────────────────────────────────────────────

function TransportsCard({
  metrics,
  currentTransport,
}: {
  metrics: Map<TransportType, TransportMetrics>;
  currentTransport: string | null;
}) {
  if (metrics.size === 0) {
    return (
      <MetricCard title="Transports" subtitle="waiting for metrics">
        <Text style={styles.empty}>No transport is reporting metrics yet.</Text>
      </MetricCard>
    );
  }
  return (
    <MetricCard title="Transports" subtitle="getTransportMetrics()">
      {[...metrics.entries()].map(([transport, m]) => (
        <View key={transport} style={styles.transportBlock}>
          <View style={styles.transportHeader}>
            <TransportBadge transport={transport} />
            {currentTransport === transport && <Text style={styles.active}>ACTIVE</Text>}
          </View>
          <KeyValueRow k="Latency" v={`${m.avgLatencyMs} ms`} />
          <KeyValueRow k="Error rate" v={`${(m.errorRate * 100).toFixed(1)}%`} />
          {m.deliveryRatio !== undefined && (
            <KeyValueRow k="Delivery ratio" v={`${(m.deliveryRatio * 100).toFixed(0)}%`} />
          )}
          {m.congestion !== undefined && <KeyValueRow k="Congestion" v={m.congestion.toFixed(2)} />}
          {m.queueDepth !== undefined && <KeyValueRow k="Queue depth" v={m.queueDepth} />}
          {m.rssi !== undefined && <KeyValueRow k="RSSI" v={`${m.rssi} dBm`} />}
        </View>
      ))}
    </MetricCard>
  );
}

// ─── BLE ────────────────────────────────────────────────────

function BleCard({diagnostics}: {diagnostics: BleDiagnostics | null}) {
  if (!diagnostics) {
    return null;
  }
  return (
    <MetricCard title="BLE Diagnostics" subtitle="degraded-path counters, monotonic">
      <KeyValueRow k="Fragment fallbacks" v={diagnostics.fragmentFallbacks} />
      <KeyValueRow k="Recipient not among peers" v={diagnostics.recipientNotAmongPeers} />
      <KeyValueRow k="Undersized MTU reports" v={diagnostics.undersizedMtuReports} />
    </MetricCard>
  );
}

const styles = StyleSheet.create({
  scroll: {
    flex: 1,
    backgroundColor: '#F2F2F7',
  },
  content: {
    padding: 16,
    paddingBottom: 48,
    gap: 12,
  },
  empty: {
    fontSize: 13,
    color: '#8E8E93',
    lineHeight: 18,
  },
  actions: {
    flexDirection: 'row',
    gap: 8,
    marginTop: 8,
  },
  action: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 8,
    backgroundColor: '#E5E5EA',
  },
  actionText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  transportBlock: {
    marginTop: 8,
    gap: 2,
  },
  transportHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginBottom: 4,
  },
  active: {
    fontSize: 10,
    fontWeight: '800',
    color: '#34C759',
    letterSpacing: 0.5,
  },
  footer: {
    alignItems: 'center',
    paddingVertical: 12,
  },
  footerText: {
    fontSize: 11,
    color: '#8E8E93',
  },
});
