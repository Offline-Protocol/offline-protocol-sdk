import React, {useMemo} from 'react';
import {View, Text, ScrollView, StyleSheet} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {
  MetricCard,
  StatTile,
  TileRow,
  Bar,
  KeyValueRow,
  Sparkline,
  TransportBadge,
  SegmentedBar,
  VerticalHistogram,
  CardDivider,
} from '../components/TelemetryViz';
import {
  transportColor,
  transportLabel,
  reasonLabel,
  formatRelative,
} from '../telemetryFormat';
import type {
  MetricsFrame,
  TransportStateTelemetryEvent,
  RoutingDecision,
  DeviceCapabilitySnapshot,
  TransportType,
  RoutingReasonCode,
} from '@offline-protocol/mesh-sdk';

/**
 * Diagnostics screen.
 *
 * Visualizes only the ethical, aggregate, anonymized observations the SDK
 * emits: no peer IDs, no message content, no per-decision internals — just
 * the shape of the mesh from this device's vantage point. Every card is
 * either (a) directly wired to a SDK-emitted field, or (b) a pure derivation
 * over a telemetry buffer already populated by ProtocolContext.
 */
export function DiagnosticsScreen() {
  const {
    latestMetrics,
    metricsHistory,
    transportTimeline,
    routingDecisions,
    deviceCapability,
    deviceCapabilityHistory,
    hopCountHistogram,
  } = useProtocol();

  return (
    <ScrollView
      style={styles.scroll}
      contentContainerStyle={styles.content}
      showsVerticalScrollIndicator={false}>
      <MeshHealthCard metrics={latestMetrics} history={metricsHistory} />
      <TransportDistributionCard history={metricsHistory} />
      <DorsDriversCard decisions={routingDecisions} />
      <LinkStabilityCard timeline={transportTimeline} />
      <ReliabilityCard metrics={latestMetrics} />
      <HopDistributionCard histogram={hopCountHistogram} />
      <RetryQueueCard metrics={latestMetrics} />
      <PartitionCard history={metricsHistory} />
      <BatteryImpactCard
        latest={deviceCapability}
        history={deviceCapabilityHistory}
        relayActiveNow={latestMetrics?.isLocalRelay ?? false}
      />
      <View style={styles.footer}>
        <Text style={styles.footerText}>
          Aggregate, anonymized observations · live from TelemetrySink · 2s cadence
        </Text>
      </View>
    </ScrollView>
  );
}

// ─── Mesh Health ─────────────────────────────────────────────

function MeshHealthCard({
  metrics,
  history,
}: {
  metrics: MetricsFrame | null;
  history: MetricsFrame[];
}) {
  const neighborSeries = useMemo(
    () => history.map(f => f.neighborCount),
    [history],
  );

  if (!metrics) {
    return (
      <MetricCard title="Mesh Health" subtitle="waiting for first frame">
        <Text style={styles.empty}>No telemetry frames received yet.</Text>
      </MetricCard>
    );
  }

  const transport = metrics.currentTransport;
  const color = transport ? transportColor(transport) : '#8E8E93';
  const label = transport ? transportLabel(transport) : 'NONE';
  const connected = metrics.neighborCount > 0;

  return (
    <MetricCard title="Mesh Health" subtitle={formatRelative(metrics.timestampMs)}>
      <View style={styles.heroRow}>
        <View>
          <Text style={styles.heroKicker}>ACTIVE TRANSPORT</Text>
          <View style={styles.heroLine}>
            <View style={[styles.heroDot, {backgroundColor: color}]} />
            <Text style={[styles.heroValue, {color}]}>{label}</Text>
          </View>
        </View>
        <View style={styles.heroRight}>
          <Text style={styles.heroKicker}>REACHABILITY</Text>
          <Text
            style={[
              styles.heroReach,
              {color: connected ? '#34C759' : '#FF3B30'},
            ]}>
            {connected ? 'CONNECTED' : 'PARTITIONED'}
          </Text>
        </View>
      </View>
      <TileRow>
        <StatTile label="Neighbors" value={metrics.neighborCount} />
        <StatTile
          label="Role"
          value={metrics.isLocalRelay ? 'RELAY' : 'LEAF'}
          accent={metrics.isLocalRelay ? '#5856D6' : undefined}
        />
        <StatTile
          label="Degree"
          value={metrics.neighborCount}
          hint="local"
        />
      </TileRow>
      {neighborSeries.length > 1 && (
        <View style={styles.sparkWrap}>
          <Text style={styles.sparkLabel}>neighbor count · last {neighborSeries.length * 2}s</Text>
          <Sparkline values={neighborSeries} color={color} height={28} />
        </View>
      )}
    </MetricCard>
  );
}

