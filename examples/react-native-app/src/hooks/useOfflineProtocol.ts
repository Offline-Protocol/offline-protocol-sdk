import { useEffect, useState, useCallback, useRef } from 'react';
import {
  OfflineProtocol,
  ProtocolConfig,
  ProtocolEvent,
  MessagePriority,
} from '@offlineprotocol/react-native';

interface UseOfflineProtocolReturn {
  protocol: OfflineProtocol | null;
  isStarted: boolean;
  error: string | null;
  events: ProtocolEvent[];
  start: () => Promise<void>;
  stop: () => Promise<void>;
  sendMessage: (recipient: string, content: string, priority: MessagePriority) => Promise<string | null>;
  clearEvents: () => void;
}

export function useOfflineProtocol(config: ProtocolConfig): UseOfflineProtocolReturn {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [events, setEvents] = useState<ProtocolEvent[]>([]);
  const protocolRef = useRef<OfflineProtocol | null>(null);

  // Initialize protocol instance
  useEffect(() => {
    console.log('Initializing protocol with config:', JSON.stringify(config, null, 2));
    
    try {
      const instance = new OfflineProtocol(config);
      console.log('Protocol instance created successfully');
      protocolRef.current = instance;
      setProtocol(instance);
      setError(null);

      // Set up event listener
      instance.on('all', (event) => {
        console.log('Protocol event:', event.type, event);
        setEvents((prev) => [event, ...prev].slice(0, 100)); // Keep last 100 events
      });

      return () => {
        // Cleanup on unmount
        console.log('Destroying protocol instance');
        instance.destroy().catch((err) => {
          console.error('Failed to destroy protocol:', err);
        });
      };
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to initialize protocol';
      console.error('Failed to initialize protocol:', err);
      setError(errorMessage);
      return undefined;
    }
  }, [config.appId, config.userId]);

  const start = useCallback(async () => {
    console.log('start() called, protocolRef.current:', !!protocolRef.current);
    
    if (!protocolRef.current) {
      const msg = 'Protocol not initialized';
      console.error(msg);
      setError(msg);
      return;
    }

    try {
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
  }, []);

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
    start,
    stop,
    sendMessage,
    clearEvents,
  };
}

