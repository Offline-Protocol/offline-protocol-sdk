import React, {
  createContext,
  useContext,
  useState,
  useCallback,
  useRef,
  useEffect,
} from 'react';
import {OfflineProtocol} from '@offline-protocol/mesh-sdk';
import type {Neighbor, ChatMessage, Chat, LogEntry} from '../types';
import {
  PROTOCOL_CONFIG,
  DEFAULT_RELAYS,
  MAX_LOG_ENTRIES,
  STALE_NEIGHBOR_MS,
  STALE_NEIGHBOR_CLEANUP_INTERVAL_MS,
} from '../constants';

// ─── Context Shape ───────────────────────────────────────────

interface ProtocolContextValue {
  // State
  protocol: OfflineProtocol | null;
  isStarted: boolean;
  isTransportEnabled: boolean;
  userId: string;
  userName: string;
  neighbors: Map<string, Neighbor>;
  chats: Map<string, Chat>;
  logs: LogEntry[];

  // Actions
  initialize: (userId: string, userName: string) => Promise<void>;
  stop: () => Promise<void>;
  sendMessage: (recipientId: string, content: string) => Promise<void>;
  toggleTransport: () => Promise<void>;
  markChatRead: (peerId: string) => void;
}

const ProtocolContext = createContext<ProtocolContextValue | null>(null);

export function useProtocol(): ProtocolContextValue {
  const ctx = useContext(ProtocolContext);
  if (!ctx) {
    throw new Error('useProtocol must be used within ProtocolProvider');
  }
  return ctx;
}

// ─── Provider ────────────────────────────────────────────────

