import type {TransportType} from '@offline-protocol/mesh-sdk';

const TRANSPORT_COLORS: Record<string, string> = {
  ble: '#007AFF',
  wifiDirect: '#34C759',
  internet: '#5856D6',
  reticulum: '#FF9500',
  nostr: '#AF52DE',
};

const TRANSPORT_LABELS: Record<string, string> = {
  ble: 'BLE',
  wifiDirect: 'WiFi-D',
  internet: 'NET',
  reticulum: 'RNS',
  nostr: 'NOSTR',
};

export function transportColor(t: TransportType | string): string {
  return TRANSPORT_COLORS[t as string] ?? '#8E8E93';
}

export function transportLabel(t: TransportType | string): string {
  return TRANSPORT_LABELS[t as string] ?? String(t).toUpperCase();
}

const TRANSPORT_STATUS_COLORS: Record<string, string> = {
  available: '#34C759',
  connecting: '#FF9500',
  unavailable: '#8E8E93',
  disconnected: '#8E8E93',
  error: '#FF3B30',
};

export function transportStatusColor(s: string): string {
  return TRANSPORT_STATUS_COLORS[s] ?? '#8E8E93';
}

const ROUTING_PHASE_COLORS: Record<string, string> = {
  scoreUpdated: '#8E8E93',
  selected: '#007AFF',
  switched: '#5856D6',
  escalated: '#FF9500',
  unknown: '#C7C7CC',
};

export function routingPhaseColor(p: string): string {
  return ROUTING_PHASE_COLORS[p] ?? '#8E8E93';
}

const REASON_LABELS: Record<string, string> = {
  initialSelection: 'initial',
  primarySelected: 'primary',
  primarySuccess: 'primary ok',
  fallbackSuccess: 'fallback ok',
  escalationApplied: 'escalation',
  currentUnavailable: 'unavailable',
  retryThreshold: 'retry',
  poorSignal: 'poor signal',
  congestion: 'congestion',
  lowTtl: 'low ttl',
  lowSuccessRate: 'low success',
  unknown: 'unknown',
};

export function reasonLabel(r?: string | null): string {
  if (!r) {return '—';}
  return REASON_LABELS[r] ?? r;
}

/** Compact human-readable byte count: 12.4 KB / 1.2 MB / 4.5 GB. */
export function formatBytes(n: number): string {
  if (n < 1024) {return `${n} B`;}
  if (n < 1024 * 1024) {return `${(n / 1024).toFixed(1)} KB`;}
  if (n < 1024 * 1024 * 1024) {return `${(n / 1024 / 1024).toFixed(1)} MB`;}
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** Compact integer count: 1.2K / 4.5M. */
export function formatCount(n: number): string {
  if (n < 1000) {return String(n);}
  if (n < 1_000_000) {return `${(n / 1000).toFixed(1)}K`;}
  return `${(n / 1_000_000).toFixed(1)}M`;
}

export function formatPercent(n: number, digits = 0): string {
  return `${(n * 100).toFixed(digits)}%`;
}

export function formatRelative(ts: number): string {
  const diff = Date.now() - ts;
  if (diff < 1000) {return 'now';}
  if (diff < 60_000) {return `${Math.floor(diff / 1000)}s`;}
  if (diff < 3_600_000) {return `${Math.floor(diff / 60_000)}m`;}
  return `${Math.floor(diff / 3_600_000)}h`;
}
