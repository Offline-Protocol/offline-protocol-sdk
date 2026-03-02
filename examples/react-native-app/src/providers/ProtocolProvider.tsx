import React, {
  createContext,
  useContext,
  useCallback,
  useState,
  useEffect,
  useRef,
} from 'react';
import { Alert } from 'react-native';
import {
  MessagePriority,
  type ProtocolEvent,
  type OfflineProtocol,
  type TransportType,
  type SendFileParams,
  type SendMediaParams,
  type MediaMetadata,
  type InternetTransportConfig,
  type WifiDirectTransportConfig,
} from '@offline-protocol/mesh-sdk';

interface OfflineProtocolWithMls extends OfflineProtocol {
  isMlsInitialized(): Promise<boolean>;
}
import { useOfflineProtocol } from '../hooks/useOfflineProtocol';
import { generateUserId } from '../utils/user';
import {
  DEFAULT_RELAY_SERVER_URL,
  HARDCODED_TOKEN,
  MAX_PRESENCE_SENDS_PER_TICK,
  PRESENCE_MESSAGE_PREFIX,
  PRESENCE_REBROADCAST_INTERVAL_MS,
  PROCESSED_MESSAGE_RETENTION_MS,
} from '../constants';

const buildEventProcessingKey = (event: Record<string, unknown>): string => {
  const eventType = String(event.type ?? '');
  const messageId = String(event.message_id ?? '');
  const seenAt = String(event.seenAt ?? '');
  const timestamp = String(event.timestamp ?? '');
  const peerHint = String(
    event.sender ??
      event.peer_id ??
      event.accepted_by ??
      event.rejected_by ??
      event.recipient ??
      '',
  );
  return `${eventType}|${messageId}|${seenAt}|${timestamp}|${peerHint}`;
};

// Relay (WebSocket) types – used for groups and online messaging
export interface OnlineMessage {
  id: string;
  sender: string;
  content: string;
  timestamp: Date;
  isFromMe: boolean;
  replyToMsg?: string;
}
export interface OnlineUser {
  userId: string;
  username: string;
  isOnline: boolean;
  lastSeen?: Date;
}
export interface OnlineGroup {
  groupId: string;
  name: string;
  createdAt: Date;
}
export interface GroupMember {
  userId: string;
  role: 'admin' | 'member';
  joinedAt: Date;
}
export interface GroupDetails {
  groupId: string;
  name: string;
  createdBy: string;
  createdAt: Date;
  members: GroupMember[];
}
export type ConnectionStatus =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'authenticated'
  | 'error';

// Internal: relay server message shapes (for handleMessage)
interface PendingSentMessage {
  content: string;
  replyToMsg?: string;
  timestamp: Date;
  groupId: string;
}
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

/** Incoming (they invited me) or sent (I invited them) secure-session invite */
export interface ConnectionRequest {
  id: string;
  name: string;
  direction: 'incoming' | 'sent';
  timestamp: number;
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
  isEncrypted?: boolean;
  replyToMsg?: string;
  contentType?: string;
  mediaMetadata?: {
    mimeType: string;
    fileName: string;
    fileSize: number;
    durationMs?: number;
    width?: number;
    height?: number;
    thumbnailBase64?: string;
  };
}

export interface Chat {
  id: string;
  peerId: string;
  peerName: string;
  lastMessage?: Message;
  unreadCount: number;
  isOnline: boolean;
  messages: Message[];
  isEncrypted?: boolean;
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
  connectionRequests: ConnectionRequest[];
  /** Currently discovered nearby peers (from neighbor_discovered / neighbor_lost) */
  neighbors: Contact[];
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

  // MLS encryption state
  isMlsInitialized: boolean;
  encryptedPeers: Set<string>;

  // Actions
  initialize: () => Promise<boolean>;
  start: () => Promise<boolean>;
  stop: () => Promise<void>;
  sendMessage: (
    recipientId: string,
    content: string,
    priority?: MessagePriority,
    replyToMsg?: string,
  ) => Promise<void>;
  markAsRead: (chatId: string) => void;
  updateUserName: (name: string) => void;

  // Runtime controls
  refreshRuntimeState: () => Promise<void>;
  enableTransport: (
    type: TransportType,
    config?: InternetTransportConfig | WifiDirectTransportConfig,
  ) => Promise<boolean>;
  disableTransport: (type: TransportType) => Promise<boolean>;
  forceTransport: (type: TransportType) => Promise<boolean>;
  releaseTransportLock: () => Promise<void>;
  setBatteryLevel: (level: number) => Promise<boolean>;
  setRelayPriority: (priority: RelayPriorityInput) => Promise<boolean>;
  updateDorsConfig: (partial: Partial<DorsRuntimeConfig>) => Promise<boolean>;
  getTransportMetrics: (
    type: TransportType,
  ) => Promise<TransportMetricsSnapshot | null>;
  sendFile: (params: SendFileParams) => Promise<string | null>;
  sendMedia: (params: SendMediaParams) => Promise<string | null>;
  sendImage: (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata) => Promise<string | null>;
  sendVoiceNote: (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata) => Promise<string | null>;
  sendVideo: (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata) => Promise<string | null>;
  cancelFileTransfer: (fileId: string) => Promise<boolean>;
  addOptimisticMessage: (recipientId: string, message: Message) => void;
  rejectConnectionRequest: (peerId: string) => Promise<void>;
  acceptConnectionRequest: (peerId: string) => Promise<void>;
  sendConnectionRequest: (peerId: string) => Promise<void>;

  // Relay and groups (user groups list and group state in context)
  relayStatus: ConnectionStatus;
  authenticatedUser: OnlineUser | null;
  relayMessages: OnlineMessage[];
  onlineUsers: Map<string, OnlineUser>;
  /** User groups list – set from relay (UserGroups) and protocol events (user_groups, group_created) */
  groups: OnlineGroup[];
  groupDetails: Map<string, GroupDetails>;
  groupMessages: Map<string, OnlineMessage[]>;
  relayError: string | null;
  connect: () => void;
  disconnect: () => void;
  authenticate: (token: string) => boolean;
  send: (message: Record<string, unknown>) => boolean;
  relaySendMessage: (recipientId: string, content: string) => boolean;
  checkPresence: (username: string) => boolean;
  setTyping: (conversationId: string) => boolean;
  clearTyping: (conversationId: string) => boolean;
  createGroup: (name: string) => Promise<boolean>;
  sendGroupMessage: (
    groupId: string,
    content: string,
    replyToMsg?: string,
  ) => Promise<boolean>;
  addGroupMember: (groupId: string, username: string) => Promise<boolean>;
  removeGroupMember: (groupId: string, username: string) => Promise<boolean>;
  leaveGroup: (groupId: string) => Promise<boolean>;
  getGroupInfo: (groupId: string) => Promise<boolean>;
  getUserGroups: () => Promise<boolean>;
  groupSetAdmin: (groupId: string, username: string) => Promise<boolean>;
  groupRemoveAdmin: (groupId: string, username: string) => Promise<boolean>;
  groupDelete: (groupId: string) => Promise<boolean>;
  clearRelayMessages: () => void;
  clearGroupMessages: (groupId: string) => void;

  // Analytics
  getAnalytics: () => {
    totalMessages: number;
    totalContacts: number;
    averageResponseTime: number;
    networkHealth: 'excellent' | 'good' | 'fair' | 'poor';
  };
}

