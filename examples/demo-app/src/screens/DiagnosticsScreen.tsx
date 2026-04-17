import React, {useMemo, useState} from 'react';
import {View, Text, ScrollView, StyleSheet, TouchableOpacity} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {
  MetricCard,
  StatTile,
  TileRow,
  Bar,
  KeyValueRow,
  Sparkline,
  TransportBadge,
  RoutingScoreBars,
  CardDivider,
} from '../components/TelemetryViz';
import {
  transportColor,
  transportLabel,
  transportStatusColor,
  routingPhaseColor,
  reasonLabel,
  formatBytes,
  formatCount,
  formatPercent,
  formatRelative,
} from '../telemetryFormat';
import type {RoutingDecision, MetricsFrame} from '@offline-protocol/mesh-sdk';

export function DiagnosticsScreen() {
  const {
    latestMetrics,
    metricsHistory,
    transportTimeline,
    routingDecisions,
    deviceCapability,
    mlsLog,
    eventCounts,
    totalProtocolEvents,
  } = useProtocol();

  return (
    <ScrollView
      style={styles.scroll}
      contentContainerStyle={styles.content}
      showsVerticalScrollIndicator={false}>
      <HeroCard metrics={latestMetrics} history={metricsHistory} />
      <NetworkAtAGlance metrics={latestMetrics} />
      <DeviceCapabilityCard
        snapshot={deviceCapability}
        isLocalRelay={latestMetrics?.isLocalRelay ?? false}
      />
      <PerTransportCard metrics={latestMetrics} history={metricsHistory} />
      <RetryDedupCard metrics={latestMetrics} />
      <RoutingDecisionsCard decisions={routingDecisions} />
      <TransportTimelineCard timeline={transportTimeline} />
      <MlsLogCard log={mlsLog} />
      <EventCountersCard counts={eventCounts} total={totalProtocolEvents} />
      <View style={styles.footer}>
        <Text style={styles.footerText}>
          Live stream from TelemetrySink · push delivery · 2s metrics cadence
        </Text>
      </View>
    </ScrollView>
  );
}

// ─── Hero ────────────────────────────────────────────────────

function HeroCard({
  metrics,
  history,
}: {
  metrics: MetricsFrame | null;
  history: MetricsFrame[];
}) {
  if (!metrics) {
    return (
      <View style={styles.hero}>
        <Text style={styles.heroEmpty}>Waiting for first telemetry frame…</Text>
      </View>
    );
  }
  const transport = metrics.currentTransport;
  const color = transport ? transportColor(transport) : '#8E8E93';
  const label = transport ? transportLabel(transport) : 'NONE';

  // Aggregate packets-per-frame across all transports for a hero sparkline
  const series = useMemo(() => {
    if (history.length < 2) {return [];}
    const totals = history.map(f =>
      f.transports.reduce((s, t) => s + t.metrics.packetsSent + t.metrics.packetsReceived, 0),
    );
    const deltas: number[] = [];
    for (let i = 1; i < totals.length; i++) {
      deltas.push(Math.max(0, totals[i] - totals[i - 1]));
    }
    return deltas;
  }, [history]);

  return (
    <View style={[styles.hero, {borderLeftColor: color}]}>
      <View style={styles.heroTop}>
        <View>
          <Text style={styles.heroLabel}>ACTIVE TRANSPORT</Text>
          <View style={styles.heroRow}>
            <View style={[styles.heroDot, {backgroundColor: color}]} />
            <Text style={[styles.heroValue, {color}]}>{label}</Text>
          </View>
        </View>
        <View>
          <Text style={styles.heroLabel}>UPDATED</Text>
          <Text style={styles.heroSecondary}>{formatRelative(metrics.timestampMs)}</Text>
        </View>
      </View>
      {series.length > 0 && (
        <View style={styles.heroSpark}>
          <Sparkline values={series} color={color} height={36} />
          <Text style={styles.heroSparkLabel}>packets / 2s · last {series.length} frames</Text>
        </View>
      )}
    </View>
  );
}

// ─── Network at a glance ─────────────────────────────────────

