import React, { createContext, useContext, useCallback, useState, useEffect, useRef } from 'react';
import { Alert, Platform } from 'react-native';
import {
  MessagePriority,
  type ProtocolEvent,
  type OfflineProtocol,
  type TransportType,
  type SendFileParams,
  type InternetTransportConfig,
  type WifiDirectTransportConfig,
} from '@offlineprotocol/react-native';
import { useOfflineProtocol } from '../hooks/useOfflineProtocol';
import { generateUserId } from '../utils/user';
import {
  DEFAULT_RELAY_SERVER_URL,
  PRESENCE_MESSAGE_PREFIX,
  PRESENCE_REBROADCAST_INTERVAL_MS,
  PROCESSED_MESSAGE_RETENTION_MS,
} from '../constants';
import type {
  DorsRuntimeConfig,
  FileTransferState,
  NativeRelayPriority,
  RelayPriorityInput,
  TransportMetricsSnapshot,
} from '../types/runtime';

export interface Contact {
  id: string;
  name: string;
  avatar?: string;
  isOnline: boolean;
  lastSeen?: number;
  signalStrength?: number;
  distance?: 'near' | 'medium' | 'far';
}

export interface Message {
  id: string;
  senderId: string;
  recipientId: string;
  content: string;
  timestamp: number;
  priority: MessagePriority;
  status: 'sending' | 'sent' | 'delivered' | 'failed';
  isFromMe: boolean;
}

export interface Chat {
  id: string;
  peerId: string;
  peerName: string;
  lastMessage?: Message;
  unreadCount: number;
  isOnline: boolean;
  messages: Message[];
}

interface PeerProfile {
  name: string;
  updatedAt: number;
}

interface ProtocolContextType {
  // Core state
  isInitialized: boolean;
  isOnline: boolean;
  currentUserId: string;
  currentUserName: string;
  
  // Contacts and chats
  contacts: Contact[];
  chats: Chat[];
  connectedPeersCount: number;
  
  // Protocol state
  events: ProtocolEvent[];
  insights: any;
  batteryLevel: number | null;
  protocol: OfflineProtocol | null;
  activeTransports: TransportType[];
  forcedTransport: TransportType | null;
  relayPriority: NativeRelayPriority;
  dorsConfig: DorsRuntimeConfig;
  fileTransfers: FileTransferState[];
  
  // Actions
  initialize: () => Promise<boolean>;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  sendMessage: (recipientId: string, content: string, priority?: MessagePriority) => Promise<void>;
  markAsRead: (chatId: string) => void;
  updateUserName: (name: string) => void;
  
  // Runtime controls
  refreshRuntimeState: () => Promise<void>;
  enableTransport: (
    type: TransportType,
    config?: InternetTransportConfig | WifiDirectTransportConfig
  ) => Promise<boolean>;
  disableTransport: (type: TransportType) => Promise<boolean>;
  forceTransport: (type: TransportType) => Promise<boolean>;
  releaseTransportLock: () => Promise<void>;
  setBatteryLevel: (level: number) => Promise<boolean>;
  setRelayPriority: (priority: RelayPriorityInput) => Promise<boolean>;
  updateDorsConfig: (partial: Partial<DorsRuntimeConfig>) => Promise<boolean>;
  getTransportMetrics: (type: TransportType) => Promise<TransportMetricsSnapshot | null>;
  sendFile: (params: SendFileParams) => Promise<string | null>;
  cancelFileTransfer: (fileId: string) => Promise<boolean>;

  // Analytics
  getAnalytics: () => {
    totalMessages: number;
    totalContacts: number;
    averageResponseTime: number;
    networkHealth: 'excellent' | 'good' | 'fair' | 'poor';
  };
}

const ProtocolContext = createContext<ProtocolContextType | undefined>(undefined);

interface ProtocolProviderProps {
  children: React.ReactNode;
}