const ProtocolContext = createContext<ProtocolContextType | undefined>(
  undefined,
);

interface ProtocolProviderProps {
  children: React.ReactNode;
}

export function ProtocolProvider({ children }: ProtocolProviderProps) {
  const [currentUserId] = useState(() => {
    const id = generateUserId();
    console.log('[ProtocolProvider] Initializing with User ID:', id);
    return id;
  });
  const [currentUserName, setCurrentUserName] = useState('Me');
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [connectionRequests, setConnectionRequests] = useState<ConnectionRequest[]>([]);
  const [neighbors, setNeighbors] = useState<Contact[]>([]);
  const [chats, setChats] = useState<Chat[]>([]);
  const [isInitialized, setIsInitialized] = useState(false);
  const [peerProfiles, setPeerProfiles] = useState<Record<string, PeerProfile>>(
    {},
  );
  const [presenceSentPeers, setPresenceSentPeers] = useState<
    Record<string, number>
  >({});
  const processedIncomingMessageIdsRef = useRef<Map<string, number>>(new Map());

  // MLS encryption state
  const [isMlsInitialized, setIsMlsInitialized] = useState(false);
  const [encryptedPeers, setEncryptedPeers] = useState<Set<string>>(new Set());
  const encryptedPeersRef = useRef<Set<string>>(new Set());
  const contactsRef = useRef<Contact[]>([]);
  const discoveredPeersRef = useRef<Set<string>>(new Set());

  // Relay (WebSocket) state – inlined from former WebSocketRelayProvider
  const [relayStatus, setRelayStatus] = useState<ConnectionStatus>('disconnected');
  const [authenticatedUser, setAuthenticatedUser] = useState<OnlineUser | null>(null);
  const [relayMessages, setRelayMessages] = useState<OnlineMessage[]>([]);
  const [onlineUsers, setOnlineUsers] = useState<Map<string, OnlineUser>>(new Map());
  const [groups, setGroups] = useState<OnlineGroup[]>([]);
  const [groupDetails, setGroupDetails] = useState<Map<string, GroupDetails>>(new Map());
  const [groupMessages, setGroupMessages] = useState<Map<string, OnlineMessage[]>>(new Map());
  const [relayError, setRelayError] = useState<string | null>(null);
  const [typingUsers, setTypingUsers] = useState<Map<string, Set<string>>>(new Map());
  const pendingSentMessagesRef = useRef<Map<string, PendingSentMessage>>(new Map());
  const pendingSessionEstablishmentsRef = useRef<Set<string>>(new Set());
  const outgoingConnectionRequestsRef = useRef<Set<string>>(new Set());
  const incomingAcceptCooldownsRef = useRef<Map<string, number>>(new Map());
  const processedConnectionAcceptedEventsRef = useRef<Set<string>>(new Set());
  const lastProcessedEventsTopKeyRef = useRef<string | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const relayMessageIdCounter = useRef(0);

  const generateRelayMessageId = useCallback(() => {
    relayMessageIdCounter.current += 1;
    return `msg_${Date.now()}_${relayMessageIdCounter.current}`;
  }, []);

  useEffect(() => {
    encryptedPeersRef.current = encryptedPeers;
  }, [encryptedPeers]);

  useEffect(() => {
    contactsRef.current = contacts;
  }, [contacts]);

  const applyGroupCreated = useCallback((payload: { group_id: string; name: string }) => {
    setGroups(prev => [
      ...prev,
      { groupId: payload.group_id, name: payload.name, createdAt: new Date() },
    ]);
  }, []);

  const applyGroupMessageReceived = useCallback(
    (
      payload: {
        group_id: string;
        sender: string;
        content: string;
        timestamp: string;
        message_id: string;
        reply_to_msg?: string;
      },
    ) => {
      const isFromMe =
        payload.sender === authenticatedUser?.userId ||
        payload.sender === authenticatedUser?.username;
      const newMessage: OnlineMessage = {
        id: payload.message_id,
        sender: payload.sender,
        content: payload.content,
        timestamp: new Date(payload.timestamp),
        isFromMe: !!isFromMe,
        replyToMsg: payload.reply_to_msg,
      };
      setGroupMessages(prev => {
        const updated = new Map(prev);
        const existing = updated.get(payload.group_id) || [];
        const idx = existing.findIndex(m => m.id === payload.message_id);
        if (idx >= 0) {
          const next = [...existing];
          next[idx] = newMessage;
          updated.set(payload.group_id, next);
        } else {
          updated.set(payload.group_id, [...existing, newMessage]);
        }
        return updated;
      });
    },
    [authenticatedUser?.userId, authenticatedUser?.username],
  );

  const applyGroupInfo = useCallback(
    (payload: {
      group_id: string;
      name: string;
      created_by: string;
      created_at: string;
      members: Array<{ user_id: string; role: string; joined_at: string }>;
    }) => {
      const details: GroupDetails = {
        groupId: payload.group_id,
        name: payload.name,
        createdBy: payload.created_by,
        createdAt: new Date(payload.created_at),
        members: (payload.members || []).map(m => ({
          userId: m.user_id || 'unknown',
          role: (m.role as 'admin' | 'member') || 'member',
          joinedAt: m.joined_at ? new Date(m.joined_at) : new Date(),
        })),
      };
      setGroupDetails(prev => {
        const updated = new Map(prev);
        updated.set(payload.group_id, details);
        return updated;
      });
    },
    [],
  );

  const applyUserGroups = useCallback(
    (payload: {
      groups: Array<{ group_id: string; name: string; created_at: string }>;
    }) => {
      setGroups(
        payload.groups.map(g => ({
          groupId: g.group_id,
          name: g.name,
          createdAt: new Date(g.created_at),
        })),
      );
    },
    [],
  );

  const applyGroupMemberAdded = useCallback(
    (payload: { group_id: string; user_id: string; added_by: string }) => {
      setGroupDetails(prev => {
        const updated = new Map(prev);
        const existing = updated.get(payload.group_id);
        if (!existing) return updated;
        if (existing.members.some(m => m.userId === payload.user_id)) return updated;
        updated.set(payload.group_id, {
          ...existing,
          members: [
            ...existing.members,
            { userId: payload.user_id, role: 'member', joinedAt: new Date() },
          ],
        });
        return updated;
      });
    },
    [],
  );

  const applyGroupMemberRemoved = useCallback(
    (payload: { group_id: string; user_id: string; removed_by: string }) => {
      setGroupDetails(prev => {
        const updated = new Map(prev);
        const existing = updated.get(payload.group_id);
        if (!existing) return updated;
        updated.set(payload.group_id, {
          ...existing,
          members: existing.members.filter(m => m.userId !== payload.user_id),
        });
        return updated;
      });
    },
    [],
  );

  const setGroupError = useCallback((reason: string) => {
    setRelayError(reason);
  }, []);

  const handleRelayMessage = useCallback(
    (event: { data?: string }) => {
      try {
        const raw = event.data ?? '{}';
        const data = JSON.parse(raw) as { type: string; [key: string]: unknown };

        switch (data.type) {
          case 'Authenticated': {
            setRelayStatus('authenticated');
            setAuthenticatedUser({
              userId: (data as any).user_id,
              username: (data as any).username,
              isOnline: true,
            });
            setRelayError(null);
            break;
          }
          case 'AuthError': {
            setRelayStatus('error');
            setRelayError((data as any).reason);
            break;
          }
          case 'MessageReceived': {
            const msg = data as any;
            setRelayMessages(prev => [
              ...prev,
              {
                id: generateRelayMessageId(),
                sender: msg.sender,
                content: msg.content,
                timestamp: new Date(msg.timestamp),
                isFromMe: false,
              },
            ]);
            break;
          }
          case 'DeliveryError': {
            const err = data as any;
            setRelayError(`Failed to deliver to ${err.recipient}: ${err.reason}`);
            break;
          }
          case 'PresenceStatus': {
            const p = data as any;
            setOnlineUsers(prev => {
              const next = new Map(prev);
              const existing = next.get(p.user_id);
              next.set(p.user_id, {
                userId: p.user_id,
                username: existing?.username ?? p.user_id,
                isOnline: p.online,
                lastSeen: existing?.lastSeen,
              });
              return next;
            });
            break;
          }
          case 'PresenceStatusWithLastSeen': {
            const p = data as any;
            const lastSeen = new Date(p.last_seen);
            setOnlineUsers(prev => {
              const next = new Map(prev);
              const existing = next.get(p.user_id);
              next.set(p.user_id, {
                userId: p.user_id,
                username: existing?.username ?? p.user_id,
                isOnline: p.online,
                lastSeen,
              });
              return next;
            });
            break;
          }
          case 'GroupMessageReceived': {
            const g = data as any;
            const newMsg: OnlineMessage = {
              id: g.message_id,
              sender: g.sender,
              content: g.content,
              timestamp: new Date(g.timestamp),
              isFromMe:
                g.sender === authenticatedUser?.userId ||
                g.sender === authenticatedUser?.username,
              replyToMsg: g.reply_to_msg,
            };
            setGroupMessages(prev => {
              const next = new Map(prev);
              const list = next.get(g.group_id) || [];
              const idx = list.findIndex((m: OnlineMessage) => m.id === g.message_id);
              if (idx >= 0) {
                const arr = [...list];
                arr[idx] = newMsg;
                next.set(g.group_id, arr);
              } else {
                next.set(g.group_id, [...list, newMsg]);
              }
              return next;
            });
            break;
          }
          case 'GroupMessageSent': {
            const g = data as any;
            const now = Date.now();
            let pending: PendingSentMessage | undefined;
            let pendingKey: string | undefined;
            for (const [k, p] of pendingSentMessagesRef.current.entries()) {
              if (p.groupId === g.group_id && now - p.timestamp.getTime() < 10000) {
                if (!pending || p.timestamp > pending.timestamp) {
                  pending = p;
                  pendingKey = k;
                }
              }
            }
            const newMsg: OnlineMessage = {
              id: g.message_id,
              sender: authenticatedUser?.username || authenticatedUser?.userId || 'me',
              content: pending?.content ?? '',
              timestamp: new Date(g.timestamp),
              isFromMe: true,
              replyToMsg: pending?.replyToMsg,
            };
            if (pendingKey) pendingSentMessagesRef.current.delete(pendingKey);
            setGroupMessages(prev => {
              const next = new Map(prev);
              const list = next.get(g.group_id) || [];
              if (!list.some((m: OnlineMessage) => m.id === g.message_id)) {
                next.set(g.group_id, [...list, newMsg]);
              }
              return next;
            });
            break;
          }
          case 'GroupCreated': {
            const g = data as any;
            applyGroupCreated({ group_id: g.group_id, name: g.name });
            break;
          }
          case 'GroupInfo': {
            const g = data as any;
            applyGroupInfo({
              group_id: g.group_id,
              name: g.name,
              created_by: g.created_by,
              created_at: g.created_at,
              members: (g.members || []).map((m: any) => ({
                user_id: m.user_id || m.username || 'unknown',
                role: m.role || 'member',
                joined_at: typeof m.joined_at === 'string' ? m.joined_at : new Date().toISOString(),
              })),
            });
            break;
          }
          case 'UserGroups': {
            const g = data as any;
            applyUserGroups({
              groups: (g.groups || []).map((x: any) => ({
                group_id: x.group_id,
                name: x.name,
                created_at: x.created_at,
              })),
            });
            break;
          }
          case 'GroupMemberAdded': {
            const g = data as any;
            applyGroupMemberAdded(g);
            break;
          }
          case 'GroupMemberRemoved': {
            const g = data as any;
            applyGroupMemberRemoved(g);
            break;
          }
          case 'GroupError': {
            setGroupError((data as any).reason ?? 'Unknown');
            break;
          }
          case 'TypingUpdate': {
            const t = data as any;
            setTypingUsers(prev => {
              const next = new Map(prev);
              const set = next.get(t.conversation_id) ?? new Set<string>();
              if (t.typing) set.add(t.user_id);
              else set.delete(t.user_id);
              next.set(t.conversation_id, set);
              return next;
            });
            break;
          }
          default:
            break;
        }
      } catch (err) {
        console.error('[ProtocolProvider] Relay message parse error', err);
      }
    },
    [
      authenticatedUser?.userId,
      authenticatedUser?.username,
      generateRelayMessageId,
      applyGroupCreated,
      applyGroupInfo,
      applyUserGroups,
      applyGroupMemberAdded,
      applyGroupMemberRemoved,
      setGroupError,
    ],
  );

  const connect = useCallback(() => {
    if (typeof WebSocket === 'undefined') return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;
    setRelayStatus('connecting');
    setRelayError(null);
    try {
      const ws = new WebSocket(DEFAULT_RELAY_SERVER_URL);
      ws.onopen = () => setRelayStatus('connected');
      ws.onmessage = handleRelayMessage;
      ws.onerror = () => {
        setRelayStatus('error');
        setRelayError(`WebSocket error. Check server at ${DEFAULT_RELAY_SERVER_URL}`);
      };
      ws.onclose = () => {
        setRelayStatus('disconnected');
        setAuthenticatedUser(null);
        wsRef.current = null;
      };
      wsRef.current = ws;
    } catch (err) {
      console.error('[ProtocolProvider] Relay connect error', err);
      setRelayStatus('error');
      setRelayError('Failed to connect to relay');
    }
  }, [handleRelayMessage]);

  const disconnect = useCallback(() => {
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }
    setRelayStatus('disconnected');
    setAuthenticatedUser(null);
    setRelayError(null);
  }, []);

  const send = useCallback((message: Record<string, unknown>) => {
    if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) return false;
    try {
      wsRef.current.send(JSON.stringify(message));
      return true;
    } catch (err) {
      console.error('[ProtocolProvider] Relay send error', err);
      return false;
    }
  }, []);

  const authenticate = useCallback(
    (token: string) => {
      if (relayStatus !== 'connected') return false;
      return send({ type: 'Authenticate', token });
    },
    [send, relayStatus],
  );

  const relaySendMessage = useCallback(
    (recipientId: string, content: string) => {
      if (relayStatus !== 'authenticated') return false;
      const ok = send({ type: 'SendMessage', recipient: recipientId, content });
      if (ok) {
        setRelayMessages(prev => [
          ...prev,
          {
            id: generateRelayMessageId(),
            sender: authenticatedUser?.userId ?? 'me',
            content,
            timestamp: new Date(),
            isFromMe: true,
          },
        ]);
      }
      return ok;
    },
    [authenticatedUser?.userId, generateRelayMessageId, send, relayStatus],
  );

  const checkPresence = useCallback(
    (username: string) => {
      if (relayStatus !== 'authenticated') return false;
      return send({ type: 'CheckPresence', username });
    },
    [send, relayStatus],
  );

  const setTyping = useCallback(
    (conversationId: string) => {
      if (relayStatus !== 'authenticated') return false;
      return send({ type: 'SetTyping', conversation_id: conversationId });
    },
    [send, relayStatus],
  );

  const clearTyping = useCallback(
    (conversationId: string) => {
      if (relayStatus !== 'authenticated') return false;
      return send({ type: 'ClearTyping', conversation_id: conversationId });
    },
    [send, relayStatus],
  );

  const clearRelayMessages = useCallback(() => {
    setRelayMessages([]);
  }, []);

  const clearGroupMessages = useCallback((groupId: string) => {
    setGroupMessages(prev => {
      const next = new Map(prev);
      next.delete(groupId);
      return next;
    });
  }, []);

  useEffect(() => {
    return () => {
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, []);

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
    sendMedia: protocolSendMedia,
    sendImage: protocolSendImage,
    sendVoiceNote: protocolSendVoiceNote,
    sendVideo: protocolSendVideo,
    cancelFileTransfer: protocolCancelFile,
  } = useOfflineProtocol({
    appId: 'offline-messenger',
    userId: currentUserId,
    transports: {
      ble: {
        enabled: true,
      },
      internet: {
        enabled: true,
        serverAddress: DEFAULT_RELAY_SERVER_URL,
        autoReconnect: true,
        authToken: HARDCODED_TOKEN || undefined,
      },
      wifiDirect: {
        enabled: true,
        deviceName: currentUserName,
        autoAccept: false,
      },
    },
    dors: {
      preferOnline: true,
      switchHysteresis: 15.0,
      switchCooldownSecs: 20,
      bleToWifiRetryThreshold: 2,
      minSuccessRateBeforeEscalation: 0.3,
      minBleSamplesBeforeSuccessRateEscalation: 5,
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
    encryption: {
      enabled: true,
      autoKeyExchange: true,
      storePending: true,
      // Connection request/accept/reject control messages are plaintext bootstrap
      // messages in the current SDK implementation.
      requireEncryption: false,
    },
    reliability: {
      ack: { defaultTimeoutMs: 15000 },
      retry: { maxRetries: 8 },
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
    [peerProfiles],
  );

  const acceptConnectionRequest = useCallback(
    async (peerId: string) => {
      if (!protocol) {
        return;
      }
      const now = Date.now();
      const previousAcceptAt = incomingAcceptCooldownsRef.current.get(peerId);
      if (
        typeof previousAcceptAt === 'number' &&
        now - previousAcceptAt < 10_000
      ) {
        return;
      }
      incomingAcceptCooldownsRef.current.set(peerId, now);
      setConnectionRequests(prev =>
        prev.filter(r => !(r.id === peerId && r.direction === 'incoming')),
      );
      try {
        const localKeyPackage = await protocol
          .mlsGetOrCreateKeyPackage()
          .catch(err => {
            console.warn(
              '[ProtocolProvider] Failed to get local key package for accept:',
              err,
            );
            return null;
          });
        await protocol.acceptConnectionRequest({
          recipient: peerId,
          accepterName: currentUserName,
          keyPackage: localKeyPackage?.keyPackageData,
        });

        // Retry briefly because key package import/propagation is asynchronous.
        for (let attempt = 0; attempt < 8; attempt += 1) {
          const state = await protocol.getEstablishmentState(peerId);
          if (state === 'HaveKeyPackage') {
            await protocol.establishSecureSession(peerId);
            break;
          }
          if (state === 'SessionPending') {
            break;
          }
          await new Promise<void>(resolve => setTimeout(() => resolve(), 750));
        }

        // Optimistically add contact on the accepter side.
        // The MLS session creator stays in Pending until the remote peer
        // confirms, so secure_session_established may fire much later.
        // Dedup in setContacts and the secure_session_established handler
        // ensures this is safe.
        setEncryptedPeers(prev => new Set(prev).add(peerId));
        setContacts(prev => {
          if (prev.some(c => c.id === peerId)) return prev;
          return [
            ...prev,
            {
              id: peerId,
              name: getPeerDisplayName(peerId),
              avatar: undefined,
              isOnline: true,
              lastSeen: now,
            },
          ];
        });
      } catch (error) {
        incomingAcceptCooldownsRef.current.delete(peerId);
        throw error;
      }
    },
    [protocol, currentUserName, getPeerDisplayName],
  );

  const rejectConnectionRequest = useCallback(
    async (peerId: string) => {
      if (protocol) {
        await protocol.rejectConnectionRequest({ recipient: peerId });
      }
      setConnectionRequests(prev =>
        prev.filter(r => r.id !== peerId),
      );
    },
    [protocol],
  );

  const sendConnectionRequest = useCallback(
    async (peerId: string) => {
      if (!protocol) {
        return;
      }
      if (pendingSessionEstablishmentsRef.current.has(peerId)) {
        return;
      }
      pendingSessionEstablishmentsRef.current.add(peerId);
      const name = getPeerDisplayName(peerId);
      console.log(
        '[ProtocolProvider] sendConnectionRequest to',
        peerId,
        'name=',
        name,
      );
      try {
        const localKeyPackage = await protocol
          .mlsGetOrCreateKeyPackage()
          .catch(err => {
            console.warn(
              '[ProtocolProvider] Failed to get local key package for request:',
              err,
            );
            return null;
          });
        outgoingConnectionRequestsRef.current.add(peerId);
        await protocol.sendConnectionRequest({
          recipient: peerId,
          senderName: currentUserName,
          keyPackage: localKeyPackage?.keyPackageData,
        });

        setConnectionRequests(prev => {
          if (prev.some(r => r.id === peerId && r.direction === 'sent')) return prev;
          return [
            ...prev.filter(r => !(r.id === peerId && r.direction === 'sent')),
            {
              id: peerId,
              name,
              direction: 'sent',
              timestamp: Date.now(),
            },
          ];
        });
      } catch (error) {
        outgoingConnectionRequestsRef.current.delete(peerId);
        throw error;
      } finally {
        pendingSessionEstablishmentsRef.current.delete(peerId);
      }
    },
    [protocol, getPeerDisplayName, currentUserName],
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

      // Mark attempted broadcasts too, so repeated failures do not spam retries.
      setPresenceSentPeers(prev => {
        const lastAttempt = prev[peerId];
        if (lastAttempt && timestamp - lastAttempt < 500) {
          return prev;
        }
        return {
          ...prev,
          [peerId]: timestamp,
        };
      });

      try {
        await protocolSendMessage(
          peerId,
          `${PRESENCE_MESSAGE_PREFIX}${JSON.stringify(payload)}`,
          MessagePriority.Low,
        );
      } catch (err) {
        console.warn(
          '[ProtocolProvider] Failed to send presence message',
          peerId,
          err,
        );
      }
    },
    [protocolSendMessage, currentUserId, currentUserName],
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
          'Bluetooth and location permissions are needed to communicate with nearby devices.',
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
        'Failed to initialize the messaging protocol. Please check permissions.',
      );
      setIsInitialized(false);
      return false;
    }
  }, [isInitialized, permissionsGranted, requestPermissions]);

  const refreshMlsStatus = useCallback(async () => {
    if (!protocol || !isOnline) {
      setIsMlsInitialized(false);
      return;
    }
    try {
      const mlsProtocol = protocol as OfflineProtocolWithMls;
      const initialized = await mlsProtocol.isMlsInitialized();
      setIsMlsInitialized(initialized);
    } catch (mlsError) {
      console.warn('[ProtocolProvider] MLS status check failed:', mlsError);
      setIsMlsInitialized(false);
    }
  }, [protocol, isOnline]);

  // Start protocol
  const start = useCallback(async (): Promise<boolean> => {
    const started = await protocolStart();
    if (!started) {
      Alert.alert('Connection Error', 'Failed to start the messaging service.');
    }
    return started;
  }, [protocolStart]);

  // Stop protocol
  const stop = useCallback(async () => {
    try {
      await protocolStop();
    } catch (err) {
      console.error('Failed to stop protocol:', err);
    }
  }, [protocolStop]);

  useEffect(() => {
    refreshMlsStatus().catch(err => {
      console.warn('[ProtocolProvider] Failed to refresh MLS status:', err);
    });
  }, [refreshMlsStatus]);

  // Send message (SDK handles MLS encryption/decryption automatically)
  const sendMessage = useCallback(
    async (
      recipientId: string,
      content: string,
      priority: MessagePriority = MessagePriority.Medium,
      replyToMsg?: string,
    ) => {
      try {
        console.log(
          `[ProtocolProvider] Sending message to ${recipientId}: "${content}" (priority: ${priority})`,
        );

        // Send message - SDK handles encryption, session creation, and
        // queuing internally via prepare_outbound_content. No need to
        // pre-check establishment state from JS; doing so adds 2-3 extra
        // native bridge round-trips (~100-600ms) on every send.
        const messageId = await protocolSendMessage(
          recipientId,
          content,
          priority,
          replyToMsg,
        );
        if (!messageId) {
          throw new Error('Message ID not returned');
        }

        const isEncrypted = isMlsInitialized && encryptedPeers.has(recipientId);
        console.log(
          `[ProtocolProvider] Message sent successfully to ${recipientId} with ID ${messageId} (encrypted: ${isEncrypted})`,
        );

        const now = Date.now();
        const newMessage: Message = {
          id: messageId, // Use the ID returned by protocolSendMessage directly
          senderId: currentUserId,
          recipientId,
          content, // Store the original plaintext content for display
          timestamp: now,
          priority,
          status: 'sending',
          isFromMe: true,
          isEncrypted,
          replyToMsg,
        };

        setChats(prevChats => {
          const existingChatIndex = prevChats.findIndex(
            chat => chat.peerId === recipientId,
          );

          if (existingChatIndex >= 0) {
            const updatedChats = [...prevChats];
            const existingChat = updatedChats[existingChatIndex];
            // Check if message already exists (deduplicate by ID)
            const messageExists = existingChat.messages.some(
              msg => msg.id === messageId,
            );
            if (messageExists) {
              console.warn(
                `[ProtocolProvider] Duplicate message ${messageId} detected when sending, skipping`,
              );
              return prevChats; // Don't add duplicate
            }
            updatedChats[existingChatIndex] = {
              ...existingChat,
              peerName: getPeerDisplayName(recipientId),
              lastMessage: newMessage,
              messages: [...existingChat.messages, newMessage],
              isEncrypted: isEncrypted || existingChat.isEncrypted,
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
            isEncrypted,
          };
          return [...prevChats, newChat];
        });

        // Do not add recipient to contacts when sending a message.
        // Contacts are only added via secure-session invite acceptance.
      } catch (err) {
        console.error('Failed to send message:', err);
        Alert.alert('Send Error', 'Failed to send message. Please try again.');
      }
    },
    [
      protocolSendMessage,
      currentUserId,
      getPeerDisplayName,
      isMlsInitialized,
      encryptedPeers,
    ],
  );

  const addOptimisticMessage = useCallback(
    (recipientId: string, message: Message) => {
      setChats(prevChats => {
        const idx = prevChats.findIndex(chat => chat.peerId === recipientId);
        if (idx >= 0) {
          const chat = prevChats[idx];
          if (chat.messages.some(m => m.id === message.id)) return prevChats;
          const updated = [...prevChats];
          updated[idx] = {
            ...chat,
            lastMessage: message,
            messages: [...chat.messages, message],
          };
          return updated;
        }
        return [
          ...prevChats,
          {
            id: recipientId,
            peerId: recipientId,
            peerName: getPeerDisplayName(recipientId),
            lastMessage: message,
            unreadCount: 0,
            isOnline: false,
            messages: [message],
          },
        ];
      });
    },
    [getPeerDisplayName],
  );

  // Mark chat as read
  const markAsRead = useCallback((chatId: string) => {
    setChats(prevChats =>
      prevChats.map(chat =>
        chat.id === chatId ? { ...chat, unreadCount: 0 } : chat,
      ),
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

    const processEventsAsync = async () => {
      const currentTopKey = buildEventProcessingKey(
        events[0] as unknown as Record<string, unknown>,
      );
      const previousTopKey = lastProcessedEventsTopKeyRef.current;
      let chronologicalEvents: ProtocolEvent[] = [];

      if (!previousTopKey) {
        chronologicalEvents = [...events].reverse();
      } else {
        const previousTopIndex = events.findIndex(event =>
          buildEventProcessingKey(event as unknown as Record<string, unknown>) ===
          previousTopKey,
        );
        if (previousTopIndex > 0) {
          chronologicalEvents = events.slice(0, previousTopIndex).reverse();
        } else if (previousTopIndex === -1) {
          // Fallback if history rotated and the previous marker is gone.
          chronologicalEvents = [...events].reverse();
        }
      }

      if (chronologicalEvents.length === 0) {
        pruneProcessedMessages();
        lastProcessedEventsTopKeyRef.current = currentTopKey;
        return;
      }

      const discoveredPeers = new Set<string>(discoveredPeersRef.current);
      const establishedPeerIds = new Set<string>([
        ...Array.from(encryptedPeersRef.current),
        ...contactsRef.current.map(contact => contact.id),
      ]);
      const receivedMessages: Message[] = [];
      const sentMessageIds = new Set<string>();
      const deliveredMessageIds = new Set<string>();
      const failedMessageIds = new Set<string>();
      const presenceUpdates = new Map<
        string,
        { name: string; timestamp: number }
      >();

      //  Get current timestamp early to use for presence throttling
      const now = Date.now();

      for (const event of chronologicalEvents) {
        const eventType = (event as { type: string }).type;
        switch (eventType) {
          case 'neighbor_discovered': {
            const peerId = (event as any).peer_id;
            if (peerId && peerId !== currentUserId) {
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
          case 'connection_request_received': {
            console.log(
              '[ProtocolProvider] connection_request_received',
              event,
            );
            const e = event as {
              sender?: string;
              sender_name?: string;
              timestamp?: number;
              key_package?: number[];
            };
            const peerId = e.sender ?? (event as any).peer_id;
            const name =
              e.sender_name ??
              (peerId && peerId.length > 4 ? `User ${peerId.slice(-4)}` : peerId ?? '');
            if (peerId && peerId !== currentUserId) {
              if (e.key_package && e.key_package.length > 0) {
                await protocol?.mlsImportKeyPackage(peerId, e.key_package).catch(err => {
                  console.warn(
                    '[ProtocolProvider] Failed to import key package from connection request',
                    { peerId, err },
                  );
                });
              }
              const acceptedAt = incomingAcceptCooldownsRef.current.get(peerId);
              if (typeof acceptedAt === 'number') {
                if (Date.now() - acceptedAt < 10_000) {
                  break;
                }
                incomingAcceptCooldownsRef.current.delete(peerId);
              }
              if (
                encryptedPeersRef.current.has(peerId)
              ) {
                break;
              }
              setConnectionRequests(prev => {
                if (prev.some(r => r.id === peerId && r.direction === 'incoming')) return prev;
                return [
                  ...prev.filter(r => !(r.id === peerId && r.direction === 'incoming')),
                  {
                    id: peerId,
                    name: name || getPeerDisplayName(peerId),
                    direction: 'incoming',
                    timestamp: typeof e.timestamp === 'number' ? e.timestamp : Date.now(),
                  },
                ];
              });
            }
            break;
          }
          case 'connection_accepted': {
            const e = event as {
              accepted_by?: string;
              accepted_by_name?: string;
              timestamp?: number;
              key_package?: number[];
            };
            const peerId = e.accepted_by;
            if (peerId && peerId !== currentUserId) {
              const acceptedEventKey = `${peerId}:${String(
                e.timestamp ?? (event as any).seenAt ?? '',
              )}`;
              if (
                processedConnectionAcceptedEventsRef.current.has(acceptedEventKey)
              ) {
                break;
              }
              processedConnectionAcceptedEventsRef.current.add(acceptedEventKey);
              const hasOutgoingRequest =
                outgoingConnectionRequestsRef.current.has(peerId);
              if (!hasOutgoingRequest && encryptedPeersRef.current.has(peerId)) {
                break;
              }
              if (!hasOutgoingRequest && !encryptedPeersRef.current.has(peerId)) {
                console.log(
                  '[ProtocolProvider] Processing late connection_accepted for',
                  peerId,
                );
              }
              outgoingConnectionRequestsRef.current.delete(peerId);
              setConnectionRequests(prev =>
                prev.filter(r => r.id !== peerId),
              );
              if (e.key_package && e.key_package.length > 0) {
                await protocol?.mlsImportKeyPackage(peerId, e.key_package).catch(err => {
                  console.warn(
                    '[ProtocolProvider] Failed to import key package from connection accepted',
                    { peerId, err },
                  );
                });
              }
              for (let attempt = 0; attempt < 8; attempt += 1) {
                const state = await protocol?.getEstablishmentState(peerId);
                if (state === 'HaveKeyPackage') {
                  await protocol?.establishSecureSession(peerId);
                  break;
                }
                if (state === 'SessionPending') {
                  break;
                }
                await new Promise<void>(resolve => setTimeout(() => resolve(), 750));
              }

              // Optimistically add contact on the sender side too.
              // secure_session_established may take time if we need to
              // process the peer's Welcome first.
              establishedPeerIds.add(peerId);
              setEncryptedPeers(prev => new Set(prev).add(peerId));
              setContacts(prev => {
                if (prev.some(c => c.id === peerId)) return prev;
                return [
                  ...prev,
                  {
                    id: peerId,
                    name: getPeerDisplayName(peerId),
                    avatar: undefined,
                    isOnline: discoveredPeers.has(peerId),
                    lastSeen: now,
                  },
                ];
              });
            }
            break;
          }
          case 'connection_rejected': {
            const e = event as { rejected_by?: string };
            const peerId = e.rejected_by;
            if (peerId) {
              incomingAcceptCooldownsRef.current.delete(peerId);
              outgoingConnectionRequestsRef.current.delete(peerId);
              setConnectionRequests(prev =>
                prev.filter(r => r.id !== peerId),
              );
            }
            break;
          }
          case 'message_sent': {
            const sentEvent = event as any;
            if (sentEvent.sender === currentUserId && sentEvent.message_id) {
              const messageId = sentEvent.message_id;
              sentMessageIds.add(messageId);
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
          case 'secure_session_established': {
            const secureEvent = event as { peer_id?: string };
            if (secureEvent.peer_id) {
              const peerId = secureEvent.peer_id;
              establishedPeerIds.add(peerId);
              incomingAcceptCooldownsRef.current.delete(peerId);
              outgoingConnectionRequestsRef.current.delete(peerId);
              setEncryptedPeers(prev => new Set(prev).add(peerId));
              setConnectionRequests(prev => prev.filter(r => r.id !== peerId));
              setContacts(prev => {
                if (prev.some(c => c.id === peerId)) return prev;
                return [
                  ...prev,
                  {
                    id: peerId,
                    name: getPeerDisplayName(peerId),
                    avatar: undefined,
                    isOnline: discoveredPeers.has(peerId),
                    lastSeen: now,
                  },
                ];
              });
            }
            break;
          }
          case 'secure_session_failed': {
            const secureEvent = event as { peer_id?: string; reason?: string };
            const peerId = secureEvent.peer_id;
            if (peerId) {
              incomingAcceptCooldownsRef.current.delete(peerId);
              outgoingConnectionRequestsRef.current.delete(peerId);
            }
            console.warn('[ProtocolProvider] secure_session_failed', secureEvent);
            break;
          }
          case 'welcome_send_failed':
          case 'welcome_send_expired': {
            console.warn('[ProtocolProvider] welcome delivery issue', event);
            break;
          }
          case 'message_received': {
            const msgEvent = event as any;
            if (!msgEvent) {
              break;
            }

            // Use server-generated message_id, don't generate our own
            const messageId: string = msgEvent.message_id;
            if (!messageId) {
              console.warn(
                '[ProtocolProvider] Received message without message_id, skipping',
              );
              break;
            }

            if (processedIncomingMessageIdsRef.current.has(messageId)) {
              break;
            }
            processedIncomingMessageIdsRef.current.set(messageId, Date.now());

            const rawContent =
              typeof msgEvent.content === 'string' ? msgEvent.content : '';

            // Handle presence messages
            if (rawContent.startsWith(PRESENCE_MESSAGE_PREFIX)) {
              try {
                const payload = JSON.parse(
                  rawContent.slice(PRESENCE_MESSAGE_PREFIX.length),
                );
                if (payload?.name && typeof payload.name === 'string') {
                  const presenceTimestamp =
                    Number(payload.timestamp) ||
                    msgEvent.timestamp ||
                    Date.now();
                  presenceUpdates.set(msgEvent.sender, {
                    name: payload.name,
                    timestamp: presenceTimestamp,
                  });
                }
              } catch (err) {
                console.warn(
                  '[ProtocolProvider] Failed to parse presence payload',
                  err,
                );
              }
              break;
            }

            const senderId = String(msgEvent.sender ?? '');
            if (!senderId) {
              break;
            }
            const isEncryptedPayload = Boolean(msgEvent.encrypted);
            if (!establishedPeerIds.has(senderId) && !isEncryptedPayload) {
              console.log(
                '[ProtocolProvider] Ignoring non-session message_received from',
                senderId || 'unknown sender',
              );
              break;
            }
            if (!establishedPeerIds.has(senderId) && isEncryptedPayload) {
              // Session events can occasionally lag behind message delivery events.
              establishedPeerIds.add(senderId);
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

            const isEncrypted = Boolean(msgEvent.encrypted);
            if (isEncrypted && msgEvent.sender) {
              setEncryptedPeers(prev => new Set(prev).add(msgEvent.sender));
            }

            const receivedMessage: Message = {
              id: messageId,
              senderId,
              recipientId: msgEvent.recipient ?? currentUserId,
              content: rawContent,
              timestamp: msgEvent.timestamp || Date.now(),
              priority: normalizePriority(msgEvent.priority),
              status: 'delivered',
              isFromMe: false,
              isEncrypted,
              replyToMsg: msgEvent.reply_to_msg || msgEvent.replyToMsg,
              contentType: msgEvent.content_type,
              mediaMetadata: msgEvent.media_metadata ? {
                mimeType: msgEvent.media_metadata.mime_type ?? '',
                fileName: msgEvent.media_metadata.file_name ?? '',
                fileSize: msgEvent.media_metadata.file_size ?? 0,
                durationMs: msgEvent.media_metadata.duration_ms,
                width: msgEvent.media_metadata.width,
                height: msgEvent.media_metadata.height,
                thumbnailBase64: msgEvent.media_metadata.thumbnail_base64,
              } : undefined,
            };
            receivedMessages.push(receivedMessage);
            break;
          }
          case 'group_created': {
            const e = event as unknown as { group_id: string; name: string };
            if (e.group_id && e.name) {
              applyGroupCreated({ group_id: e.group_id, name: e.name });
            }
            break;
          }
          case 'group_message_received': {
            const e = event as unknown as {
              group_id: string;
              sender: string;
              content: string;
              timestamp: string;
              message_id: string;
              reply_to_msg?: string;
            };
            if (e.group_id && e.message_id) {
              applyGroupMessageReceived(
                {
                  group_id: e.group_id,
                  sender: e.sender,
                  content: e.content ?? '',
                  timestamp: e.timestamp ?? new Date().toISOString(),
                  message_id: e.message_id,
                  reply_to_msg: e.reply_to_msg,
                },
              );
            }
            break;
          }
          case 'group_info': {
            const e = event as unknown as {
              group_id: string;
              name: string;
              created_by: string;
              created_at: string;
              members: Array<{
                user_id: string;
                role: string;
                joined_at: string;
              }>;
            };
            if (e.group_id) {
              applyGroupInfo({
                group_id: e.group_id,
                name: e.name ?? '',
                created_by: e.created_by ?? '',
                created_at:
                  typeof e.created_at === 'string'
                    ? e.created_at
                    : new Date().toISOString(),
                members: (e.members ?? []).map(m => ({
                  user_id: m.user_id ?? (m as any).username ?? 'unknown',
                  role: m.role ?? 'member',
                  joined_at:
                    typeof m.joined_at === 'string'
                      ? m.joined_at
                      : new Date().toISOString(),
                })),
              });
            }
            break;
          }
          case 'user_groups': {
            const e = event as unknown as {
              groups: Array<{
                group_id: string;
                name: string;
                created_at: string;
              }>;
            };
            if (e.groups && Array.isArray(e.groups)) {
              applyUserGroups({
                groups: e.groups.map(g => ({
                  group_id: g.group_id,
                  name: g.name,
                  created_at:
                    typeof g.created_at === 'string'
                      ? g.created_at
                      : new Date(g.created_at).toISOString(),
                })),
              });
            }
            break;
          }
          case 'group_member_added': {
            const e = event as unknown as {
              group_id: string;
              user_id: string;
              added_by: string;
            };
            if (e.group_id && e.user_id) {
              applyGroupMemberAdded(e);
            }
            break;
          }
          case 'group_member_removed': {
            const e = event as unknown as {
              group_id: string;
              user_id: string;
              removed_by: string;
            };
            if (e.group_id && e.user_id) {
              applyGroupMemberRemoved(e);
            }
            break;
          }
          case 'group_error': {
            const e = event as unknown as { reason: string };
            setGroupError(e.reason ?? 'Unknown group error');
            break;
          }
          case 'file_received': {
            const fe = event as any;
            if (!fe.file_id || !fe.sender) break;
            const senderId = fe.sender;
            const receivedFileMessage: Message = {
              id: fe.file_id,
              senderId,
              recipientId: currentUserId,
              content: fe.file_data ?? '',
              timestamp: Date.now(),
              priority: MessagePriority.Medium,
              status: 'delivered',
              isFromMe: false,
              contentType: fe.content_type || 'file',
              mediaMetadata: fe.media_metadata ? {
                mimeType: fe.media_metadata.mime_type ?? '',
                fileName: fe.media_metadata.file_name ?? fe.file_name ?? '',
                fileSize: fe.media_metadata.file_size ?? fe.file_size ?? 0,
                durationMs: fe.media_metadata.duration_ms,
                width: fe.media_metadata.width,
                height: fe.media_metadata.height,
                thumbnailBase64: fe.media_metadata.thumbnail_base64,
              } : {
                mimeType: '',
                fileName: fe.file_name ?? '',
                fileSize: fe.file_size ?? 0,
              },
            };
            receivedMessages.push(receivedFileMessage);
            break;
          }
          default:
            break;
        }
      }

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
        return peerId.length > 4
          ? `User ${peerId.slice(-4)}`
          : `User ${peerId}`;
      };

      // Contacts are only added via secure-session invite acceptance.
      // Only update existing contacts (isOnline, name, lastSeen) from discovery/presence.
      setContacts(prevContacts => {
        const contactMap = new Map<string, Contact>(
          prevContacts.map(contact => [contact.id, contact]),
        );
        let changed = false;

        const nextContacts = Array.from(contactMap.values()).map(contact => {
          const isOnline = discoveredPeers.has(contact.id);
          const profile = updatedProfiles[contact.id];
          const name = profile?.name ?? contact.name;
          const lastSeen = isOnline ? now : contact.lastSeen;
          if (
            name !== contact.name ||
            isOnline !== contact.isOnline ||
            lastSeen !== contact.lastSeen
          ) {
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
        setChats(prevChats => {
          const updatedChats = [...prevChats];

          receivedMessages.forEach(message => {
            const existingChatIndex = updatedChats.findIndex(
              chat => chat.peerId === message.senderId,
            );
            if (existingChatIndex >= 0) {
              const existingChat = updatedChats[existingChatIndex];
              // Check if message already exists (deduplicate by ID)
              const messageExists = existingChat.messages.some(
                msg => msg.id === message.id,
              );
              if (messageExists) {
                console.warn(
                  `[ProtocolProvider] Duplicate message ${message.id} detected, skipping`,
                );
                return; // Skip adding duplicate
              }
              const nextMessages = [...existingChat.messages, message];
              updatedChats[existingChatIndex] = {
                ...existingChat,
                peerName: resolvePeerName(message.senderId),
                lastMessage: message,
                unreadCount: existingChat.unreadCount + 1,
                isOnline:
                  discoveredPeers.has(message.senderId) ||
                  existingChat.isOnline,
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
        setChats(prevChats => {
          let updated = false;

          const nextChats = prevChats.map(chat => {
            let nextChat = chat;

            const profile = updatedProfiles[chat.peerId];
            if (
              profile &&
              profile.name.trim().length > 0 &&
              profile.name.trim() !== chat.peerName
            ) {
              nextChat = {
                ...nextChat,
                peerName: profile.name.trim(),
              };
              updated = true;
            }

            let messagesChanged = false;
            const nextMessages = nextChat.messages.map((message): Message => {
              if (
                failedMessageIds.has(message.id) &&
                message.status !== 'failed'
              ) {
                console.warn(
                  `[ProtocolProvider] Message ${message.id} marked as failed`,
                );
                messagesChanged = true;
                return { ...message, status: 'failed' };
              }
              if (
                deliveredMessageIds.has(message.id) &&
                message.status !== 'delivered'
              ) {
                console.log(
                  `[ProtocolProvider] Message ${message.id} marked as delivered`,
                );
                messagesChanged = true;
                return { ...message, status: 'delivered' };
              }
              if (
                sentMessageIds.has(message.id) &&
                message.status === 'sending'
              ) {
                console.log(
                  `[ProtocolProvider] Message ${message.id} marked as sent`,
                );
                messagesChanged = true;
                return { ...message, status: 'sent' };
              }
              return message;
            });

            if (messagesChanged) {
              updated = true;
              const lastMessage =
                nextMessages[nextMessages.length - 1] ?? nextChat.lastMessage;
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
      lastProcessedEventsTopKeyRef.current = currentTopKey;
      discoveredPeersRef.current = new Set(discoveredPeers);

      // Update neighbors list (currently discovered nearby peers)
      setNeighbors(
        Array.from(discoveredPeers)
          .filter(pid => pid !== currentUserId)
          .map(peerId => ({
            id: peerId,
            name: resolvePeerName(peerId),
            avatar: undefined,
            isOnline: true,
            lastSeen: now,
          })),
      );
    };

    // Run the async event processing
    processEventsAsync().catch(err => {
      console.error('[ProtocolProvider] Error processing events:', err);
    });
  }, [
    events,
    currentUserId,
    peerProfiles,
    sendPresenceToPeer,
    getPeerDisplayName,
    applyGroupCreated,
    applyGroupMessageReceived,
    applyGroupInfo,
    applyUserGroups,
    applyGroupMemberAdded,
    applyGroupMemberRemoved,
    setGroupError,
  ]);

  // Reset presence broadcast cache when the local user name changes
  useEffect(() => {
    setPresenceSentPeers(prev => {
      if (Object.keys(prev).length === 0) {
        return prev;
      }
      return {};
    });
  }, [currentUserName]);

  // Broadcast presence to established contacts that are currently nearby.
  // Only targets peers in encryptedPeers ∩ neighbors to avoid flooding BLE
  // with connection attempts to peers we merely discovered via advertisement.
  useEffect(() => {
    if (!isOnline) {
      return;
    }
    const now = Date.now();
    const nearbyPeerIds = new Set(neighbors.map(n => n.id));

    const candidatePeerIds: string[] = [];
    encryptedPeers.forEach(peerId => {
      if (peerId === currentUserId || !nearbyPeerIds.has(peerId)) {
        return;
      }
      const lastSent = presenceSentPeers[peerId];
      if (!lastSent || now - lastSent > PRESENCE_REBROADCAST_INTERVAL_MS) {
        candidatePeerIds.push(peerId);
      }
    });

    for (
      let i = 0;
      i < Math.min(candidatePeerIds.length, MAX_PRESENCE_SENDS_PER_TICK);
      i++
    ) {
      void sendPresenceToPeer(candidatePeerIds[i]);
    }
  }, [
    neighbors,
    encryptedPeers,
    presenceSentPeers,
    isOnline,
    sendPresenceToPeer,
    currentUserId,
  ]);

  // Get analytics data
  const getAnalytics = useCallback(() => {
    const totalMessages = chats.reduce(
      (sum, chat) => sum + chat.messages.length,
      0,
    );
    const totalContacts = contacts.length;

    // Calculate average response time (simplified)
    const conversations = chats.filter(chat => chat.messages.length > 1);
    const responseTimes = conversations.map(chat => {
      const messages = chat.messages.sort((a, b) => a.timestamp - b.timestamp);
      let totalTime = 0;
      let responseCount = 0;

      for (let i = 1; i < messages.length; i++) {
        if (messages[i].isFromMe !== messages[i - 1].isFromMe) {
          totalTime += messages[i].timestamp - messages[i - 1].timestamp;
          responseCount++;
        }
      }

      return responseCount > 0 ? totalTime / responseCount : 0;
    });

    const averageResponseTime =
      responseTimes.length > 0
        ? responseTimes.reduce((sum, time) => sum + time, 0) /
          responseTimes.length
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

  // Group functions via mesh-sdk: call protocol then send JSON over relay
  const createGroup = useCallback(
    async (name: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupCreate(name);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] createGroup failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const sendGroupMessage = useCallback(
    async (
      groupId: string,
      content: string,
      replyToMsg?: string,
    ) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupSendMessage(
          groupId,
          content,
          replyToMsg ?? null,
        );
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] sendGroupMessage failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const addGroupMember = useCallback(
    async (groupId: string, username: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupAddMember(groupId, username);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] addGroupMember failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const removeGroupMember = useCallback(
    async (groupId: string, username: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupRemoveMember(groupId, username);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] removeGroupMember failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const leaveGroup = useCallback(
    async (groupId: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupLeave(groupId);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] leaveGroup failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const getGroupInfo = useCallback(
    async (groupId: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupGetInfo(groupId);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] getGroupInfo failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const getUserGroups = useCallback(async () => {
    if (!protocol || relayStatus !== 'authenticated') return false;
    try {
      const json = await protocol.groupGetUserGroups();
      return send(JSON.parse(json));
    } catch (e) {
      console.error('[ProtocolProvider] getUserGroups failed', e);
      return false;
    }
  }, [protocol, relayStatus, send]);

  const groupSetAdmin = useCallback(
    async (groupId: string, username: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupSetAdmin(groupId, username);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] groupSetAdmin failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const groupRemoveAdmin = useCallback(
    async (groupId: string, username: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupRemoveAdmin(groupId, username);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] groupRemoveAdmin failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const groupDelete = useCallback(
    async (groupId: string) => {
      if (!protocol || relayStatus !== 'authenticated') return false;
      try {
        const json = await protocol.groupDelete(groupId);
        return send(JSON.parse(json));
      } catch (e) {
        console.error('[ProtocolProvider] groupDelete failed', e);
        return false;
      }
    },
    [protocol, relayStatus, send],
  );

  const contextValue: ProtocolContextType = {
    isInitialized,
    isOnline,
    currentUserId,
    currentUserName,
    contacts,
    connectionRequests,
    neighbors,
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
    isMlsInitialized,
    encryptedPeers,
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
    sendMedia: protocolSendMedia,
    sendImage: protocolSendImage,
    sendVoiceNote: protocolSendVoiceNote,
    sendVideo: protocolSendVideo,
    cancelFileTransfer: protocolCancelFile,
    addOptimisticMessage,
    getAnalytics,
    rejectConnectionRequest,
    acceptConnectionRequest,
    sendConnectionRequest,
    relayStatus,
    authenticatedUser,
    relayMessages,
    onlineUsers,
    groups,
    groupDetails,
    groupMessages,
    relayError,
    connect,
    disconnect,
    authenticate,
    send,
    relaySendMessage,
    checkPresence,
    setTyping,
    clearTyping,
    createGroup,
    sendGroupMessage,
    addGroupMember,
    removeGroupMember,
    leaveGroup,
    getGroupInfo,
    getUserGroups,
    groupSetAdmin,
    groupRemoveAdmin,
    groupDelete,
    clearRelayMessages,
    clearGroupMessages,
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
