import React from 'react';
import {View, Text, StyleSheet} from 'react-native';
import type {RoutingScoreEntry} from '@offline-protocol/mesh-sdk';
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

// ─── RoutingScoreBars — full DORS breakdown for one row ──────

const SCORE_DIMENSIONS: Array<{key: keyof RoutingScoreEntry; label: string; color: string}> = [
  {key: 'signal', label: 'Signal', color: '#007AFF'},
  {key: 'proximity', label: 'Proximity', color: '#5AC8FA'},
  {key: 'bandwidth', label: 'Bandwidth', color: '#34C759'},
  {key: 'congestion', label: 'Congestion', color: '#FF9500'},
  {key: 'energy', label: 'Energy', color: '#FFCC00'},
  {key: 'reliability', label: 'Reliability', color: '#5856D6'},
  {key: 'load', label: 'Load', color: '#AF52DE'},
];

export function RoutingScoreBars({entry}: {entry: RoutingScoreEntry}) {
  const max = Math.max(
    1,
    ...SCORE_DIMENSIONS.map(d => Math.abs(Number(entry[d.key]) || 0)),
    Math.abs(entry.total),
  );
  return (
    <View style={scoreStyles.wrap}>
      <View style={scoreStyles.header}>
        <TransportBadge transport={entry.transport} small />
        <Text style={scoreStyles.total}>total {entry.total.toFixed(2)}</Text>
      </View>
      {SCORE_DIMENSIONS.map(d => {
        const v = Number(entry[d.key]) || 0;
        return (
          <Bar
            key={d.key as string}
            label={d.label}
            value={Math.abs(v)}
            max={max}
            color={d.color}
            rightLabel={v.toFixed(2)}
          />
        );
      })}
    </View>
  );
}

const scoreStyles = StyleSheet.create({
  wrap: {
    backgroundColor: '#F2F2F7',
    borderRadius: 8,
    padding: 10,
    marginTop: 8,
  },
  header: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 4,
  },
  total: {
    fontSize: 11,
    color: '#8E8E93',
    fontWeight: '600',
    fontVariant: ['tabular-nums'],
  },
});

// ─── Section divider for headings inside a card ──────────────

export function CardDivider() {
  return <View style={{height: StyleSheet.hairlineWidth, backgroundColor: '#E5E5EA', marginVertical: 8}} />;
}