// ─── Transport time distribution ─────────────────────────────

function TransportDistributionCard({history}: {history: MetricsFrame[]}) {
  const dist = useMemo(() => computeTransportDistribution(history), [history]);
  return (
    <MetricCard
      title="Transport Distribution"
      subtitle={dist.total > 0 ? `${dist.windowSec}s window` : 'no samples'}>
      {dist.total === 0 ? (
        <Text style={styles.empty}>
          Waiting for metrics frames. Each frame contributes its active transport
          to the distribution.
        </Text>
      ) : (
        <>
          <SegmentedBar
            segments={dist.entries.map(e => ({
              key: e.transport,
              value: e.count,
              color: transportColor(e.transport),
            }))}
          />
          <View style={styles.legendRow}>
            {dist.entries.map(e => (
              <View key={e.transport} style={styles.legendItem}>
                <View
                  style={[styles.legendDot, {backgroundColor: transportColor(e.transport)}]}
                />
                <Text style={styles.legendText}>
                  {transportLabel(e.transport)} · {Math.round((e.count / dist.total) * 100)}%
                </Text>
              </View>
            ))}
          </View>
        </>
      )}
    </MetricCard>
  );
}

interface TransportDistribution {
  entries: Array<{transport: TransportType; count: number}>;
  total: number;
  windowSec: number;
}

function computeTransportDistribution(history: MetricsFrame[]): TransportDistribution {
  const counts = new Map<TransportType, number>();
  let total = 0;
  let windowStart = history[0]?.timestampMs ?? 0;
  let windowEnd = history[0]?.timestampMs ?? 0;
  for (const frame of history) {
    windowEnd = frame.timestampMs;
    if (windowStart === 0) {windowStart = frame.timestampMs;}
    if (frame.currentTransport) {
      counts.set(
        frame.currentTransport,
        (counts.get(frame.currentTransport) ?? 0) + 1,
      );
      total += 1;
    }
  }
  const entries = Array.from(counts.entries())
    .map(([transport, count]) => ({transport, count}))
    .sort((a, b) => b.count - a.count);
  const windowSec = Math.max(0, Math.round((windowEnd - windowStart) / 1000));
  return {entries, total, windowSec};
}

// ─── DORS switch drivers ─────────────────────────────────────

function DorsDriversCard({decisions}: {decisions: RoutingDecision[]}) {
  const drivers = useMemo(() => computeDorsDrivers(decisions), [decisions]);
  const totalSwitches = drivers.reduce((s, d) => s + d.count, 0);
  const max = Math.max(1, ...drivers.map(d => d.count));
  return (
    <MetricCard
      title="DORS Switch Drivers"
      subtitle={totalSwitches > 0 ? `${totalSwitches} switches observed` : 'no switches yet'}>
      {totalSwitches === 0 ? (
        <Text style={styles.empty}>
          Transport switches appear here once DORS moves between transports.
          Reason codes tell you what triggered the switch — signal, congestion,
          unavailability, etc.
        </Text>
      ) : (
        drivers.map(d => (
          <Bar
            key={d.reason}
            label={reasonLabel(d.reason)}
            value={d.count}
            max={max}
            color="#5856D6"
            rightLabel={String(d.count)}
          />
        ))
      )}
    </MetricCard>
  );
}

