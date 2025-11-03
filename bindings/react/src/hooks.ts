/**
 * React hooks for Offline Protocol SDK
 * 
 * @module hooks
 * 
 * Note: React must be installed as a peer dependency for this module to work.
 * The types will be resolved at runtime when React is available in the consuming application.
 */

// @ts-ignore - React is a peer dependency and will be available at runtime
import { useEffect, useState, useRef, useCallback } from 'react';
import { OfflineProtocol, ProtocolConfig, MessagePriority, Event, EventListener } from './index';

/**
 * Hook to create and manage an OfflineProtocol instance
 * 
 * @param config - Protocol configuration
 * @returns Object containing protocol instance, start status, and control functions
 * 
 * @example
 * ```typescript
 * const { protocol, isStarted, start, stop } = useOfflineProtocol({
 *   appId: 'my-app',
 *   userId: 'user123',
 * });
 * 
 * useEffect(() => {
 *   if (!isStarted) {
 *     start();
 *   }
 *   return () => {
 *     stop();
 *   };
 * }, [isStarted, start, stop]);
 * ```
 */
export function useOfflineProtocol(config: ProtocolConfig) {
  const [isStarted, setIsStarted] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const protocolRef = useRef<OfflineProtocol | null>(null);

  // Initialize protocol instance
  useEffect(() => {
    protocolRef.current = new OfflineProtocol(config);
    return () => {
      if (protocolRef.current) {
        protocolRef.current.stop().catch(console.error);
        protocolRef.current = null;
      }
    };
  }, [config.appId, config.userId]); // Re-create if appId or userId changes

  const start = useCallback(async () => {
    if (!protocolRef.current) {
      setError(new Error('Protocol not initialized'));
      return;
    }

    try {
      await protocolRef.current.start();
      setIsStarted(true);
      setError(null);
    } catch (err) {
      const error = err instanceof Error ? err : new Error(String(err));
      setError(error);
      setIsStarted(false);
    }
  }, []);

  const stop = useCallback(async () => {
    if (!protocolRef.current) {
      return;
    }

    try {
      await protocolRef.current.stop();
      setIsStarted(false);
      setError(null);
    } catch (err) {
      const error = err instanceof Error ? err : new Error(String(err));
      setError(error);
    }
  }, []);

  return {
    protocol: protocolRef.current,
    isStarted,
    error,
    start,
    stop,
  };
}

/**
 * Hook to listen to protocol events
 * 
 * @param protocol - Protocol instance (from useOfflineProtocol)
 * @param event - Event name
 * @param listener - Event handler function
 * 
 * @example
 * ```typescript
 * useProtocolEvent(protocol, 'message:received', (event) => {
 *   console.log('Received:', event.content);
 * });
 * ```
 */
export function useProtocolEvent<T extends Event>(
  protocol: OfflineProtocol | null,
  event: string,
  listener: EventListener<T>
): void {
  useEffect(() => {
    if (!protocol) {
      return;
    }

    // Cast to any to allow dynamic event names (the on method has overloads for specific events)
    (protocol as any).on(event, listener);

    return () => {
      (protocol as any).off(event, listener);
    };
  }, [protocol, event, listener]);
}

/**
 * Hook to send messages with the protocol
 * 
 * @param protocol - Protocol instance (from useOfflineProtocol)
 * @returns Function to send messages
 * 
 * @example
 * ```typescript
 * const sendMessage = useSendMessage(protocol);
 * 
 * const handleSend = async () => {
 *   try {
 *     const messageId = await sendMessage({
 *       recipient: 'user456',
 *       content: 'Hello!',
 *       priority: MessagePriority.High,
 *     });
 *     console.log('Sent:', messageId);
 *   } catch (error) {
 *     console.error('Failed to send:', error);
 *   }
 * };
 * ```
 */
export function useSendMessage(protocol: OfflineProtocol | null) {
  return useCallback(
    async (params: {
      recipient: string;
      content: string;
      priority?: MessagePriority;
    }) => {
      if (!protocol) {
        throw new Error('Protocol is not initialized');
      }

      if (!protocol.started) {
        throw new Error('Protocol is not started');
      }

      return protocol.sendMessage(params);
    },
    [protocol]
  );
}