function NetworkAtAGlance({metrics}: {metrics: MetricsFrame | null}) {
  if (!metrics) {return null;}
  const dedupPct = Math.round(metrics.dedup.capacityUsedPercent);
  return (
    <MetricCard title="Network at a Glance">
      <TileRow>
        <StatTile
          label="Neighbors"
          value={metrics.neighborCount}
          accent={metrics.neighborCount > 0 ? '#34C759' : undefined}
        />
        <StatTile
          label="ACK Pend"
          value={metrics.ackPending}
          accent={metrics.ackPending > 10 ? '#FF9500' : undefined}
        />
        <StatTile
          label="Retry Q"
          value={metrics.retryQueue.totalCount}
          hint={`${metrics.retryQueue.readyCount} ready`}
          accent={metrics.retryQueue.totalCount > 0 ? '#FF9500' : undefined}
        />
        <StatTile
          label="Dedup"
          value={`${dedupPct}%`}
          hint={metrics.dedup.mode}
          accent={dedupPct >= 80 ? '#FF3B30' : undefined}
        />
      </TileRow>
    </MetricCard>
  );
}

// ─── Device capability ───────────────────────────────────────

function DeviceCapabilityCard({
  snapshot,
  isLocalRelay,
}: {
  snapshot: ReturnType<typeof useProtocol>['deviceCapability'];
  isLocalRelay: boolean;
}) {
  if (!snapshot) {return null;}
  const battery = snapshot.batteryLevel;
  const batteryColor =
    snapshot.isCharging
      ? '#34C759'
      : battery !== undefined && battery !== null && battery <= 20
      ? '#FF3B30'
      : battery !== undefined && battery !== null && battery <= 40
      ? '#FF9500'
      : '#34C759';

  return (
    <MetricCard title="Device" subtitle={formatRelative(snapshot.timestampMs)}>
      {battery !== undefined && battery !== null ? (
        <Bar
          label="Battery"
          value={battery}
          max={100}
          color={batteryColor}
          rightLabel={`${battery}%${snapshot.isCharging ? ' ⚡' : ''}`}
        />
      ) : (
        <KeyValueRow k="Battery" v="unknown" />
      )}
      <View style={styles.chipRow}>
        <View
          style={[
            styles.chip,
            {backgroundColor: snapshot.relayRole === 'relay' ? '#5856D6' : '#E5E5EA'},
          ]}>
          <Text
            style={[
              styles.chipText,
              {color: snapshot.relayRole === 'relay' ? '#FFFFFF' : '#3C3C43'},
            ]}>
            {snapshot.relayRole === 'relay' ? 'RELAY ROLE' : 'REGULAR ROLE'}
          </Text>
        </View>
        {isLocalRelay && (
          <View style={[styles.chip, {backgroundColor: '#34C759'}]}>
            <Text style={[styles.chipText, {color: '#FFFFFF'}]}>ACTIVE RELAY</Text>
          </View>
        )}
        <View
          style={[
            styles.chip,
            {backgroundColor: snapshot.isCharging ? '#34C759' : '#E5E5EA'},
          ]}>
          <Text
            style={[
              styles.chipText,
              {color: snapshot.isCharging ? '#FFFFFF' : '#3C3C43'},
            ]}>
            {snapshot.isCharging ? 'CHARGING' : 'ON BATTERY'}
          </Text>
        </View>
      </View>
      {snapshot.changedFields !== 0 && (
        <Text style={styles.changedHint}>
          changed: {decodeChangedFields(snapshot.changedFields)}
        </Text>
      )}
    </MetricCard>
  );
}

function decodeChangedFields(mask: number): string {
  const out: string[] = [];
  if (mask & 0x01) {out.push('battery');}
  if (mask & 0x02) {out.push('charging');}
  if (mask & 0x04) {out.push('relay-role');}
  return out.join(', ') || 'none';
}

// ─── Per-transport metrics ───────────────────────────────────

function PerTransportCard({
  metrics,
  history,
}: {
  metrics: MetricsFrame | null;
  history: MetricsFrame[];
}) {
  if (!metrics || metrics.transports.length === 0) {return null;}
  return (
    <MetricCard
      title="Per-Transport Metrics"
      subtitle={`${metrics.transports.length} transport${metrics.transports.length === 1 ? '' : 's'}`}>
      {metrics.transports.map((entry, i) => (
        <View key={entry.transport}>
          {i > 0 && <CardDivider />}
          <TransportRow entry={entry} history={history} />
        </View>
      ))}
    </MetricCard>
  );
}