function computeDorsDrivers(
  decisions: RoutingDecision[],
): Array<{reason: RoutingReasonCode; count: number}> {
  const counts = new Map<RoutingReasonCode, number>();
  for (const d of decisions) {
    if (d.phase !== 'switched' || !d.reasonCode) {continue;}
    counts.set(d.reasonCode, (counts.get(d.reasonCode) ?? 0) + 1);
  }
  return Array.from(counts.entries())
    .map(([reason, count]) => ({reason, count}))
    .sort((a, b) => b.count - a.count);
}

// ─── Link stability ──────────────────────────────────────────

function LinkStabilityCard({
  timeline,
}: {
  timeline: TransportStateTelemetryEvent[];
}) {
  const stability = useMemo(() => computeLinkStability(timeline), [timeline]);

  if (stability.length === 0) {
    return (
      <MetricCard title="Link Stability" subtitle="no transitions yet">
        <Text style={styles.empty}>
          Every connect/disconnect on a transport is recorded here. With no
          transitions observed, every link has been stable since startup.
        </Text>
      </MetricCard>
    );
  }

  return (
    <MetricCard title="Link Stability" subtitle={`${stability.length} transports observed`}>
      {stability.map(s => (
        <View key={s.transport} style={styles.linkRow}>
          <TransportBadge transport={s.transport} small />
          <View style={styles.linkMeta}>
            <Text style={styles.linkMetaValue}>{s.flaps} transitions</Text>
            <Text style={styles.linkMetaHint}>
              {s.avgGapMs > 0
                ? `avg gap ${formatDuration(s.avgGapMs)}`
                : 'single event'}
            </Text>
          </View>
        </View>
      ))}
    </MetricCard>
  );
}

interface LinkStabilityEntry {
  transport: TransportType;
  flaps: number;
  avgGapMs: number;
}

function computeLinkStability(
  timeline: TransportStateTelemetryEvent[],
): LinkStabilityEntry[] {
  const byTransport = new Map<TransportType, number[]>();
  for (const ev of timeline) {
    const arr = byTransport.get(ev.transport) ?? [];
    arr.push(ev.timestampMs);
    byTransport.set(ev.transport, arr);
  }
  return Array.from(byTransport.entries())
    .map(([transport, timestamps]) => {
      const sorted = [...timestamps].sort((a, b) => a - b);
      let totalGap = 0;
      for (let i = 1; i < sorted.length; i++) {
        totalGap += sorted[i] - sorted[i - 1];
      }
      const avgGapMs = sorted.length > 1 ? totalGap / (sorted.length - 1) : 0;
      return {transport, flaps: sorted.length, avgGapMs};
    })
    .sort((a, b) => b.flaps - a.flaps);
}

// ─── Reliability by transport ────────────────────────────────

