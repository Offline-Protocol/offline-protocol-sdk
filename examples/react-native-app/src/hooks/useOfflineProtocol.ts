import { useEffect, useState, useCallback, useRef } from 'react';
import {
  OfflineProtocol,
  ProtocolConfig,
  ProtocolEvent,
  MessagePriority,
} from '@offlineprotocol/react-native';
import { requestBluetoothPermissions, showPermissionRationale, getPermissionDeniedMessage } from '../utils/permissions';
import { ensureBluetoothEnabled } from '../utils/bluetooth';

interface UseOfflineProtocolReturn {
  protocol: OfflineProtocol | null;
  isStarted: boolean;
  error: string | null;
  events: ProtocolEvent[];
  permissionsGranted: boolean;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  sendMessage: (recipient: string, content: string, priority: MessagePriority) => Promise<string | null>;
  clearEvents: () => void;
  requestPermissions: () => Promise<boolean>;
}

export function useOfflineProtocol(config: ProtocolConfig): UseOfflineProtocolReturn {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [events, setEvents] = useState<ProtocolEvent[]>([]);
  const [permissionsGranted, setPermissionsGranted] = useState(false);
  const protocolRef = useRef<OfflineProtocol | null>(null);

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
        // Special formatting for diagnostic messages
        if (event.type === 'diagnostic') {
          console.log('🔍', (event as any).message);
        } else {
          console.log('Protocol event:', event.type, event);
        }
        setEvents((prev) => [event, ...prev].slice(0, 100)); // Keep last 100 events
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
      console.log('Protocol started successfully');
      setIsStarted(true);
      setError(null);
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to start protocol';
      console.error('Failed to start protocol:', err);
      setError(errorMessage);
      setIsStarted(false);
    }
  }, [permissionsGranted, requestPermissions, initializeProtocol]);

  const stop = useCallback(async () => {
    if (!protocolRef.current) {
      setError('Protocol not initialized');
      return;
    }

    try {
      await protocolRef.current.stop();
      setIsStarted(false);
      setError(null);
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
  }, []);

  return {
    protocol,
    isStarted,
    error,
    events,
    permissionsGranted,
    start,
    stop,
    sendMessage,
    clearEvents,
    requestPermissions,
  };
}

