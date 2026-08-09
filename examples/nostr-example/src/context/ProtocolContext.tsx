import React, {
  createContext,
  useContext,
  useState,
  useCallback,
  useRef,
  useEffect,
} from 'react';
import {OfflineProtocol} from '@offline-protocol/mesh-sdk';
import type {Neighbor, ChatMessage, Chat, LogEntry, ConnectionStatus} from '../types';
import {
  PROTOCOL_CONFIG,
  DEFAULT_RELAYS,
  MAX_LOG_ENTRIES,
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
  sendConnectionRequest: (recipientId: string) => Promise<void>;
  acceptConnection: (peerId: string) => Promise<void>;
  rejectConnection: (peerId: string) => Promise<void>;
  cancelConnectionRequest: (peerId: string) => Promise<void>;
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
  const userNameRef = useRef(userName);

  // Keep refs in sync
  useEffect(() => { userIdRef.current = userId; }, [userId]);
  useEffect(() => { userNameRef.current = userName; }, [userName]);

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
            const existing = next.get(peerId);
            // Preserve connection status if peer already known
            next.set(peerId, {
              peerId,
              transport: event.transport || 'nostr',
              discoveredAt: Date.now(),
              connectionStatus: existing?.connectionStatus || 'none',
              displayName: existing?.displayName,
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

      // ─── Connection Request Events ─────────────────────────

      case 'connection_request_received': {
        const sender = event.sender;
        const senderName = event.sender_name || sender;
        if (sender) {
          setNeighbors(prev => {
            const next = new Map(prev);
            const existing = next.get(sender);
            next.set(sender, {
              peerId: sender,
              transport: existing?.transport || 'nostr',
              discoveredAt: existing?.discoveredAt || Date.now(),
              connectionStatus: 'pending_received',
              displayName: senderName,
            });
            return next;
          });
          addLog('info', `Connection request from ${senderName} (${sender})`);
        }
        break;
      }

      case 'connection_accepted': {
        const acceptedBy = event.accepted_by;
        const acceptedByName = event.accepted_by_name || acceptedBy;
        if (acceptedBy) {
          setNeighbors(prev => {
            const next = new Map(prev);
            const existing = next.get(acceptedBy);
            if (existing) {
              next.set(acceptedBy, {
                ...existing,
                connectionStatus: 'accepted',
                displayName: existing.displayName || acceptedByName,
              });
            }
            return next;
          });
          addLog('info', `Connection accepted by ${acceptedByName}`);
        }
        break;
      }

      case 'connection_rejected': {
        const rejectedBy = event.rejected_by;
        if (rejectedBy) {
          setNeighbors(prev => {
            const next = new Map(prev);
            const existing = next.get(rejectedBy);
            if (existing) {
              next.set(rejectedBy, {...existing, connectionStatus: 'rejected'});
            }
            return next;
          });
          addLog('info', `Connection rejected by ${rejectedBy}`);
        }
        break;
      }

      case 'connection_request_cancelled': {
        const cancelledBy = event.cancelled_by;
        if (cancelledBy) {
          setNeighbors(prev => {
            const next = new Map(prev);
            const existing = next.get(cancelledBy);
            if (existing) {
              next.set(cancelledBy, {...existing, connectionStatus: 'none'});
            }
            return next;
          });
          addLog('info', `Connection request cancelled by ${cancelledBy}`);
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
      profile: uid,
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

  // ─── Connection Request Actions ──────────────────────────

  const sendConnectionRequest = useCallback(async (recipientId: string) => {
    if (!protocolRef.current) {return;}
    try {
      await protocolRef.current.sendConnectionRequest({
        recipient: recipientId,
        senderName: userNameRef.current,
      });
      setNeighbors(prev => {
        const next = new Map(prev);
        const existing = next.get(recipientId);
        next.set(recipientId, {
          peerId: recipientId,
          transport: existing?.transport || 'nostr',
          discoveredAt: existing?.discoveredAt || Date.now(),
          connectionStatus: 'pending_sent',
          displayName: existing?.displayName,
        });
        return next;
      });
      addLog('info', `Connection request sent to ${recipientId}`);
    } catch (error: any) {
      addLog('error', `Failed to send connection request: ${error.message}`);
    }
  }, [addLog]);

  const acceptConnection = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    try {
      await protocolRef.current.acceptConnectionRequest({
        recipient: peerId,
        accepterName: userNameRef.current,
      });
      setNeighbors(prev => {
        const next = new Map(prev);
        const existing = next.get(peerId);
        if (existing) {
          next.set(peerId, {...existing, connectionStatus: 'accepted'});
        }
        return next;
      });
      addLog('info', `Connection accepted for ${peerId}`);
    } catch (error: any) {
      addLog('error', `Failed to accept connection: ${error.message}`);
    }
  }, [addLog]);

  const rejectConnection = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    try {
      await protocolRef.current.rejectConnectionRequest({recipient: peerId});
      setNeighbors(prev => {
        const next = new Map(prev);
        const existing = next.get(peerId);
        if (existing) {
          next.set(peerId, {...existing, connectionStatus: 'rejected'});
        }
        return next;
      });
      addLog('info', `Connection rejected for ${peerId}`);
    } catch (error: any) {
      addLog('error', `Failed to reject connection: ${error.message}`);
    }
  }, [addLog]);

  const cancelConnectionRequest = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    try {
      await protocolRef.current.cancelConnectionRequest({recipient: peerId});
      setNeighbors(prev => {
        const next = new Map(prev);
        const existing = next.get(peerId);
        if (existing) {
          next.set(peerId, {...existing, connectionStatus: 'none'});
        }
        return next;
      });
      addLog('info', `Connection request cancelled for ${peerId}`);
    } catch (error: any) {
      addLog('error', `Failed to cancel connection request: ${error.message}`);
    }
  }, [addLog]);

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
    sendConnectionRequest,
    acceptConnection,
    rejectConnection,
    cancelConnectionRequest,
  };

  return (
    <ProtocolContext.Provider value={value}>
      {children}
    </ProtocolContext.Provider>
  );
}