function ReliabilityCard({metrics}: {metrics: MetricsFrame | null}) {
  if (!metrics || metrics.transports.length === 0) {
    return (
      <MetricCard title="Reliability by Transport" subtitle="no data">
        <Text style={styles.empty}>
          Per-transport delivery, error, and latency metrics will appear once a
          transport becomes active.
        </Text>
      </MetricCard>
    );
  }
  return (
    <MetricCard
      title="Reliability by Transport"
      subtitle={`${metrics.transports.length} transport${metrics.transports.length === 1 ? '' : 's'}`}>
      {metrics.transports.map((entry, i) => (
        <View key={entry.transport}>
          {i > 0 && <CardDivider />}
          <View style={styles.reliabilityHeader}>
            <TransportBadge transport={entry.transport} small />
          </View>
          <Bar
            label="Delivery"
            value={entry.metrics.deliveryRatio ?? 0}
            max={1}
            color="#34C759"
            rightLabel={
              entry.metrics.deliveryRatio !== undefined
                ? `${(entry.metrics.deliveryRatio * 100).toFixed(1)}%`
                : '—'
            }
          />
          <Bar
            label="Error rate"
            value={entry.metrics.errorRate}
            max={1}
            color={
              entry.metrics.errorRate >= 0.1
                ? '#FF3B30'
                : entry.metrics.errorRate >= 0.02
                ? '#FF9500'
                : '#34C759'
            }
            rightLabel={`${(entry.metrics.errorRate * 100).toFixed(2)}%`}
          />
          <Bar
            label="Avg latency"
            value={Math.min(entry.metrics.avgLatencyMs, 1000)}
            max={1000}
            color="#007AFF"
            rightLabel={`${Math.round(entry.metrics.avgLatencyMs)} ms`}
          />
          {entry.metrics.averageHopCount !== undefined && (
            <KeyValueRow
              k="Avg hop count"
              v={entry.metrics.averageHopCount.toFixed(2)}
            />
          )}
        </View>
      ))}
    </MetricCard>
  );
}

// ─── Hop count distribution ──────────────────────────────────

function HopDistributionCard({
  histogram,
}: {
  histogram: Record<number, number>;
}) {
  const {buckets, total, max} = useMemo(() => {
    const observed = Object.keys(histogram)
      .map(k => Number(k))
      .filter(n => Number.isFinite(n));
    const maxHop = observed.length > 0 ? Math.max(...observed) : 0;
    const upper = Math.min(Math.max(maxHop, 7), 15);
    const arr: Array<{key: string | number; value: number}> = [];
    let runningTotal = 0;
    for (let h = 0; h <= upper; h++) {
      const v = histogram[h] ?? 0;
      runningTotal += v;
      arr.push({key: h === 15 ? '15+' : h, value: v});
    }
    return {buckets: arr, total: runningTotal, max: upper};
  }, [histogram]);

  if (total === 0) {
    return (
      <MetricCard title="Hop Count Distribution" subtitle="no samples">
        <Text style={styles.empty}>
          Sampled from every received and delivered message's final hop count.
          Direct deliveries land at 0; mesh-routed deliveries climb from there.
        </Text>
      </MetricCard>
    );
  }

  return (
    <MetricCard
      title="Hop Count Distribution"
      subtitle={`${total} message${total === 1 ? '' : 's'} sampled · max observed ${max}`}>
      <VerticalHistogram buckets={buckets} color="#007AFF" height={70} />
    </MetricCard>
  );
}

// ─── Retry queue ─────────────────────────────────────────────

function RetryQueueCard({metrics}: {metrics: MetricsFrame | null}) {
  if (!metrics) {return null;}
  const r = metrics.retryQueue;
  const total = Math.max(
    1,
    r.criticalPriorityCount + r.highPriorityCount + r.mediumPriorityCount + r.lowPriorityCount,
  );
  const anyPending = total > 1 || r.totalCount > 0;
  return (
    <MetricCard
      title="Retry Queue"
      subtitle={
        anyPending
          ? `${r.totalCount} pending · ${r.readyCount} ready`
          : 'queue empty'
      }>
      {anyPending ? (
        <>
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
        </>
      ) : (
        <Text style={styles.empty}>
          No messages are waiting for retry. Queue fills under intermittent
          connectivity or ACK timeouts.
        </Text>
      )}
    </MetricCard>
  );
}

// ─── Partition & recovery ────────────────────────────────────