function TransportRow({
  entry,
  history,
}: {
  entry: MetricsFrame['transports'][number];
  history: MetricsFrame[];
}) {
  const m = entry.metrics;
  const isCurrent = false; // visual emphasis is via the badge color already
  void isCurrent;

  // Per-transport packet sparkline (last N frames)
  const packetSeries = useMemo(() => {
    if (history.length < 2) {return [];}
    const totals = history.map(f => {
      const t = f.transports.find(x => x.transport === entry.transport);
      return t ? t.metrics.packetsSent + t.metrics.packetsReceived : 0;
    });
    const deltas: number[] = [];
    for (let i = 1; i < totals.length; i++) {
      deltas.push(Math.max(0, totals[i] - totals[i - 1]));
    }
    return deltas;
  }, [history, entry.transport]);

  const errorPct = m.errorRate * 100;
  const errorColor = errorPct >= 10 ? '#FF3B30' : errorPct >= 2 ? '#FF9500' : '#34C759';

  return (
    <View>
      <View style={styles.transportHeader}>
        <TransportBadge transport={entry.transport} />
        <Sparkline
          values={packetSeries}
          color={transportColor(entry.transport)}
          height={20}
        />
      </View>

      <View style={styles.kvGrid}>
        <View style={styles.kvCol}>
          <KeyValueRow k="Sent" v={`${formatCount(m.packetsSent)} pkt`} />
          <KeyValueRow k="Bytes ↑" v={formatBytes(m.bytesSent)} />
          <KeyValueRow k="Latency" v={`${m.avgLatencyMs.toFixed(0)} ms`} />
          {m.bandwidthBps !== undefined && (
            <KeyValueRow k="Bandwidth" v={`${formatBytes(m.bandwidthBps)}/s`} />
          )}
          {m.queueDepth !== undefined && (
            <KeyValueRow k="Queue" v={m.queueDepth} />
          )}
        </View>
        <View style={styles.kvCol}>
          <KeyValueRow k="Recv" v={`${formatCount(m.packetsReceived)} pkt`} />
          <KeyValueRow k="Bytes ↓" v={formatBytes(m.bytesReceived)} />
          <KeyValueRow
            k="Errors"
            v={`${errorPct.toFixed(1)}%`}
            accent={errorColor}
          />
          {m.deliveryRatio !== undefined && (
            <KeyValueRow k="Delivery" v={formatPercent(m.deliveryRatio, 1)} />
          )}
          {m.averageHopCount !== undefined && (
            <KeyValueRow k="Hops" v={m.averageHopCount.toFixed(1)} />
          )}
        </View>
      </View>

      {m.rssi !== undefined && (
        <Bar
          label="RSSI"
          value={Math.max(0, m.rssi + 100)}
          max={70}
          color="#007AFF"
          rightLabel={`${m.rssi} dBm`}
        />
      )}
      {m.congestion !== undefined && (
        <Bar
          label="Congestion"
          value={m.congestion}
          max={1}
          color={m.congestion > 0.7 ? '#FF3B30' : m.congestion > 0.4 ? '#FF9500' : '#34C759'}
          rightLabel={formatPercent(m.congestion, 0)}
        />
      )}
      {m.energyCost !== undefined && (
        <Bar
          label="Energy"
          value={m.energyCost}
          max={1}
          color="#FFCC00"
          rightLabel={m.energyCost.toFixed(2)}
        />
      )}
    </View>
  );
}

// ─── Retry queue + dedup ─────────────────────────────────────

