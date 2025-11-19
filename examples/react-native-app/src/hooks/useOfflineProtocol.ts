import { useEffect, useState, useCallback, useRef } from 'react';
import { Platform } from 'react-native';
import {
  OfflineProtocol,
  ProtocolConfig,
  ProtocolEvent,
  MessagePriority,
  DiagnosticEvent,
  FileProgressEvent,
  FileReceivedEvent,
  MAX_EVENT_HISTORY,
  PROTOCOL_START_DELAY_MS,
} from '@offlineprotocol/react-native';
import { requestBluetoothPermissions, showPermissionRationale, getPermissionDeniedMessage } from '../utils/permissions';
import { ensureBluetoothEnabled } from '../utils/bluetooth';
import { deriveInsights, type DerivedInsights } from '../utils/deriveInsights';
import { useTransportManagement } from './useTransportManagement';
import { useFileTransfer } from './useFileTransfer';
import { useRuntimeState } from './useRuntimeState';

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
  // Re-export from useTransportManagement
  activeTransports: ReturnType<typeof useTransportManagement>['activeTransports'];
  forcedTransport: ReturnType<typeof useTransportManagement>['forcedTransport'];
  enableTransport: ReturnType<typeof useTransportManagement>['enableTransport'];
  disableTransport: ReturnType<typeof useTransportManagement>['disableTransport'];
  forceTransport: ReturnType<typeof useTransportManagement>['forceTransport'];
  releaseTransportLock: ReturnType<typeof useTransportManagement>['releaseTransportLock'];
  // Re-export from useRuntimeState
  batteryLevel: ReturnType<typeof useRuntimeState>['batteryLevel'];
  relayPriority: ReturnType<typeof useRuntimeState>['relayPriority'];
  dorsConfig: ReturnType<typeof useRuntimeState>['dorsConfig'];
  setBatteryLevel: ReturnType<typeof useRuntimeState>['setBatteryLevel'];
  setRelayPriority: ReturnType<typeof useRuntimeState>['setRelayPriority'];
  updateDorsConfig: ReturnType<typeof useRuntimeState>['updateDorsConfig'];
  getTransportMetrics: ReturnType<typeof useRuntimeState>['getTransportMetrics'];
  refreshRuntimeState: ReturnType<typeof useRuntimeState>['refreshRuntimeState'];
  // Re-export from useFileTransfer
  fileTransfers: ReturnType<typeof useFileTransfer>['fileTransfers'];
  sendFile: ReturnType<typeof useFileTransfer>['sendFile'];
  cancelFileTransfer: ReturnType<typeof useFileTransfer>['cancelFileTransfer'];
}

export function useOfflineProtocol(config: ProtocolConfig): UseOfflineProtocolReturn {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [events, setEvents] = useState<ProtocolEvent[]>([]);
  const [insights, setInsights] = useState<DerivedInsights>(() => deriveInsights([]));
  const [permissionsGranted, setPermissionsGranted] = useState(false);
  const protocolRef = useRef<OfflineProtocol | null>(null);

  // Use extracted hooks for specific concerns
  const transportManagement = useTransportManagement(protocol);
  const runtimeState = useRuntimeState(protocol);
  const fileTransfer = useFileTransfer(protocol, isStarted);

  // Combined refresh function that refreshes all runtime state
  const refreshRuntimeState = useCallback(async () => {
    await Promise.all([
      transportManagement.refreshTransports(),
      runtimeState.refreshRuntimeState(),
    ]);
  }, [transportManagement, runtimeState]);

  // Initialize protocol instance - delayed until permissions are checked
  const initializeProtocol = useCallback(() => {
    if (protocolRef.current) {
      console.log('Protocol already initialized');
      return;
    }

    console.log('Initializing protocol with config:', JSON.stringify(config, null, 2));
    
    try {
      // First check if the native module is available
      console.log('Checking if OfflineProtocol class is available...');
      console.log('OfflineProtocol constructor:', typeof OfflineProtocol);
      
      console.log('Creating OfflineProtocol instance...');
      const instance = new OfflineProtocol(config);
      console.log('Protocol instance created successfully:', instance);
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
          fileTransfer.handleFileProgress(annotatedEvent as FileProgressEvent);
        } else if (annotatedEvent.type === 'file_received') {
          fileTransfer.handleFileReceived(annotatedEvent as FileReceivedEvent);
        }

        setEvents((prev) => {
          const nextEvents = [annotatedEvent, ...prev].slice(0, MAX_EVENT_HISTORY);
          setInsights(deriveInsights(nextEvents));
          return nextEvents;
        });
      });

      // Refresh runtime state after initialization
      transportManagement.refreshTransports().catch((err) => {
        console.warn('Failed to refresh transports after initialization', err);
      });
      runtimeState.refreshRuntimeState().catch((err) => {
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

    if (permissionsGranted) {
      console.log('Permissions already granted, skipping request');
      return true;
    }

    if (Platform.OS === 'ios') {
      // iOS prompts automatically when Bluetooth managers are initialized.
      setPermissionsGranted(true);
      setError(null);
      return true;
    }
    
    // Show rationale before requesting permissions (Android only)
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
  }, [permissionsGranted]);

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
      }, PROTOCOL_START_DELAY_MS);
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
      await transportManagement.refreshTransports();
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to stop protocol';
      setError(errorMessage);
    }
  }, []);

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
    start,
    stop,
    sendMessage,
    clearEvents,
    requestPermissions,
    // Re-export from extracted hooks
    activeTransports: transportManagement.activeTransports,
    forcedTransport: transportManagement.forcedTransport,
    enableTransport: transportManagement.enableTransport,
    disableTransport: transportManagement.disableTransport,
    forceTransport: transportManagement.forceTransport,
    releaseTransportLock: transportManagement.releaseTransportLock,
    batteryLevel: runtimeState.batteryLevel,
    relayPriority: runtimeState.relayPriority,
    dorsConfig: runtimeState.dorsConfig,
    setBatteryLevel: runtimeState.setBatteryLevel,
    setRelayPriority: runtimeState.setRelayPriority,
    updateDorsConfig: runtimeState.updateDorsConfig,
    getTransportMetrics: runtimeState.getTransportMetrics,
    refreshRuntimeState,
    fileTransfers: fileTransfer.fileTransfers,
    sendFile: fileTransfer.sendFile,
    cancelFileTransfer: fileTransfer.cancelFileTransfer,
  };
}