function PartitionCard({history}: {history: MetricsFrame[]}) {
  const partition = useMemo(() => computePartitionStats(history), [history]);

  return (
    <MetricCard
      title="Partition & Recovery"
      subtitle={
        partition.observed === 0
          ? 'no partitions observed'
          : `${partition.observed} resolved · ${formatDuration(partition.avgDurationMs)} avg`
      }>
      <TileRow>
        <StatTile
          label="State"
          value={partition.currentMs !== null ? 'PARTITIONED' : 'CONNECTED'}
          accent={partition.currentMs !== null ? '#FF3B30' : '#34C759'}
        />
        <StatTile
          label="Current"
          value={partition.currentMs !== null ? formatDuration(partition.currentMs) : '—'}
          hint={partition.currentMs !== null ? 'in progress' : ''}
        />
        <StatTile label="Resolved" value={partition.observed} hint="in window" />
      </TileRow>
      {partition.observed > 0 && (
        <View style={{marginTop: 6}}>
          <KeyValueRow k="Avg duration" v={formatDuration(partition.avgDurationMs)} />
          <KeyValueRow k="Longest" v={formatDuration(partition.maxDurationMs)} />
          <KeyValueRow
            k="Last ended"
            v={
              partition.lastEndedMs !== null
                ? formatRelative(partition.lastEndedMs)
                : '—'
            }
          />
        </View>
      )}
    </MetricCard>
  );
}

interface PartitionStats {
  observed: number;
  avgDurationMs: number;
  maxDurationMs: number;
  currentMs: number | null;
  lastEndedMs: number | null;
}

function computePartitionStats(history: MetricsFrame[]): PartitionStats {
  const completed: Array<{startMs: number; endMs: number}> = [];
  let partitionStart: number | null = null;
  let lastEndedMs: number | null = null;
  for (const frame of history) {
    if (frame.neighborCount === 0 && partitionStart === null) {
      partitionStart = frame.timestampMs;
    } else if (frame.neighborCount > 0 && partitionStart !== null) {
      completed.push({startMs: partitionStart, endMs: frame.timestampMs});
      lastEndedMs = frame.timestampMs;
      partitionStart = null;
    }
  }
  const currentMs =
    partitionStart !== null ? Math.max(0, Date.now() - partitionStart) : null;
  const durations = completed.map(p => p.endMs - p.startMs);
  const avgDurationMs = durations.length
    ? durations.reduce((s, d) => s + d, 0) / durations.length
    : 0;
  const maxDurationMs = durations.length ? Math.max(...durations) : 0;
  return {
    observed: completed.length,
    avgDurationMs,
    maxDurationMs,
    currentMs,
    lastEndedMs,
  };
}

// ─── Battery impact ──────────────────────────────────────────

function BatteryImpactCard({
  latest,
  history,
  relayActiveNow,
}: {
  latest: DeviceCapabilitySnapshot | null;
  history: DeviceCapabilitySnapshot[];
  relayActiveNow: boolean;
}) {
  const drain = useMemo(() => computeBatteryDrain(history), [history]);

  if (!latest || latest.batteryLevel === undefined || latest.batteryLevel === null) {
    return (
      <MetricCard title="Battery Impact" subtitle="no samples">
        <Text style={styles.empty}>
          Device capability snapshots are emitted when battery level, charging
          state, or relay role changes. Nothing to plot yet.
        </Text>
      </MetricCard>
    );
  }

  const level = latest.batteryLevel;
  const batteryColor = latest.isCharging
    ? '#34C759'
    : level <= 20
    ? '#FF3B30'
    : level <= 40
    ? '#FF9500'
    : '#34C759';

  const levelSeries = history
    .map(s => s.batteryLevel)
    .filter((n): n is number => typeof n === 'number');

  return (
    <MetricCard title="Battery Impact" subtitle={formatRelative(latest.timestampMs)}>
      <Bar
        label="Level"
        value={level}
        max={100}
        color={batteryColor}
        rightLabel={`${level}%${latest.isCharging ? ' ⚡' : ''}`}
      />
      <TileRow>
        <StatTile
          label="Drain"
          value={drain ? `${drain.ratePerHour.toFixed(1)}%/h` : '—'}
          hint={drain ? `${drain.samples} samples` : 'need ≥2 samples'}
          accent={drain && drain.ratePerHour > 10 ? '#FF3B30' : undefined}
        />
        <StatTile
          label="Role"
          value={latest.relayRole === 'relay' ? 'RELAY' : 'LEAF'}
          accent={latest.relayRole === 'relay' ? '#5856D6' : undefined}
        />
        <StatTile
          label="Relay active"
          value={relayActiveNow ? 'YES' : 'NO'}
          accent={relayActiveNow ? '#5856D6' : undefined}
        />
      </TileRow>
      {levelSeries.length > 1 && (
        <View style={styles.sparkWrap}>
          <Text style={styles.sparkLabel}>
            battery level · last {levelSeries.length} samples
          </Text>
          <Sparkline values={levelSeries} color={batteryColor} height={28} />
        </View>
      )}
    </MetricCard>
  );
}