function RetryDedupCard({metrics}: {metrics: MetricsFrame | null}) {
  if (!metrics) {return null;}
  const r = metrics.retryQueue;
  const total = Math.max(
    1,
    r.criticalPriorityCount + r.highPriorityCount + r.mediumPriorityCount + r.lowPriorityCount,
  );
  return (
    <MetricCard title="Reliability" subtitle="retry queue · deduplicator">
      <Text style={styles.subhead}>RETRY QUEUE BY PRIORITY</Text>
      <Bar
        label="Critical"
        value={r.criticalPriorityCount}
        max={total}
        color="#FF3B30"
        rightLabel={String(r.criticalPriorityCount)}
      />
      <Bar
        label="High"
        value={r.highPriorityCount}
        max={total}
        color="#FF9500"
        rightLabel={String(r.highPriorityCount)}
      />
      <Bar
        label="Medium"
        value={r.mediumPriorityCount}
        max={total}
        color="#007AFF"
        rightLabel={String(r.mediumPriorityCount)}
      />
      <Bar
        label="Low"
        value={r.lowPriorityCount}
        max={total}
        color="#8E8E93"
        rightLabel={String(r.lowPriorityCount)}
      />
      <CardDivider />
      <Text style={styles.subhead}>DEDUPLICATOR</Text>
      <KeyValueRow k="Mode" v={metrics.dedup.mode} />
      <KeyValueRow k="Tracked" v={formatCount(metrics.dedup.totalTracked)} />
      <KeyValueRow k="Recent" v={formatCount(metrics.dedup.recentTracked)} />
      <Bar
        label="Capacity"
        value={metrics.dedup.capacityUsedPercent}
        max={100}
        color={
          metrics.dedup.capacityUsedPercent >= 80
            ? '#FF3B30'
            : metrics.dedup.capacityUsedPercent >= 50
            ? '#FF9500'
            : '#34C759'
        }
        rightLabel={`${metrics.dedup.capacityUsedPercent.toFixed(0)}%`}
      />
      {metrics.dedup.falsePositiveRate !== undefined && (
        <KeyValueRow
          k="False-pos rate"
          v={`${(metrics.dedup.falsePositiveRate * 100).toFixed(3)}%`}
        />
      )}
    </MetricCard>
  );
}

// ─── DORS decisions ──────────────────────────────────────────

function RoutingDecisionsCard({decisions}: {decisions: RoutingDecision[]}) {
  const [expanded, setExpanded] = useState<number | null>(null);
  return (
    <MetricCard
      title="DORS Decisions"
      subtitle={decisions.length > 0 ? `${decisions.length} recent` : 'no decisions yet'}>
      {decisions.length === 0 && (
        <Text style={styles.empty}>
          No routing decisions emitted yet. They appear when DORS scores or switches transports.
        </Text>
      )}
      {decisions.slice(0, 8).map((d, i) => {
        const isOpen = expanded === i;
        return (
          <View key={i} style={styles.decisionRow}>
            <TouchableOpacity
              activeOpacity={0.7}
              onPress={() => setExpanded(isOpen ? null : i)}>
              <View style={styles.decisionHeader}>
                <View
                  style={[
                    styles.phaseBadge,
                    {backgroundColor: routingPhaseColor(d.phase)},
                  ]}>
                  <Text style={styles.phaseText}>{d.phase}</Text>
                </View>
                <Text style={styles.reason}>{reasonLabel(d.reasonCode)}</Text>
                <Text style={styles.decisionTime}>{formatRelative(d.timestampMs)}</Text>
              </View>
              <View style={styles.decisionBody}>
                {d.from && (
                  <>
                    <TransportBadge transport={d.from} small />
                    <Text style={styles.arrow}>→</Text>
                  </>
                )}
                {d.to ? (
                  <TransportBadge transport={d.to} small />
                ) : (
                  <Text style={styles.noTo}>—</Text>
                )}
                {d.winningScore !== undefined && (
                  <Text style={styles.score}>score {d.winningScore.toFixed(2)}</Text>
                )}
                {d.scores.length > 0 && (
                  <Text style={styles.expandHint}>{isOpen ? '▾' : '▸'} scores</Text>
                )}
              </View>
            </TouchableOpacity>
            {isOpen && d.scores.length > 0 && (
              <View>
                {d.scores.map(s => (
                  <RoutingScoreBars key={s.transport} entry={s} />
                ))}
              </View>
            )}
          </View>
        );
      })}
    </MetricCard>
  );
}

// ─── Transport state timeline ────────────────────────────────

