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

export function formatRelative(ts: number): string {
  const diff = Date.now() - ts;
  if (diff < 1000) {return 'now';}
  if (diff < 60_000) {return `${Math.floor(diff / 1000)}s`;}
  if (diff < 3_600_000) {return `${Math.floor(diff / 60_000)}m`;}
  return `${Math.floor(diff / 3_600_000)}h`;
}
