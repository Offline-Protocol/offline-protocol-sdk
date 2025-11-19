import { useCallback, useState } from 'react';
import {
  OfflineProtocol,
  TransportType,
  InternetTransportConfig,
  WifiDirectTransportConfig,
} from '@offlineprotocol/react-native';

interface UseTransportManagementReturn {
  activeTransports: TransportType[];
  forcedTransport: TransportType | null;
  enableTransport: (
    type: TransportType,
    config?: InternetTransportConfig | WifiDirectTransportConfig
  ) => Promise<boolean>;
  disableTransport: (type: TransportType) => Promise<boolean>;
  forceTransport: (type: TransportType) => Promise<boolean>;
  releaseTransportLock: () => Promise<void>;
  refreshTransports: () => Promise<void>;
}

/**
 * Hook for managing transport operations (enable, disable, force, etc.)
 * Extracted from useOfflineProtocol to follow single responsibility principle.
 */
export function useTransportManagement(
  protocol: OfflineProtocol | null
): UseTransportManagementReturn {
  const [activeTransports, setActiveTransports] = useState<TransportType[]>([]);
  const [forcedTransport, setForcedTransport] = useState<TransportType | null>(null);

  const refreshTransports = useCallback(async () => {
    if (!protocol) {
      return;
    }
    try {
      const transports = await protocol.getActiveTransports();
      setActiveTransports(transports);
    } catch (err) {
      console.warn('Failed to get active transports', err);
    }
  }, [protocol]);

  const enableTransport = useCallback(
    async (
      type: TransportType,
      config?: InternetTransportConfig | WifiDirectTransportConfig
    ): Promise<boolean> => {
      if (!protocol) {
        return false;
      }
      try {
        await protocol.enableTransport(type, config);
        await refreshTransports();
        return true;
      } catch (err) {
        console.error(`Failed to enable ${type} transport`, err);
        return false;
      }
    },
    [protocol, refreshTransports]
  );

  const disableTransport = useCallback(
    async (type: TransportType): Promise<boolean> => {
      if (!protocol) {
        return false;
      }
      try {
        await protocol.disableTransport(type);
        await refreshTransports();
        return true;
      } catch (err) {
        console.error(`Failed to disable ${type} transport`, err);
        return false;
      }
    },
    [protocol, refreshTransports]
  );

  const forceTransport = useCallback(
    async (type: TransportType): Promise<boolean> => {
      if (!protocol) {
        return false;
      }
      try {
        await protocol.forceTransport(type);
        setForcedTransport(type);
        return true;
      } catch (err) {
        console.error(`Failed to force ${type} transport`, err);
        return false;
      }
    },
    [protocol]
  );

  const releaseTransportLock = useCallback(async (): Promise<void> => {
    if (!protocol) {
      return;
    }
    try {
      await protocol.releaseTransportLock();
      setForcedTransport(null);
    } catch (err) {
      console.error('Failed to release transport lock', err);
    }
  }, [protocol]);

  return {
    activeTransports,
    forcedTransport,
    enableTransport,
    disableTransport,
    forceTransport,
    releaseTransportLock,
    refreshTransports,
  };
}