function TransportTimelineCard({
  timeline,
}: {
  timeline: ReturnType<typeof useProtocol>['transportTimeline'];
}) {
  return (
    <MetricCard
      title="Transport State Timeline"
      subtitle={timeline.length > 0 ? `${timeline.length} transitions` : 'no transitions'}>
      {timeline.length === 0 && (
        <Text style={styles.empty}>
          Transitions appear when a transport's connection status changes.
        </Text>
      )}
      {timeline.slice(0, 12).map((ev, i) => (
        <View key={`${ev.timestampMs}-${i}`} style={styles.timelineRow}>
          <View
            style={[
              styles.timelineDot,
              {backgroundColor: transportStatusColor(ev.current)},
            ]}
          />
          <View style={styles.timelineBody}>
            <View style={styles.timelineTopRow}>
              <TransportBadge transport={ev.transport} small />
              <Text style={styles.timelineTime}>{formatRelative(ev.timestampMs)}</Text>
            </View>
            <View style={styles.timelineTransition}>
              <Text style={[styles.statusText, {color: transportStatusColor(ev.previous)}]}>
                {ev.previous}
              </Text>
              <Text style={styles.arrow}>→</Text>
              <Text style={[styles.statusText, {color: transportStatusColor(ev.current)}]}>
                {ev.current}
              </Text>
            </View>
          </View>
        </View>
      ))}
    </MetricCard>
  );
}

// ─── MLS lifecycle log ───────────────────────────────────────

const MLS_TYPE_COLORS: Record<string, string> = {
  initialized: '#34C759',
  encryption_used: '#007AFF',
  session_ready: '#34C759',
  session_missing: '#FF9500',
  decryption_failed: '#FF3B30',
};

function MlsLogCard({log}: {log: ReturnType<typeof useProtocol>['mlsLog']}) {
  return (
    <MetricCard
      title="MLS Lifecycle"
      subtitle={log.length > 0 ? `${log.length} events` : 'no MLS activity yet'}>
      {log.length === 0 && (
        <Text style={styles.empty}>
          MLS lifecycle events appear when a session initializes, encrypts, or fails.
        </Text>
      )}
      {log.slice(0, 10).map((entry, i) => {
        const color = MLS_TYPE_COLORS[entry.type] ?? '#8E8E93';
        return (
          <View key={i} style={styles.mlsRow}>
            <View style={[styles.mlsDot, {backgroundColor: color}]} />
            <View style={styles.mlsBody}>
              <View style={styles.mlsTopRow}>
                <Text style={[styles.mlsType, {color}]}>{entry.type}</Text>
                <Text style={styles.mlsTime}>{formatRelative(entry.ts)}</Text>
              </View>
              {renderMlsDetail(entry.raw)}
            </View>
          </View>
        );
      })}
    </MetricCard>
  );
}

function renderMlsDetail(raw: any): React.ReactNode {
  if (!raw || typeof raw !== 'object') {return null;}
  const interesting = ['groupId', 'group_id', 'peerId', 'peer_id', 'ciphersuite', 'reason', 'epoch'];
  const lines: string[] = [];
  for (const k of interesting) {
    if (raw[k] !== undefined && raw[k] !== null) {
      lines.push(`${k}=${String(raw[k])}`);
    }
  }
  if (lines.length === 0) {return null;}
  return <Text style={styles.mlsDetail}>{lines.join(' · ')}</Text>;
}

// ─── Event counters ──────────────────────────────────────────

function EventCountersCard({
  counts,
  total,
}: {
  counts: Record<string, number>;
  total: number;
}) {
  const sorted = useMemo(
    () =>
      Object.entries(counts)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 12),
    [counts],
  );
  if (sorted.length === 0) {
    return (
      <MetricCard title="Protocol Event Stream" subtitle="0 total">
        <Text style={styles.empty}>No protocol events received yet.</Text>
      </MetricCard>
    );
  }
  const max = Math.max(...sorted.map(([, n]) => n));
  return (
    <MetricCard
      title="Protocol Event Stream"
      subtitle={`${formatCount(total)} total · top ${sorted.length}`}>
      {sorted.map(([type, n]) => (
        <Bar
          key={type}
          label={shortenEventType(type)}
          value={n}
          max={max}
          color="#007AFF"
          rightLabel={String(n)}
        />
      ))}
    </MetricCard>
  );
}

function shortenEventType(t: string): string {
  // Strip any common prefixes for readability while keeping uniqueness
  return t.replace(/^protocol_/, '').replace(/_/g, ' ');
}