interface BatteryDrain {
  ratePerHour: number;
  samples: number;
}

function computeBatteryDrain(history: DeviceCapabilitySnapshot[]): BatteryDrain | null {
  const levels = history.filter(
    (s): s is DeviceCapabilitySnapshot & {batteryLevel: number} =>
      typeof s.batteryLevel === 'number' && !s.isCharging,
  );
  if (levels.length < 2) {return null;}
  const first = levels[0];
  const last = levels[levels.length - 1];
  const dMs = last.timestampMs - first.timestampMs;
  if (dMs <= 0) {return null;}
  const dLevel = first.batteryLevel - last.batteryLevel; // positive = draining
  const hours = dMs / 3_600_000;
  return {ratePerHour: dLevel / hours, samples: levels.length};
}

// ─── Formatters ──────────────────────────────────────────────

function formatDuration(ms: number): string {
  if (ms < 1000) {return `${Math.round(ms)} ms`;}
  if (ms < 60_000) {return `${(ms / 1000).toFixed(1)} s`;}
  if (ms < 3_600_000) {return `${Math.floor(ms / 60_000)} min`;}
  return `${(ms / 3_600_000).toFixed(1)} h`;
}

// ─── Styles ──────────────────────────────────────────────────

const styles = StyleSheet.create({
  scroll: {
    flex: 1,
    backgroundColor: '#F2F2F7',
  },
  content: {
    paddingBottom: 24,
  },
  empty: {
    fontSize: 12,
    color: '#8E8E93',
    fontStyle: 'italic',
    paddingVertical: 4,
    lineHeight: 16,
  },
  heroRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'flex-start',
    marginBottom: 10,
  },
  heroRight: {
    alignItems: 'flex-end',
  },
  heroKicker: {
    fontSize: 10,
    fontWeight: '700',
    color: '#8E8E93',
    letterSpacing: 0.6,
  },
  heroLine: {
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
    fontSize: 22,
    fontWeight: '800',
    letterSpacing: 0.4,
  },
  heroReach: {
    fontSize: 16,
    fontWeight: '800',
    marginTop: 4,
    letterSpacing: 0.5,
  },
  sparkWrap: {
    marginTop: 10,
  },
  sparkLabel: {
    fontSize: 10,
    color: '#8E8E93',
    marginBottom: 4,
  },
  legendRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 10,
    marginTop: 8,
  },
  legendItem: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  legendDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  legendText: {
    fontSize: 11,
    color: '#3C3C43',
    fontWeight: '600',
  },
  linkRow: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 6,
    gap: 10,
  },
  linkMeta: {
    flex: 1,
    alignItems: 'flex-end',
  },
  linkMetaValue: {
    fontSize: 12,
    color: '#1C1C1E',
    fontWeight: '700',
    fontVariant: ['tabular-nums'],
  },
  linkMetaHint: {
    fontSize: 10,
    color: '#8E8E93',
    marginTop: 2,
  },
  reliabilityHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 4,
    marginTop: 2,
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