export function ProtocolProvider({ children }: ProtocolProviderProps) {
  const [currentUserId] = useState(() => generateUserId());
  const [currentUserName, setCurrentUserName] = useState('Me');
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [chats, setChats] = useState<Chat[]>([]);
  const [isInitialized, setIsInitialized] = useState(false);
  const [peerProfiles, setPeerProfiles] = useState<Record<string, PeerProfile>>({});
  const [presenceSentPeers, setPresenceSentPeers] = useState<Record<string, number>>({});
  const processedIncomingMessageIdsRef = useRef<Map<string, number>>(new Map());

  const {
    protocol,
    isStarted: isOnline,
    error,
    events,
    insights,
    permissionsGranted,
    batteryLevel,
    start: protocolStart,
    stop: protocolStop,
    sendMessage: protocolSendMessage,
    requestPermissions,
    activeTransports,
    forcedTransport,
    relayPriority,
    dorsConfig,
    fileTransfers,
    refreshRuntimeState,
    enableTransport,
    disableTransport,
    forceTransport,
    releaseTransportLock,
    setBatteryLevel: setBatteryLevelRuntime,
    setRelayPriority: setRelayPriorityRuntime,
    updateDorsConfig: updateDorsConfigRuntime,
    getTransportMetrics,
    sendFile: protocolSendFile,
    cancelFileTransfer: protocolCancelFile,
  } = useOfflineProtocol({
    appId: 'offline-messenger',
    userId: currentUserId,
    transports: {
      ble: {
        enabled: true,
      },
      internet: {
        enabled: false,
        serverAddress: DEFAULT_RELAY_SERVER_URL,
        autoReconnect: true,
      },
      wifiDirect: {
        enabled: false,
        deviceName: currentUserName,
        autoAccept: false,
      },
    },
    dors: {
      preferOnline: false,
      switchHysteresis: 15.0,
      switchCooldownSecs: 20,
      bleToWifiRetryThreshold: 2,
      rssiSwitchThreshold: -85,
      congestionQueueThreshold: 50,
      stabilityWindowSecs: 8,
      poorSignalDurationSecs: 10,
      ttlEscalationThreshold: 2,
      congestionDurationSecs: 10,
      ttlEscalationHoldSecs: 20,
      historyWindowSize: 10,
      queueRecoveryRatio: 0.5,
    },
    relay: {
      allowRelay: true,
      relayPriority: 'auto',
    },
  });

  const getPeerDisplayName = useCallback(
    (peerId: string) => {
      const profile = peerProfiles[peerId];
      if (profile && profile.name.trim().length > 0) {
        return profile.name.trim();
      }
      return peerId.length > 4 ? `User ${peerId.slice(-4)}` : `User ${peerId}`;
    },
    [peerProfiles]
  );

  const sendPresenceToPeer = useCallback(
    async (peerId: string) => {
      if (peerId === currentUserId) {
        return;
      }

      const timestamp = Date.now();
      const payload = {
        type: 'presence',
        name: currentUserName,
        userId: currentUserId,
        timestamp,
      };

      try {
        const result = await protocolSendMessage(
          peerId,
          `${PRESENCE_MESSAGE_PREFIX}${JSON.stringify(payload)}`,
          MessagePriority.Low
        );

        if (result) {
          setPresenceSentPeers((prev) => {
            const lastSent = prev[peerId];
            if (lastSent && timestamp - lastSent < 500) {
              return prev;
            }
            return {
              ...prev,
              [peerId]: timestamp,
            };
          });
        }
      } catch (err) {
        console.warn('[ProtocolProvider] Failed to send presence message', peerId, err);
      }
    },
    [protocolSendMessage, currentUserId, currentUserName]
  );

  // Initialize protocol
  const initialize = useCallback(async (): Promise<boolean> => {
    if (isInitialized && permissionsGranted) {
      return true;
    }

    try {
      const granted = await requestPermissions();
      if (!granted) {
        Alert.alert(
          'Permissions Required',
          'Bluetooth and location permissions are needed to communicate with nearby devices.'
        );
        setIsInitialized(false);
        return false;
      }

      setIsInitialized(true);
      return true;
    } catch (err) {
      console.error('Failed to initialize protocol:', err);
      Alert.alert(
        'Initialization Error',
        'Failed to initialize the messaging protocol. Please check permissions.'
      );
      setIsInitialized(false);
      return false;
    }
  }, [isInitialized, permissionsGranted, requestPermissions]);

  // Start protocol
  const start = useCallback(async () => {
    try {
      await protocolStart();
    } catch (err) {
      console.error('Failed to start protocol:', err);
      Alert.alert('Connection Error', 'Failed to start the messaging service.');
    }
  }, [protocolStart]);

  // Stop protocol
  const stop = useCallback(async () => {
    try {
      await protocolStop();
    } catch (err) {
      console.error('Failed to stop protocol:', err);
    }
  }, [protocolStop]);

  // Send message
  const sendMessage = useCallback(async (
    recipientId: string, 
    content: string, 
    priority: MessagePriority = MessagePriority.Medium
  ) => {
    try {
      console.log(`[ProtocolProvider] Sending message to ${recipientId}: "${content}" (priority: ${priority})`);
      const messageId = await protocolSendMessage(recipientId, content, priority);
      if (!messageId) {
        throw new Error('Message ID not returned');
      }
      console.log(`[ProtocolProvider] Message queued successfully to ${recipientId} with ID ${messageId}`);

      const now = Date.now();
      const newMessage: Message = {
        id: messageId,
        senderId: currentUserId,
        recipientId,
        content,
        timestamp: now,
        priority,
        status: 'sending',
        isFromMe: true,
      };

      setChats(prevChats => {
        const existingChatIndex = prevChats.findIndex(chat => chat.peerId === recipientId);
        
        if (existingChatIndex >= 0) {
          const updatedChats = [...prevChats];
          const existingChat = updatedChats[existingChatIndex];
          updatedChats[existingChatIndex] = {
            ...existingChat,
            peerName: getPeerDisplayName(recipientId),
            lastMessage: newMessage,
            messages: [...existingChat.messages, newMessage],
          };
          return updatedChats;
        }

        // Create new chat
        const newChat: Chat = {
          id: recipientId,
          peerId: recipientId,
          peerName: getPeerDisplayName(recipientId),
          lastMessage: newMessage,
          unreadCount: 0,
          isOnline: false,
          messages: [newMessage],
        };
        return [...prevChats, newChat];
      });

      setContacts((prevContacts) => {
        if (prevContacts.some((contact) => contact.id === recipientId)) {
          return prevContacts;
        }
        return [
          ...prevContacts,
          {
            id: recipientId,
            name: getPeerDisplayName(recipientId),
            avatar: undefined,
            isOnline: false,
            lastSeen: now,
          },
        ];
      });
    } catch (err) {
      console.error('Failed to send message:', err);
      Alert.alert('Send Error', 'Failed to send message. Please try again.');
    }
  }, [protocolSendMessage, currentUserId, getPeerDisplayName]);

  // Mark chat as read
  const markAsRead = useCallback((chatId: string) => {
    setChats(prevChats => 
      prevChats.map(chat => 
        chat.id === chatId ? { ...chat, unreadCount: 0 } : chat
      )
    );
  }, []);

  // Update user name
  const updateUserName = useCallback((name: string) => {
    setCurrentUserName(name);
  }, []);

  // Process protocol events to update contacts, chats, and metadata
  useEffect(() => {
    const pruneProcessedMessages = () => {
      const cutoff = Date.now() - PROCESSED_MESSAGE_RETENTION_MS;
      processedIncomingMessageIdsRef.current.forEach((seenAt, messageId) => {
        if (seenAt < cutoff) {
          processedIncomingMessageIdsRef.current.delete(messageId);
        }
      });
    };

    if (events.length === 0) {
      pruneProcessedMessages();
      return;
    }

    const chronologicalEvents = [...events].reverse();
    const discoveredPeers = new Set<string>();
    const receivedMessages: Message[] = [];
    const messageSenders = new Set<string>();
    const sentMessageIds = new Set<string>();
    const deliveredMessageIds = new Set<string>();
    const failedMessageIds = new Set<string>();
    const presenceUpdates = new Map<string, { name: string; timestamp: number }>();

    chronologicalEvents.forEach((event) => {
      switch (event.type) {
        case 'neighbor_discovered': {
          const peerId = (event as any).peer_id;
          if (peerId) {
            discoveredPeers.add(peerId);
          }
          break;
        }
        case 'neighbor_lost': {
          const peerId = (event as any).peer_id;
          if (peerId) {
            discoveredPeers.delete(peerId);
          }
          break;
        }
        case 'message_sent': {
          const sentEvent = event as any;
          if (sentEvent.sender === currentUserId && sentEvent.message_id) {
            sentMessageIds.add(sentEvent.message_id);
          }
          break;
        }
        case 'message_delivered': {
          const deliveredEvent = event as any;
          if (deliveredEvent.message_id) {
            deliveredMessageIds.add(deliveredEvent.message_id);
          }
          break;
        }
        case 'message_failed': {
          const failedEvent = event as any;
          if (failedEvent.message_id) {
            failedMessageIds.add(failedEvent.message_id);
          }
          break;
        }
        case 'message_received': {
          const msgEvent = event as any;
          if (!msgEvent) {
            break;
          }

          const messageId: string =
            msgEvent.message_id || `inbound_${msgEvent.sender}_${msgEvent.timestamp ?? Date.now()}`;

          if (processedIncomingMessageIdsRef.current.has(messageId)) {
            break;
          }
          processedIncomingMessageIdsRef.current.set(messageId, Date.now());

          const rawContent = typeof msgEvent.content === 'string' ? msgEvent.content : '';
          messageSenders.add(msgEvent.sender);

          if (rawContent.startsWith(PRESENCE_MESSAGE_PREFIX)) {
            try {
              const payload = JSON.parse(rawContent.slice(PRESENCE_MESSAGE_PREFIX.length));
              if (payload?.name && typeof payload.name === 'string') {
                const presenceTimestamp = Number(payload.timestamp) || msgEvent.timestamp || Date.now();
                presenceUpdates.set(msgEvent.sender, {
                  name: payload.name,
                  timestamp: presenceTimestamp,
                });
              }
            } catch (err) {
              console.warn('[ProtocolProvider] Failed to parse presence payload', err);
            }
            break;
          }

          const normalizePriority = (value: unknown): MessagePriority => {
            if (typeof value === 'number') {
              switch (value) {
                case MessagePriority.Low:
                  return MessagePriority.Low;
                case MessagePriority.High:
                  return MessagePriority.High;
                case MessagePriority.Critical:
                  return MessagePriority.Critical;
                case MessagePriority.Medium:
                default:
                  return MessagePriority.Medium;
              }
            }
            if (typeof value === 'string') {
              switch (value.toLowerCase()) {
                case 'low':
                  return MessagePriority.Low;
                case 'high':
                  return MessagePriority.High;
                case 'critical':
                  return MessagePriority.Critical;
                case 'medium':
                default:
                  return MessagePriority.Medium;
              }
            }
            return MessagePriority.Medium;
          };

          const receivedMessage: Message = {
            id: messageId,
            senderId: msgEvent.sender,
            recipientId: msgEvent.recipient ?? currentUserId,
            content: rawContent,
            timestamp: msgEvent.timestamp || Date.now(),
            priority: normalizePriority(msgEvent.priority),
            status: 'delivered',
            isFromMe: false,
          };
          receivedMessages.push(receivedMessage);
          break;
        }
        default:
          break;
      }
    });

    const updatedProfiles: Record<string, PeerProfile> = { ...peerProfiles };
    let profilesChanged = false;
    presenceUpdates.forEach(({ name, timestamp }, peerId) => {
      const trimmedName = name.trim();
      if (!trimmedName) {
        return;
      }
      const existing = updatedProfiles[peerId];
      if (!existing || timestamp >= existing.updatedAt) {
        updatedProfiles[peerId] = {
          name: trimmedName,
          updatedAt: timestamp,
        };
        profilesChanged = true;
      }
    });

    if (profilesChanged) {
      setPeerProfiles(updatedProfiles);
    }

    const resolvePeerName = (peerId: string) => {
      const profile = updatedProfiles[peerId];
      if (profile && profile.name.trim().length > 0) {
        return profile.name.trim();
      }
      return peerId.length > 4 ? `User ${peerId.slice(-4)}` : `User ${peerId}`;
    };

    const now = Date.now();

    setContacts((prevContacts) => {
      const contactMap = new Map<string, Contact>(prevContacts.map((contact) => [contact.id, contact]));
      let changed = false;

      discoveredPeers.forEach((peerId) => {
        if (!contactMap.has(peerId)) {
          contactMap.set(peerId, {
            id: peerId,
            name: resolvePeerName(peerId),
            avatar: undefined,
            isOnline: true,
            lastSeen: now,
            signalStrength: Math.random(),
            distance: Math.random() > 0.6 ? 'near' : Math.random() > 0.3 ? 'medium' : 'far',
          });
          changed = true;
        }
      });

      messageSenders.forEach((peerId) => {
        if (!contactMap.has(peerId)) {
          contactMap.set(peerId, {
            id: peerId,
            name: resolvePeerName(peerId),
            avatar: undefined,
            isOnline: discoveredPeers.has(peerId),
            lastSeen: now,
            signalStrength: Math.random(),
            distance: Math.random() > 0.6 ? 'near' : Math.random() > 0.3 ? 'medium' : 'far',
          });
          changed = true;
        }
      });

      const nextContacts = Array.from(contactMap.values()).map((contact) => {
        const isOnline = discoveredPeers.has(contact.id);
        const profile = updatedProfiles[contact.id];
        const name = profile?.name ?? contact.name;
        const lastSeen = isOnline ? now : contact.lastSeen;
        if (name !== contact.name || isOnline !== contact.isOnline || lastSeen !== contact.lastSeen) {
          changed = true;
          return {
            ...contact,
            name,
            isOnline,
            lastSeen,
          };
        }
        return contact;
      });

      return changed ? nextContacts : prevContacts;
    });

    if (receivedMessages.length > 0) {
      setChats((prevChats) => {
        const updatedChats = [...prevChats];

        receivedMessages.forEach((message) => {
          const existingChatIndex = updatedChats.findIndex((chat) => chat.peerId === message.senderId);
          if (existingChatIndex >= 0) {
            const existingChat = updatedChats[existingChatIndex];
            const nextMessages = [...existingChat.messages, message];
            updatedChats[existingChatIndex] = {
              ...existingChat,
              peerName: resolvePeerName(message.senderId),
              lastMessage: message,
              unreadCount: existingChat.unreadCount + 1,
              isOnline: discoveredPeers.has(message.senderId) || existingChat.isOnline,
              messages: nextMessages,
            };
          } else {
            updatedChats.push({
              id: message.senderId,
              peerId: message.senderId,
              peerName: resolvePeerName(message.senderId),
              lastMessage: message,
              unreadCount: 1,
              isOnline: discoveredPeers.has(message.senderId),
              messages: [message],
            });
          }
        });

        return updatedChats;
      });
    }

    if (
      sentMessageIds.size > 0 ||
      deliveredMessageIds.size > 0 ||
      failedMessageIds.size > 0 ||
      profilesChanged
    ) {
      setChats((prevChats) => {
        let updated = false;

        const nextChats = prevChats.map((chat) => {
          let nextChat = chat;

          const profile = updatedProfiles[chat.peerId];
          if (profile && profile.name.trim().length > 0 && profile.name.trim() !== chat.peerName) {
            nextChat = {
              ...nextChat,
              peerName: profile.name.trim(),
            };
            updated = true;
          }

          let messagesChanged = false;
          const nextMessages = nextChat.messages.map((message): Message => {
            if (failedMessageIds.has(message.id) && message.status !== 'failed') {
              console.warn(`[ProtocolProvider] Message ${message.id} marked as failed`);
              messagesChanged = true;
              return { ...message, status: 'failed' };
            }
            if (deliveredMessageIds.has(message.id) && message.status !== 'delivered') {
              console.log(`[ProtocolProvider] Message ${message.id} marked as delivered`);
              messagesChanged = true;
              return { ...message, status: 'delivered' };
            }
            if (sentMessageIds.has(message.id) && message.status === 'sending') {
              console.log(`[ProtocolProvider] Message ${message.id} marked as sent`);
              messagesChanged = true;
              return { ...message, status: 'sent' };
            }
            return message;
          });

          if (messagesChanged) {
            updated = true;
            const lastMessage = nextMessages[nextMessages.length - 1] ?? nextChat.lastMessage;
            return {
              ...nextChat,
              messages: nextMessages,
              lastMessage,
            };
          }

          return nextChat;
        });

        return updated ? nextChats : prevChats;
      });
    }

    pruneProcessedMessages();
  }, [events, currentUserId, peerProfiles, processedIncomingMessageIdsRef]);

  // Reset presence broadcast cache when the local user name changes
  useEffect(() => {
    setPresenceSentPeers((prev) => {
      if (Object.keys(prev).length === 0) {
        return prev;
      }
      return {};
    });
  }, [currentUserName]);

  // Broadcast presence to online peers periodically
  useEffect(() => {
    if (!isOnline || contacts.length === 0) {
      return;
    }
    const now = Date.now();
    contacts.forEach((contact) => {
      if (!contact.isOnline) {
        return;
      }
      const lastSent = presenceSentPeers[contact.id];
      if (!lastSent || now - lastSent > PRESENCE_REBROADCAST_INTERVAL_MS) {
        void sendPresenceToPeer(contact.id);
      }
    });
  }, [contacts, presenceSentPeers, isOnline, sendPresenceToPeer]);

  // Get analytics data
  const getAnalytics = useCallback(() => {
    const totalMessages = chats.reduce((sum, chat) => sum + chat.messages.length, 0);
    const totalContacts = contacts.length;
    
    // Calculate average response time (simplified)
    const conversations = chats.filter(chat => chat.messages.length > 1);
    const responseTimes = conversations.map(chat => {
      const messages = chat.messages.sort((a, b) => a.timestamp - b.timestamp);
      let totalTime = 0;
      let responseCount = 0;
      
      for (let i = 1; i < messages.length; i++) {
        if (messages[i].isFromMe !== messages[i-1].isFromMe) {
          totalTime += messages[i].timestamp - messages[i-1].timestamp;
          responseCount++;
        }
      }
      
      return responseCount > 0 ? totalTime / responseCount : 0;
    });
    
    const averageResponseTime = responseTimes.length > 0 
      ? responseTimes.reduce((sum, time) => sum + time, 0) / responseTimes.length 
      : 0;

    // Determine network health based on connected peers and recent activity
    const connectedPeers = contacts.filter(c => c.isOnline).length;
    let networkHealth: 'excellent' | 'good' | 'fair' | 'poor';
    
    if (connectedPeers >= 5) networkHealth = 'excellent';
    else if (connectedPeers >= 3) networkHealth = 'good';
    else if (connectedPeers >= 1) networkHealth = 'fair';
    else networkHealth = 'poor';

    return {
      totalMessages,
      totalContacts,
      averageResponseTime: Math.round(averageResponseTime / 1000), // Convert to seconds
      networkHealth,
    };
  }, [chats, contacts]);

  const connectedPeersCount = contacts.filter(c => c.isOnline).length;

  const contextValue: ProtocolContextType = {
    isInitialized,
    isOnline,
    currentUserId,
    currentUserName,
    contacts,
    chats,
    connectedPeersCount,
    events,
    insights,
    batteryLevel,
    protocol,
    activeTransports,
    forcedTransport,
    relayPriority,
    dorsConfig,
    fileTransfers,
    initialize,
    start,
    stop,
    sendMessage,
    markAsRead,
    updateUserName,
    refreshRuntimeState,
    enableTransport,
    disableTransport,
    forceTransport,
    releaseTransportLock,
    setBatteryLevel: setBatteryLevelRuntime,
    setRelayPriority: setRelayPriorityRuntime,
    updateDorsConfig: updateDorsConfigRuntime,
    getTransportMetrics,
    sendFile: protocolSendFile,
    cancelFileTransfer: protocolCancelFile,
    getAnalytics,
  };

  return (
    <ProtocolContext.Provider value={contextValue}>
      {children}
    </ProtocolContext.Provider>
  );
}

export function useProtocol() {
  const context = useContext(ProtocolContext);
  if (context === undefined) {
    throw new Error('useProtocol must be used within a ProtocolProvider');
  }
  return context;
}
