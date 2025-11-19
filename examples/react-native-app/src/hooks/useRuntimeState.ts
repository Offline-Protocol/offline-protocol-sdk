import { useCallback, useState } from 'react';
import { OfflineProtocol } from '@offlineprotocol/react-native';
import {
  DorsRuntimeConfig,
  NativeRelayPriority,
  RelayPriorityInput,
  TransportMetricsSnapshot,
  normalizeRelayPriority,
  DEFAULT_DORS_CONFIG,
} from '../types/runtime';

interface UseRuntimeStateReturn {
  batteryLevel: number | null;
  relayPriority: NativeRelayPriority;
  dorsConfig: DorsRuntimeConfig;
  setBatteryLevel: (level: number) => Promise<boolean>;
  setRelayPriority: (priority: RelayPriorityInput) => Promise<boolean>;
  updateDorsConfig: (partial: Partial<DorsRuntimeConfig>) => Promise<boolean>;
  getTransportMetrics: (type: string) => Promise<TransportMetricsSnapshot | null>;
  refreshRuntimeState: () => Promise<void>;
}

/**
 * Hook for managing runtime state (battery, relay priority, DORS config, etc.)
 * Extracted from useOfflineProtocol to follow single responsibility principle.
 */
export function useRuntimeState(
  protocol: OfflineProtocol | null
): UseRuntimeStateReturn {
  const [batteryLevel, setBatteryLevelState] = useState<number | null>(null);
  const [relayPriority, setRelayPriorityState] = useState<NativeRelayPriority>('medium');
  const [dorsConfigState, setDorsConfigState] = useState<DorsRuntimeConfig>(DEFAULT_DORS_CONFIG);

  const refreshRuntimeState = useCallback(async () => {
    if (!protocol) {
      return;
    }

    try {
      const level = await protocol.getBatteryLevel();
      setBatteryLevelState(level ?? null);
    } catch (err) {
      console.warn('Failed to get battery level', err);
    }

    try {
      const priority = await protocol.getRelayPriority();
      const normalized = normalizeRelayPriority(priority);
      if (normalized) {
        setRelayPriorityState(normalized);
      }
    } catch (err) {
      console.warn('Failed to get relay priority', err);
    }

    try {
      const dors = await protocol.getDorsConfig();
      setDorsConfigState(dors);
    } catch (err) {
      console.warn('Failed to get DORS config', err);
    }
  }, [protocol]);

  const setBatteryLevel = useCallback(
    async (level: number): Promise<boolean> => {
      const clamped = Math.max(0, Math.min(100, Math.round(level)));
      if (!protocol) {
        return false;
      }
      try {
        await protocol.setBatteryLevel(clamped);
        setBatteryLevelState(clamped);
        return true;
      } catch (err) {
        console.error('Failed to set battery level', err);
        return false;
      }
    },
    [protocol]
  );

  const setRelayPriority = useCallback(
    async (priority: RelayPriorityInput): Promise<boolean> => {
      if (!protocol) {
        return false;
      }
      const normalized = normalizeRelayPriority(priority);
      if (!normalized) {
        return false;
      }
      try {
        await protocol.setRelayPriority(normalized);
        setRelayPriorityState(normalized);
        return true;
      } catch (err) {
        console.error('Failed to set relay priority', err);
        return false;
      }
    },
    [protocol]
  );

  const updateDorsConfig = useCallback(
    async (partial: Partial<DorsRuntimeConfig>): Promise<boolean> => {
      if (!protocol) {
        return false;
      }
      const nextConfig = { ...dorsConfigState, ...partial };
      try {
        await protocol.updateDorsConfig(nextConfig);
        setDorsConfigState(nextConfig);
        return true;
      } catch (err) {
        console.error('Failed to update DORS configuration', err);
        return false;
      }
    },
    [protocol, dorsConfigState]
  );

  const getTransportMetrics = useCallback(
    async (type: string): Promise<TransportMetricsSnapshot | null> => {
      if (!protocol) {
        return null;
      }
      try {
        // Type assertion is safe here as the native module validates the transport type
        return await protocol.getTransportMetrics(type as 'ble' | 'internet' | 'wifiDirect');
      } catch (err) {
        console.error('Failed to get transport metrics', err);
        return null;
      }
    },
    [protocol]
  );

  return {
    batteryLevel,
    relayPriority,
    dorsConfig: dorsConfigState,
    setBatteryLevel,
    setRelayPriority,
    updateDorsConfig,
    getTransportMetrics,
    refreshRuntimeState,
  };
}

