import type { OfflineProtocol } from '@offlineprotocol/react-native';

export type DorsRuntimeConfig = Awaited<ReturnType<OfflineProtocol['getDorsConfig']>>;
export type TransportMetricsSnapshot = Awaited<ReturnType<OfflineProtocol['getTransportMetrics']>>;
export type RelayPriorityInput = 'low' | 'medium' | 'high' | 'never' | 'always' | 'auto';
export type NativeRelayPriority = 'low' | 'medium' | 'high';

export interface FileTransferState {
  fileId: string;
  fileName: string;
  direction: 'outbound' | 'inbound';
  percentage: number;
  chunksCompleted: number;
  totalChunks: number;
  status: 'pending' | 'completed' | 'cancelled';
  recipient?: string;
  sender?: string;
  lastUpdated: number;
}

export const DEFAULT_DORS_CONFIG: DorsRuntimeConfig = {
  preferOnline: false,
  switchHysteresis: 15,
  switchCooldownSecs: 20,
  bleToWifiRetryThreshold: 2,
  rssiSwitchThreshold: -85,
  congestionQueueThreshold: 50,
  stabilityWindowSecs: 8,
  poorSignalDurationSecs: 10,
  ttlEscalationThreshold: 2,
};

export const normalizeRelayPriority = (priority: string): NativeRelayPriority | null => {
  const normalized = priority.toLowerCase();
  switch (normalized) {
    case 'low':
    case 'medium':
    case 'high':
      return normalized as NativeRelayPriority;
    case 'never':
      return 'low';
    case 'always':
      return 'high';
    case 'auto':
      return 'medium';
    default:
      return null;
  }
};

export const mapRelayInputToNative = (priority: RelayPriorityInput): NativeRelayPriority => {
  switch (priority) {
    case 'auto':
      return 'medium';
    case 'never':
      return 'low';
    case 'always':
      return 'high';
    default:
      return priority;
  }
};

export const labelRelayPriority = (priority: RelayPriorityInput): string => {
  switch (priority) {
    case 'auto':
      return 'Auto';
    case 'never':
      return 'Never';
    case 'always':
      return 'Always';
    case 'low':
      return 'Low';
    case 'medium':
      return 'Medium';
    default:
      return 'High';
  }
};

