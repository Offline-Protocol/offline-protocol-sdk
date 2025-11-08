import { useEffect, useState, useCallback, useRef, useMemo } from 'react';
import {
  OfflineProtocol,
  ProtocolConfig,
  ProtocolEvent,
  MessagePriority,
  DiagnosticEvent,
  TransportType,
  InternetTransportConfig,
  WifiDirectTransportConfig,
  FileProgressEvent,
  FileReceivedEvent,
  SendFileParams,
} from '@offlineprotocol/react-native';
import { requestBluetoothPermissions, showPermissionRationale, getPermissionDeniedMessage } from '../utils/permissions';
import { ensureBluetoothEnabled } from '../utils/bluetooth';
import { deriveInsights, type DerivedInsights } from '../utils/deriveInsights';
import {
  DEFAULT_DORS_CONFIG,
  DorsRuntimeConfig,
  FileTransferState,
  NativeRelayPriority,
  RelayPriorityInput,
  TransportMetricsSnapshot,
  normalizeRelayPriority,
} from '../types/runtime';

interface UseOfflineProtocolReturn {
  protocol: OfflineProtocol | null;
  isStarted: boolean;
  error: string | null;
  events: ProtocolEvent[];
  insights: DerivedInsights;
  permissionsGranted: boolean;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  sendMessage: (recipient: string, content: string, priority: MessagePriority) => Promise<string | null>;
  clearEvents: () => void;
  requestPermissions: () => Promise<boolean>;
  activeTransports: TransportType[];
  batteryLevel: number | null;
  relayPriority: NativeRelayPriority;
  dorsConfig: DorsRuntimeConfig;
  forcedTransport: TransportType | null;
  fileTransfers: FileTransferState[];
  refreshRuntimeState: () => Promise<void>;
  enableTransport: (type: TransportType, config?: InternetTransportConfig | WifiDirectTransportConfig) => Promise<boolean>;
  disableTransport: (type: TransportType) => Promise<boolean>;
  forceTransport: (type: TransportType) => Promise<boolean>;
  releaseTransportLock: () => Promise<void>;
  setBatteryLevel: (level: number) => Promise<boolean>;
  setRelayPriority: (priority: RelayPriorityInput) => Promise<boolean>;
  updateDorsConfig: (partial: Partial<DorsRuntimeConfig>) => Promise<boolean>;
  getTransportMetrics: (type: TransportType) => Promise<TransportMetricsSnapshot | null>;
  sendFile: (params: SendFileParams) => Promise<string | null>;
  cancelFileTransfer: (fileId: string) => Promise<boolean>;
}

