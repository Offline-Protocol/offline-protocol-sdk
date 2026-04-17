import React from 'react';
import {View, Text, StyleSheet} from 'react-native';
import {transportColor, transportLabel} from '../telemetryFormat';

// ─── MetricCard — section frame ──────────────────────────────

export function MetricCard({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string;
  children: React.ReactNode;
}) {
  return (
    <View style={cardStyles.card}>
      <View style={cardStyles.header}>
        <Text style={cardStyles.title}>{title}</Text>
        {subtitle && <Text style={cardStyles.subtitle}>{subtitle}</Text>}
      </View>
      <View style={cardStyles.body}>{children}</View>
    </View>
  );
}

const cardStyles = StyleSheet.create({
  card: {
    backgroundColor: '#FFFFFF',
    borderRadius: 12,
    marginHorizontal: 12,
    marginTop: 12,
    overflow: 'hidden',
  },
  header: {
    flexDirection: 'row',
    alignItems: 'baseline',
    justifyContent: 'space-between',
    paddingHorizontal: 14,
    paddingTop: 12,
    paddingBottom: 6,
  },
  title: {
    fontSize: 14,
    fontWeight: '700',
    color: '#1C1C1E',
    letterSpacing: 0.2,
  },
  subtitle: {
    fontSize: 11,
    color: '#8E8E93',
    fontWeight: '500',
  },
  body: {
    paddingHorizontal: 14,
    paddingBottom: 12,
  },
});

// ─── StatTile — small KPI tile in a row of 4 ─────────────────

export function StatTile({
  label,
  value,
  accent,
  hint,
}: {
  label: string;
  value: string | number;
  accent?: string;
  hint?: string;
}) {
  return (
    <View style={tileStyles.tile}>
      <Text style={[tileStyles.value, accent ? {color: accent} : null]}>{value}</Text>
      <Text style={tileStyles.label}>{label}</Text>
      {hint && <Text style={tileStyles.hint}>{hint}</Text>}
    </View>
  );
}

export function TileRow({children}: {children: React.ReactNode}) {
  return <View style={tileStyles.row}>{children}</View>;
}

const tileStyles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    gap: 8,
  },
  tile: {
    flex: 1,
    backgroundColor: '#F2F2F7',
    borderRadius: 10,
    paddingVertical: 10,
    paddingHorizontal: 8,
    alignItems: 'center',
  },
  value: {
    fontSize: 22,
    fontWeight: '700',
    color: '#1C1C1E',
    fontVariant: ['tabular-nums'],
  },
  label: {
    fontSize: 10,
    color: '#8E8E93',
    fontWeight: '600',
    marginTop: 2,
    textTransform: 'uppercase',
    letterSpacing: 0.4,
  },
  hint: {
    fontSize: 10,
    color: '#8E8E93',
    marginTop: 2,
  },
});

// ─── Bar — labelled horizontal bar ───────────────────────────

export function Bar({
  label,
  value,
  max = 1,
  color = '#007AFF',
  rightLabel,
  height = 6,
}: {
  label?: string;
  value: number;
  max?: number;
  color?: string;
  rightLabel?: string;
  height?: number;
}) {
  const ratio = max > 0 ? Math.max(0, Math.min(1, value / max)) : 0;
  return (
    <View style={barStyles.row}>
      {label !== undefined && (
        <Text style={barStyles.label} numberOfLines={1}>
          {label}
        </Text>
      )}
      <View style={[barStyles.track, {height}]}>
        <View
          style={[
            barStyles.fill,
            {width: `${ratio * 100}%`, backgroundColor: color, height},
          ]}
        />
      </View>
      {rightLabel !== undefined && (
        <Text style={barStyles.rightLabel}>{rightLabel}</Text>
      )}
    </View>
  );
}

const barStyles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    marginVertical: 3,
    gap: 8,
  },
  label: {
    fontSize: 11,
    color: '#3C3C43',
    width: 78,
    fontWeight: '500',
  },
  track: {
    flex: 1,
    backgroundColor: '#E5E5EA',
    borderRadius: 999,
    overflow: 'hidden',
  },
  fill: {
    borderRadius: 999,
  },
  rightLabel: {
    fontSize: 11,
    fontWeight: '600',
    color: '#1C1C1E',
    width: 56,
    textAlign: 'right',
    fontVariant: ['tabular-nums'],
  },
});

// ─── KeyValueRow — 2-col table row ───────────────────────────

export function KeyValueRow({
  k,
  v,
  accent,
}: {
  k: string;
  v: string | number;
  accent?: string;
}) {
  return (
    <View style={kvStyles.row}>
      <Text style={kvStyles.k}>{k}</Text>
      <Text style={[kvStyles.v, accent ? {color: accent} : null]}>{v}</Text>
    </View>
  );
}

const kvStyles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    paddingVertical: 4,
  },
  k: {
    fontSize: 12,
    color: '#8E8E93',
    fontWeight: '500',
  },
  v: {
    fontSize: 13,
    fontWeight: '600',
    color: '#1C1C1E',
    fontVariant: ['tabular-nums'],
  },
});

// ─── Sparkline — bar-style mini chart ────────────────────────