const styles = StyleSheet.create({
  scroll: {
    flex: 1,
    backgroundColor: '#F2F2F7',
  },
  content: {
    paddingBottom: 24,
  },
  hero: {
    backgroundColor: '#FFFFFF',
    marginHorizontal: 12,
    marginTop: 12,
    borderRadius: 12,
    padding: 14,
    borderLeftWidth: 4,
    borderLeftColor: '#C7C7CC',
  },
  heroEmpty: {
    color: '#8E8E93',
    fontStyle: 'italic',
    textAlign: 'center',
    paddingVertical: 16,
  },
  heroTop: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'flex-end',
  },
  heroLabel: {
    fontSize: 10,
    fontWeight: '700',
    color: '#8E8E93',
    letterSpacing: 0.6,
  },
  heroRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginTop: 4,
  },
  heroDot: {
    width: 10,
    height: 10,
    borderRadius: 5,
  },
  heroValue: {
    fontSize: 28,
    fontWeight: '800',
    letterSpacing: 0.5,
  },
  heroSecondary: {
    fontSize: 14,
    color: '#1C1C1E',
    fontWeight: '600',
    marginTop: 4,
    fontVariant: ['tabular-nums'],
  },
  heroSpark: {
    marginTop: 12,
  },
  heroSparkLabel: {
    fontSize: 10,
    color: '#8E8E93',
    marginTop: 4,
    textAlign: 'right',
  },
  chipRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 6,
    marginTop: 10,
  },
  chip: {
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 6,
  },
  chipText: {
    fontSize: 10,
    fontWeight: '700',
    letterSpacing: 0.4,
  },
  changedHint: {
    fontSize: 10,
    color: '#8E8E93',
    marginTop: 6,
    fontStyle: 'italic',
  },
  transportHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    marginBottom: 6,
  },
  kvGrid: {
    flexDirection: 'row',
    gap: 16,
    marginVertical: 4,
  },
  kvCol: {
    flex: 1,
  },
  subhead: {
    fontSize: 10,
    fontWeight: '700',
    color: '#8E8E93',
    letterSpacing: 0.6,
    marginBottom: 4,
  },
  empty: {
    fontSize: 12,
    color: '#8E8E93',
    fontStyle: 'italic',
    paddingVertical: 4,
  },
  decisionRow: {
    paddingVertical: 8,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5EA',
  },
  decisionHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  phaseBadge: {
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 4,
  },
  phaseText: {
    fontSize: 10,
    fontWeight: '700',
    color: '#FFFFFF',
    letterSpacing: 0.4,
  },
  reason: {
    fontSize: 11,
    color: '#3C3C43',
    fontWeight: '600',
    flex: 1,
  },
  decisionTime: {
    fontSize: 10,
    color: '#8E8E93',
    fontVariant: ['tabular-nums'],
  },
  decisionBody: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginTop: 6,
  },
  arrow: {
    fontSize: 14,
    color: '#8E8E93',
    fontWeight: '700',
  },
  noTo: {
    fontSize: 12,
    color: '#8E8E93',
  },
  score: {
    fontSize: 11,
    color: '#1C1C1E',
    fontWeight: '600',
    fontVariant: ['tabular-nums'],
    marginLeft: 'auto',
  },
  expandHint: {
    fontSize: 10,
    color: '#007AFF',
    fontWeight: '600',
  },
  timelineRow: {
    flexDirection: 'row',
    paddingVertical: 8,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5EA',
    gap: 10,
  },
  timelineDot: {
    width: 10,
    height: 10,
    borderRadius: 5,
    marginTop: 4,
  },
  timelineBody: {
    flex: 1,
    gap: 4,
  },
  timelineTopRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  timelineTime: {
    fontSize: 10,
    color: '#8E8E93',
    fontVariant: ['tabular-nums'],
  },
  timelineTransition: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  statusText: {
    fontSize: 11,
    fontWeight: '600',
  },
  mlsRow: {
    flexDirection: 'row',
    paddingVertical: 8,
    gap: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5EA',
  },
  mlsDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    marginTop: 6,
  },
  mlsBody: {
    flex: 1,
  },
  mlsTopRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  mlsType: {
    fontSize: 12,
    fontWeight: '700',
    letterSpacing: 0.3,
  },
  mlsTime: {
    fontSize: 10,
    color: '#8E8E93',
    fontVariant: ['tabular-nums'],
  },
  mlsDetail: {
    fontSize: 11,
    color: '#3C3C43',
    marginTop: 2,
    fontFamily: 'Courier',
  },
  footer: {
    paddingVertical: 16,
    alignItems: 'center',
  },
  footerText: {
    fontSize: 10,
    color: '#8E8E93',
    fontStyle: 'italic',
  },
});