export function ProtocolProvider({children}: {children: React.ReactNode}) {
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [isTransportEnabled, setIsTransportEnabled] = useState(false);
  const [userId, setUserId] = useState('');
  const [userName, setUserName] = useState('');
  const [neighbors, setNeighbors] = useState<Map<string, Neighbor>>(new Map());
  const [chats, setChats] = useState<Map<string, Chat>>(new Map());
  const [logs, setLogs] = useState<LogEntry[]>([]);

  const protocolRef = useRef<OfflineProtocol | null>(null);
  const processedMessagesRef = useRef<Set<string>>(new Set());
  const userIdRef = useRef(userId);

  // Keep refs in sync
  useEffect(() => { userIdRef.current = userId; }, [userId]);

  // ─── Logging ─────────────────────────────────────────────

  const addLog = useCallback((level: LogEntry['level'], message: string) => {
    setLogs(prev => {
      const next = [
        ...prev,
        {
          id: Date.now().toString() + Math.random(),
          timestamp: Date.now(),
          level,
          message,
        },
      ];
      return next.length > MAX_LOG_ENTRIES ? next.slice(-MAX_LOG_ENTRIES) : next;
    });
  }, []);

  // ─── Event Handler ───────────────────────────────────────

  const handleEvent = useCallback((event: any) => {
    const eventType = event.type;

    switch (eventType) {
      case 'message_received': {
        const msgId = event.message_id || event.messageId || Date.now().toString();

        // Deduplication
        if (processedMessagesRef.current.has(msgId)) {break;}
        processedMessagesRef.current.add(msgId);

        // Prevent unbounded growth
        if (processedMessagesRef.current.size > 1000) {
          processedMessagesRef.current.clear();
        }

        const senderId = event.sender || 'unknown';
        const content = event.content || '';

        const chatMsg: ChatMessage = {
          id: msgId,
          senderId,
          recipientId: userIdRef.current,
          content,
          timestamp: Date.now(),
          status: 'delivered',
          isOutgoing: false,
        };

        setChats(prev => {
          const next = new Map(prev);
          const chat = next.get(senderId) || {peerId: senderId, messages: [], unreadCount: 0};
          next.set(senderId, {
            ...chat,
            messages: [...chat.messages, chatMsg],
            unreadCount: chat.unreadCount + 1,
          });
          return next;
        });

        addLog('info', `Message from ${senderId}: ${content}`);
        break;
      }

      case 'message_sent': {
        const msgId = event.message_id || event.messageId;
        if (msgId) {
          setChats(prev => {
            const next = new Map(prev);
            for (const [peerId, chat] of next) {
              const updated = chat.messages.map(m =>
                m.id === msgId ? {...m, status: 'sent' as const} : m,
              );
              if (updated !== chat.messages) {
                next.set(peerId, {...chat, messages: updated});
              }
            }
            return next;
          });
        }
        addLog('info', `Message sent: ${msgId}`);
        break;
      }

      case 'message_delivered': {
        const msgId = event.message_id || event.messageId;
        if (msgId) {
          setChats(prev => {
            const next = new Map(prev);
            for (const [peerId, chat] of next) {
              const updated = chat.messages.map(m =>
                m.id === msgId ? {...m, status: 'delivered' as const} : m,
              );
              if (updated !== chat.messages) {
                next.set(peerId, {...chat, messages: updated});
              }
            }
            return next;
          });
        }
        addLog('info', `Message delivered: ${msgId}`);
        break;
      }

      case 'message_failed': {
        const msgId = event.message_id || event.messageId;
        if (msgId) {
          setChats(prev => {
            const next = new Map(prev);
            for (const [peerId, chat] of next) {
              const updated = chat.messages.map(m =>
                m.id === msgId ? {...m, status: 'failed' as const} : m,
              );
              if (updated !== chat.messages) {
                next.set(peerId, {...chat, messages: updated});
              }
            }
            return next;
          });
        }
        addLog('error', `Message failed: ${msgId} - ${event.reason || 'unknown'}`);
        break;
      }

      case 'neighbor_discovered': {
        const peerId = event.peer_id || event.peerId;
        if (peerId) {
          setNeighbors(prev => {
            const next = new Map(prev);
            next.set(peerId, {
              peerId,
              transport: event.transport || 'nostr',
              discoveredAt: Date.now(),
            });
            return next;
          });
          addLog('info', `Peer discovered: ${peerId}`);
        }
        break;
      }

      case 'neighbor_lost': {
        const peerId = event.peer_id || event.peerId;
        if (peerId) {
          setNeighbors(prev => {
            const next = new Map(prev);
            next.delete(peerId);
            return next;
          });
          addLog('info', `Peer lost: ${peerId}`);
        }
        break;
      }

      case 'transport_switched':
        addLog('info', `Transport switched to: ${event.to}`);
        break;

      default:
        addLog('debug', `Event: ${eventType}`);
        break;
    }
  }, [addLog]);

  // ─── Stale Neighbor Cleanup ──────────────────────────────

  useEffect(() => {
    if (!isStarted) {return;}

    const interval = setInterval(() => {
      const now = Date.now();
      setNeighbors(prev => {
        let changed = false;
        const next = new Map(prev);
        for (const [peerId, neighbor] of next) {
          if (now - neighbor.discoveredAt > STALE_NEIGHBOR_MS) {
            next.delete(peerId);
            changed = true;
          }
        }
        return changed ? next : prev;
      });
    }, STALE_NEIGHBOR_CLEANUP_INTERVAL_MS);

    return () => clearInterval(interval);
  }, [isStarted]);

  // ─── Shutdown Cleanup ────────────────────────────────────

  useEffect(() => {
    return () => {
      if (protocolRef.current) {
        protocolRef.current.stop().catch(() => {});
      }
    };
  }, []);

  // ─── Actions ─────────────────────────────────────────────

  const initialize = useCallback(async (uid: string, uname: string) => {
    setUserId(uid);
    setUserName(uname);

    const proto = new OfflineProtocol({
      ...PROTOCOL_CONFIG,
      userId: uid,
    });
    protocolRef.current = proto;
    setProtocol(proto);

    // Register event handler
    proto.on('all', handleEvent);

    // Start the protocol
    await proto.start();
    setIsStarted(true);
    setIsTransportEnabled(true);

    addLog('info', 'Protocol started with Nostr transport');
    addLog('info', `User ID: ${uid}`);
    addLog('info', `Relays: ${DEFAULT_RELAYS.join(', ')}`);
  }, [handleEvent, addLog]);

  const stop = useCallback(async () => {
    if (!protocolRef.current) {return;}
    try {
      await protocolRef.current.stop();
      setIsStarted(false);
      setIsTransportEnabled(false);
      setNeighbors(new Map());
      addLog('info', 'Protocol stopped');
    } catch (error: any) {
      addLog('error', `Failed to stop: ${error.message}`);
    }
  }, [addLog]);

  const sendMessage = useCallback(async (recipientId: string, content: string) => {
    if (!protocolRef.current) {return;}

    // Optimistic message
    const tempId = Date.now().toString() + Math.random();
    const chatMsg: ChatMessage = {
      id: tempId,
      senderId: userIdRef.current,
      recipientId,
      content,
      timestamp: Date.now(),
      status: 'sending',
      isOutgoing: true,
    };

    setChats(prev => {
      const next = new Map(prev);
      const chat = next.get(recipientId) || {peerId: recipientId, messages: [], unreadCount: 0};
      next.set(recipientId, {
        ...chat,
        messages: [...chat.messages, chatMsg],
      });
      return next;
    });

    try {
      const msgId = await protocolRef.current.sendMessage({
        recipient: recipientId,
        content,
      });

      // Update temp ID with real message ID and status
      setChats(prev => {
        const next = new Map(prev);
        const chat = next.get(recipientId);
        if (chat) {
          next.set(recipientId, {
            ...chat,
            messages: chat.messages.map(m =>
              m.id === tempId ? {...m, id: msgId || tempId, status: 'sent'} : m,
            ),
          });
        }
        return next;
      });

      addLog('info', `Sent to ${recipientId}: ${content}`);
    } catch (error: any) {
      // Mark as failed
      setChats(prev => {
        const next = new Map(prev);
        const chat = next.get(recipientId);
        if (chat) {
          next.set(recipientId, {
            ...chat,
            messages: chat.messages.map(m =>
              m.id === tempId ? {...m, status: 'failed'} : m,
            ),
          });
        }
        return next;
      });

      addLog('error', `Send failed: ${error.message}`);
      throw error;
    }
  }, [addLog]);

  const toggleTransport = useCallback(async () => {
    if (!protocolRef.current) {return;}
    try {
      if (isTransportEnabled) {
        await protocolRef.current.disableTransport('nostr');
        setIsTransportEnabled(false);
        addLog('info', 'Nostr transport disabled');
      } else {
        await protocolRef.current.enableTransport('nostr', {
          enabled: true,
          relayUrls: DEFAULT_RELAYS,
          autoReconnect: true,
        });
        setIsTransportEnabled(true);
        addLog('info', 'Nostr transport enabled');
      }
    } catch (error: any) {
      addLog('error', `Toggle transport failed: ${error.message}`);
    }
  }, [isTransportEnabled, addLog]);

  const markChatRead = useCallback((peerId: string) => {
    setChats(prev => {
      const chat = prev.get(peerId);
      if (!chat || chat.unreadCount === 0) {return prev;}
      const next = new Map(prev);
      next.set(peerId, {...chat, unreadCount: 0});
      return next;
    });
  }, []);

  // ─── Context Value ───────────────────────────────────────

  const value: ProtocolContextValue = {
    protocol,
    isStarted,
    isTransportEnabled,
    userId,
    userName,
    neighbors,
    chats,
    logs,
    initialize,
    stop,
    sendMessage,
    toggleTransport,
    markChatRead,
  };

  return (
    <ProtocolContext.Provider value={value}>
      {children}
    </ProtocolContext.Provider>
  );
}