export function Sparkline({
  values,
  color = '#007AFF',
  height = 28,
}: {
  values: number[];
  color?: string;
  height?: number;
}) {
  if (values.length === 0) {
    return <View style={[sparkStyles.row, {height}]} />;
  }
  const max = Math.max(1, ...values);
  return (
    <View style={[sparkStyles.row, {height}]}>
      {values.map((v, i) => {
        const ratio = max > 0 ? v / max : 0;
        return (
          <View
            key={i}
            style={[
              sparkStyles.bar,
              {
                height: Math.max(1, ratio * height),
                backgroundColor: color,
                opacity: 0.35 + 0.65 * (i / Math.max(1, values.length - 1)),
              },
            ]}
          />
        );
      })}
    </View>
  );
}

const sparkStyles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 1,
  },
  bar: {
    flex: 1,
    minWidth: 2,
    borderRadius: 1,
  },
});

// ─── TransportBadge — colored chip ───────────────────────────

export function TransportBadge({
  transport,
  small,
}: {
  transport: string;
  small?: boolean;
}) {
  const color = transportColor(transport);
  return (
    <View
      style={[
        badgeStyles.badge,
        {backgroundColor: color},
        small && badgeStyles.badgeSmall,
      ]}>
      <Text style={[badgeStyles.text, small && badgeStyles.textSmall]}>
        {transportLabel(transport)}
      </Text>
    </View>
  );
}

const badgeStyles = StyleSheet.create({
  badge: {
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 6,
    alignSelf: 'flex-start',
  },
  badgeSmall: {
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 4,
  },
  text: {
    fontSize: 11,
    fontWeight: '700',
    color: '#FFFFFF',
    letterSpacing: 0.4,
  },
  textSmall: {
    fontSize: 9,
  },
});

// ─── Segmented bar — stacked segments for share-of-total views ───

export interface SegmentedBarSegment {
  key: string;
  value: number;
  color: string;
  label?: string;
}

/**
 * A single horizontal bar split into colored segments proportional to each
 * segment's value. Used by the transport-distribution card.
 * Renders nothing when `total <= 0` so callers can inline it without guards.
 */
export function SegmentedBar({
  segments,
  height = 14,
}: {
  segments: SegmentedBarSegment[];
  height?: number;
}) {
  const total = segments.reduce((s, seg) => s + Math.max(0, seg.value), 0);
  if (total <= 0) {return null;}
  return (
    <View style={[segmentStyles.track, {height}]}>
      {segments.map(seg => {
        const v = Math.max(0, seg.value);
        if (v === 0) {return null;}
        const pct = (v / total) * 100;
        return (
          <View
            key={seg.key}
            style={{width: `${pct}%`, backgroundColor: seg.color, height}}
          />
        );
      })}
    </View>
  );
}

const segmentStyles = StyleSheet.create({
  track: {
    flexDirection: 'row',
    width: '100%',
    backgroundColor: '#E5E5EA',
    borderRadius: 999,
    overflow: 'hidden',
  },
});

// ─── VerticalHistogram — vertical bar distribution ──────────────

/**
 * Vertical bar histogram for distributions keyed by integer bucket
 * (hop count, retry count, etc.). Labels each column with its bucket
 * key and renders count below. Empty buckets are shown as empty columns
 * so the shape of the distribution is visible.
 */
export function VerticalHistogram({
  buckets,
  color = '#007AFF',
  height = 60,
}: {
  buckets: Array<{key: string | number; value: number}>;
  color?: string;
  height?: number;
}) {
  const max = Math.max(1, ...buckets.map(b => b.value));
  return (
    <View style={histoStyles.wrap}>
      <View style={[histoStyles.row, {height}]}>
        {buckets.map(b => {
          const ratio = b.value / max;
          return (
            <View key={String(b.key)} style={histoStyles.col}>
              <View style={histoStyles.barWrap}>
                <View
                  style={[
                    histoStyles.bar,
                    {height: Math.max(b.value > 0 ? 2 : 0, ratio * height), backgroundColor: color},
                  ]}
                />
              </View>
            </View>
          );
        })}
      </View>
      <View style={histoStyles.labelRow}>
        {buckets.map(b => (
          <View key={`l-${b.key}`} style={histoStyles.col}>
            <Text style={histoStyles.bucketLabel}>{b.key}</Text>
            <Text style={histoStyles.bucketValue}>{b.value}</Text>
          </View>
        ))}
      </View>
    </View>
  );
}

const histoStyles = StyleSheet.create({
  wrap: {
    marginTop: 4,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 3,
  },
  col: {
    flex: 1,
    alignItems: 'center',
  },
  barWrap: {
    width: '100%',
    flex: 1,
    justifyContent: 'flex-end',
    alignItems: 'center',
  },
  bar: {
    width: '80%',
    borderTopLeftRadius: 2,
    borderTopRightRadius: 2,
  },
  labelRow: {
    flexDirection: 'row',
    gap: 3,
    marginTop: 4,
  },
  bucketLabel: {
    fontSize: 10,
    color: '#3C3C43',
    fontWeight: '600',
    fontVariant: ['tabular-nums'],
  },
  bucketValue: {
    fontSize: 9,
    color: '#8E8E93',
    fontVariant: ['tabular-nums'],
  },
});

// ─── Section divider for headings inside a card ──────────────

export function CardDivider() {
  return <View style={{height: StyleSheet.hairlineWidth, backgroundColor: '#E5E5EA', marginVertical: 8}} />;
}
