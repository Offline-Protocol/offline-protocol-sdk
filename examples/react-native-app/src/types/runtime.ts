import type { OfflineProtocol } from '@offline-protocol/mesh-sdk';

export type DorsRuntimeConfig = Awaited<ReturnType<OfflineProtocol['getDorsConfig']>>;
export type TransportMetricsSnapshot = Awaited<ReturnType<OfflineProtocol['getTransportMetrics']>>;
export type RelayPriorityInput = 'low' | 'medium' | 'high' | 'never' | 'always' | 'auto';
/** The vocabulary the SDK speaks since 0.22 (was 'low' | 'medium' | 'high'). */
export type NativeRelayPriority = 'never' | 'auto' | 'always';

export interface FileTransferState {
  fileId: string;
  fileName: string;
  contentType?: string;
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
  minSuccessRateBeforeEscalation: 0.3,
  minBleSamplesBeforeSuccessRateEscalation: 5,
  rssiSwitchThreshold: -85,
  congestionQueueThreshold: 50,
  stabilityWindowSecs: 8,
  poorSignalDurationSecs: 10,
  ttlEscalationThreshold: 2,
  congestionDurationSecs: 10,
  ttlEscalationHoldSecs: 20,
  historyWindowSize: 10,
  queueRecoveryRatio: 0.5,
  lowBatteryThreshold: 20,
  relayMinBatteryLevel: 30,
  relayOptimalConnectionCount: 4,
};

export const normalizeRelayPriority = (priority: string): NativeRelayPriority | null => {
  const normalized = priority.toLowerCase();
  switch (normalized) {
    case 'never':
    case 'auto':
    case 'always':
      return normalized as NativeRelayPriority;
    // Legacy spelling of the same three values.
    case 'low':
      return 'never';
    case 'medium':
      return 'auto';
    case 'high':
      return 'always';
    default:
      return null;
  }
};

export const mapRelayInputToNative = (priority: RelayPriorityInput): NativeRelayPriority =>
  normalizeRelayPriority(priority) ?? 'auto';

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