export function useOfflineProtocol(config: ProtocolConfig): UseOfflineProtocolReturn {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [events, setEvents] = useState<ProtocolEvent[]>([]);
  const [insights, setInsights] = useState<DerivedInsights>(() => deriveInsights([]));
  const [permissionsGranted, setPermissionsGranted] = useState(false);
  const [activeTransports, setActiveTransports] = useState<TransportType[]>([]);
  const [batteryLevel, setBatteryLevelState] = useState<number | null>(null);
  const [relayPriority, setRelayPriorityState] = useState<NativeRelayPriority>('medium');
  const [dorsConfigState, setDorsConfigState] = useState<DorsRuntimeConfig>(DEFAULT_DORS_CONFIG);
  const [forcedTransport, setForcedTransport] = useState<TransportType | null>(null);
  const [fileTransfers, setFileTransfers] = useState<Record<string, FileTransferState>>({});
  const protocolRef = useRef<OfflineProtocol | null>(null);

  const refreshRuntimeState = useCallback(async () => {
    const instance = protocolRef.current;
    if (!instance) {
      console.warn('refreshRuntimeState: Protocol instance not available');
      return;
    }

    try {
      const transports = await instance.getActiveTransports();
      setActiveTransports(transports);
    } catch (err) {
      console.warn('Failed to get active transports', err);
    }

    try {
      const level = await instance.getBatteryLevel();
      setBatteryLevelState(level ?? null);
    } catch (err) {
      console.warn('Failed to get battery level', err);
    }

    try {
      const priority = await instance.getRelayPriority();
      const normalized = normalizeRelayPriority(priority);
      if (normalized) {
        setRelayPriorityState(normalized);
      }
    } catch (err) {
      console.warn('Failed to get relay priority', err);
    }

    try {
      const dors = await instance.getDorsConfig();
      setDorsConfigState(dors);
    } catch (err) {
      console.warn('Failed to get DORS config', err);
    }
  }, []);

  // Initialize protocol instance - delayed until permissions are checked
  const initializeProtocol = useCallback(() => {
    if (protocolRef.current) {
      console.log('Protocol already initialized');
      return;
    }

    console.log('Initializing protocol with config:', JSON.stringify(config, null, 2));
    
    try {
      const instance = new OfflineProtocol(config);
      console.log('Protocol instance created successfully');
      protocolRef.current = instance;
      setProtocol(instance);
      setError(null);

      // Set up event listener
      instance.on('all', (event) => {
        const annotatedEvent = {
          ...event,
          seenAt: Date.now(),
        } as unknown as ProtocolEvent;

        // Filter out verbose diagnostic events
        if (annotatedEvent.type === 'diagnostic') {
          const diagnostic = annotatedEvent as DiagnosticEvent;
          const message = diagnostic.message.toLowerCase();
          // Only log important diagnostics, skip verbose BLE operations
          if (
            message.includes('error') ||
            message.includes('warning') ||
            message.includes('peer discovered') ||
            message.includes('peer lost') ||
            message.includes('message received') ||
            message.includes('message sent')
          ) {
            console.log('🔍', diagnostic.message, diagnostic.context ?? '');
          }
        } else {
          // CRITICAL: Always log message_received events for debugging
          if (annotatedEvent.type === 'message_received') {
            console.log('🎉 MESSAGE_RECEIVED EVENT:', annotatedEvent);
          }
          
          // Only log important protocol events
          if (
            annotatedEvent.type === 'neighbor_discovered' ||
            annotatedEvent.type === 'neighbor_lost' ||
            annotatedEvent.type === 'message_received' ||
            annotatedEvent.type === 'message_sent' ||
            annotatedEvent.type === 'message_delivered' ||
            annotatedEvent.type === 'message_failed'
          ) {
            console.log('Protocol event:', annotatedEvent.type, annotatedEvent);
          }
        }

        if (annotatedEvent.type === 'file_progress') {
          const progressEvent = annotatedEvent as FileProgressEvent;
          setFileTransfers((prev) => {
            const existing = prev[progressEvent.file_id];
            const nextState: FileTransferState = {
              fileId: progressEvent.file_id,
              fileName: existing?.fileName ?? progressEvent.file_id,
              direction: existing?.direction ?? 'outbound',
              percentage: progressEvent.percentage,
              chunksCompleted: progressEvent.chunks_sent,
              totalChunks: progressEvent.total_chunks,
              status: progressEvent.percentage >= 100 ? 'completed' : 'pending',
              recipient: existing?.recipient,
              sender: existing?.sender,
              lastUpdated: Date.now(),
            };
            return {
              ...prev,
              [progressEvent.file_id]: nextState,
            };
          });
        } else if (annotatedEvent.type === 'file_received') {
          const receivedEvent = annotatedEvent as FileReceivedEvent;
          setFileTransfers((prev) => ({
            ...prev,
            [receivedEvent.file_id]: {
              fileId: receivedEvent.file_id,
              fileName: receivedEvent.file_name,
              direction: 'inbound',
              percentage: 100,
              chunksCompleted: prev[receivedEvent.file_id]?.chunksCompleted ?? 0,
              totalChunks: prev[receivedEvent.file_id]?.totalChunks ?? 0,
              status: 'completed',
              sender: receivedEvent.sender,
              lastUpdated: Date.now(),
            },
          }));
        }

        setEvents((prev) => {
          const nextEvents = [annotatedEvent, ...prev].slice(0, 200);
          setInsights(deriveInsights(nextEvents));
          return nextEvents;
        });
      });

      refreshRuntimeState().catch((err) => {
        console.warn('Failed to refresh runtime state after initialization', err);
      });
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to initialize protocol';
      console.error('Failed to initialize protocol:', err);
      setError(errorMessage);
      throw err;
    }
  }, [config]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (protocolRef.current) {
        console.log('Destroying protocol instance');
        protocolRef.current.destroy().catch((err) => {
          console.error('Failed to destroy protocol:', err);
        });
      }
    };
  }, []);

  const requestPermissions = useCallback(async (): Promise<boolean> => {
    console.log('Requesting Bluetooth permissions...');
    
    // Show rationale before requesting permissions
    const shouldRequest = await showPermissionRationale();
    if (!shouldRequest) {
      setError('Permissions are required to use offline messaging');
      setPermissionsGranted(false);
      return false;
    }

    // Request permissions
    const result = await requestBluetoothPermissions();
    setPermissionsGranted(result.granted);

    if (!result.granted) {
      const message = getPermissionDeniedMessage(result.deniedPermissions);
      setError(message);
      console.error('Permissions denied:', result.deniedPermissions);
      return false;
    }

    console.log('All permissions granted');
    setError(null);
    return true;
  }, []);

  const start = useCallback(async () => {
    console.log('start() called');
    
    try {
      // Step 1: Check and request permissions if needed
      if (!permissionsGranted) {
        console.log('Permissions not granted, requesting...');
        const granted = await requestPermissions();
        if (!granted) {
          console.error('Cannot start protocol without permissions');
          return;
        }
      }

      // Step 2: Ensure Bluetooth is enabled
      console.log('Checking if Bluetooth is enabled...');
      const bluetoothEnabled = await ensureBluetoothEnabled();
      if (!bluetoothEnabled) {
        const msg = 'Bluetooth must be enabled to use offline messaging';
        console.error(msg);
        setError(msg);
        return;
      }

      // Step 3: Initialize protocol if not already done
      if (!protocolRef.current) {
        console.log('Initializing protocol...');
        initializeProtocol();
      }

      if (!protocolRef.current) {
        const msg = 'Failed to initialize protocol';
        console.error(msg);
        setError(msg);
        return;
      }

      // Step 4: Start the protocol
      console.log('Calling protocol.start()...');
      await protocolRef.current.start();
      
      // Give the protocol a moment to fully initialize before refreshing state
      setTimeout(async () => {
        await refreshRuntimeState();
      }, 500);
      
      setForcedTransport(null);
      console.log('Protocol started successfully');
      setIsStarted(true);
      setError(null);
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to start protocol';
      console.error('Failed to start protocol:', err);
      setError(errorMessage);
      setIsStarted(false);
    }
  }, [permissionsGranted, requestPermissions, initializeProtocol, refreshRuntimeState]);

  const stop = useCallback(async () => {
    if (!protocolRef.current) {
      setError('Protocol not initialized');
      return;
    }

    try {
      await protocolRef.current.stop();
      setIsStarted(false);
      setError(null);
      setActiveTransports([]);
      setForcedTransport(null);
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to stop protocol';
      setError(errorMessage);
    }
  }, []);

  const enableTransport = useCallback(
    async (type: TransportType, config?: InternetTransportConfig | WifiDirectTransportConfig): Promise<boolean> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return false;
      }
      try {
        await protocolRef.current.enableTransport(type, config);
        await refreshRuntimeState();
        return true;
      } catch (err) {
        const errorMessage =
          err instanceof Error ? err.message : `Failed to enable ${type} transport`;
        setError(errorMessage);
        return false;
      }
    },
    [refreshRuntimeState]
  );

  const disableTransport = useCallback(
    async (type: TransportType): Promise<boolean> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return false;
      }
      try {
        await protocolRef.current.disableTransport(type);
        await refreshRuntimeState();
        return true;
      } catch (err) {
        const errorMessage =
          err instanceof Error ? err.message : `Failed to disable ${type} transport`;
        setError(errorMessage);
        return false;
      }
    },
    [refreshRuntimeState]
  );

  const forceTransport = useCallback(async (type: TransportType): Promise<boolean> => {
    if (!protocolRef.current) {
      setError('Protocol not initialized');
      return false;
    }
    try {
      await protocolRef.current.forceTransport(type);
      setForcedTransport(type);
      return true;
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : `Failed to force ${type} transport`;
      setError(errorMessage);
      return false;
    }
  }, []);

  const releaseTransportLock = useCallback(async (): Promise<void> => {
    if (!protocolRef.current) {
      setError('Protocol not initialized');
      return;
    }
    try {
      await protocolRef.current.releaseTransportLock();
      setForcedTransport(null);
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : 'Failed to release transport lock';
      setError(errorMessage);
    }
  }, []);

  const setBatteryLevel = useCallback(async (level: number): Promise<boolean> => {
    const clamped = Math.max(0, Math.min(100, Math.round(level)));
    if (!protocolRef.current) {
      setError('Protocol not initialized');
      return false;
    }
    try {
      await protocolRef.current.setBatteryLevel(clamped);
      setBatteryLevelState(clamped);
      return true;
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : 'Failed to set battery level';
      setError(errorMessage);
      return false;
    }
  }, []);

  const setRelayPriority = useCallback(
    async (priority: RelayPriorityInput): Promise<boolean> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return false;
      }
      const normalized = normalizeRelayPriority(priority);
      if (!normalized) {
        setError('Invalid relay priority');
        return false;
      }
      try {
        await protocolRef.current.setRelayPriority(normalized);
        setRelayPriorityState(normalized);
        return true;
      } catch (err) {
        const errorMessage =
          err instanceof Error ? err.message : 'Failed to set relay priority';
        setError(errorMessage);
        return false;
      }
    },
    []
  );

  const updateDorsConfig = useCallback(
    async (partial: Partial<DorsRuntimeConfig>): Promise<boolean> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return false;
      }
      const nextConfig = { ...dorsConfigState, ...partial };
      try {
        await protocolRef.current.updateDorsConfig(nextConfig);
        setDorsConfigState(nextConfig);
        return true;
      } catch (err) {
        const errorMessage =
          err instanceof Error ? err.message : 'Failed to update DORS configuration';
        setError(errorMessage);
        return false;
      }
    },
    [dorsConfigState]
  );

  const getTransportMetrics = useCallback(
    async (type: TransportType): Promise<TransportMetricsSnapshot | null> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return null;
      }
      try {
        return await protocolRef.current.getTransportMetrics(type);
      } catch (err) {
        const errorMessage =
          err instanceof Error ? err.message : 'Failed to get transport metrics';
        setError(errorMessage);
        return null;
      }
    },
    []
  );

  const sendFile = useCallback(
    async (params: SendFileParams): Promise<string | null> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return null;
      }
      if (!isStarted) {
        setError('Protocol not started');
        return null;
      }
      try {
        const fileId = await protocolRef.current.sendFile(params);
        const fileName =
          params.fileName ?? params.filePath.split(/[\\/]/).pop() ?? params.filePath;
        setFileTransfers((prev) => ({
          ...prev,
          [fileId]: {
            fileId,
            fileName,
            direction: 'outbound',
            percentage: 0,
            chunksCompleted: 0,
            totalChunks: 0,
            status: 'pending',
            recipient: params.recipient,
            lastUpdated: Date.now(),
          },
        }));
        setError(null);
        return fileId;
      } catch (err) {
        const errorMessage =
          err instanceof Error ? err.message : 'Failed to send file';
        setError(errorMessage);
        return null;
      }
    },
    [isStarted]
  );

  const cancelFileTransfer = useCallback(
    async (fileId: string): Promise<boolean> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return false;
      }
      try {
        const result = await protocolRef.current.cancelFileTransfer(fileId);
        if (result) {
          setFileTransfers((prev) => {
            const existing = prev[fileId];
            if (!existing) {
              return prev;
            }
            return {
              ...prev,
              [fileId]: {
                ...existing,
                status: 'cancelled',
                lastUpdated: Date.now(),
              },
            };
          });
        }
        return result;
      } catch (err) {
        const errorMessage =
          err instanceof Error ? err.message : 'Failed to cancel file transfer';
        setError(errorMessage);
        return false;
      }
    },
    []
  );

  const fileTransferList = useMemo(() => {
    return Object.values(fileTransfers).sort((a, b) => b.lastUpdated - a.lastUpdated);
  }, [fileTransfers]);
  const sendMessage = useCallback(
    async (recipient: string, content: string, priority: MessagePriority): Promise<string | null> => {
      if (!protocolRef.current) {
        setError('Protocol not initialized');
        return null;
      }

      if (!isStarted) {
        setError('Protocol not started');
        return null;
      }

      try {
        const messageId = await protocolRef.current.sendMessage({
          recipient,
          content,
          priority,
        });
        setError(null);
        return messageId;
      } catch (err) {
        const errorMessage = err instanceof Error ? err.message : 'Failed to send message';
        setError(errorMessage);
        return null;
      }
    },
    [isStarted]
  );

  const clearEvents = useCallback(() => {
    setEvents([]);
    setInsights(deriveInsights([]));
  }, []);

  return {
    protocol,
    isStarted,
    error,
    events,
    insights,
    permissionsGranted,
    activeTransports,
    batteryLevel,
    relayPriority,
    dorsConfig: dorsConfigState,
    forcedTransport,
    fileTransfers: fileTransferList,
    refreshRuntimeState,
    start,
    stop,
    enableTransport,
    disableTransport,
    forceTransport,
    releaseTransportLock,
    setBatteryLevel,
    setRelayPriority,
    updateDorsConfig,
    getTransportMetrics,
    sendMessage,
    sendFile,
    cancelFileTransfer,
    clearEvents,
    requestPermissions,
  };
}

