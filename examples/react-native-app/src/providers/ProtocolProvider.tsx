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
  type InternetTransportConfig,
  type WifiDirectTransportConfig,
} from '@offline-protocol/mesh-sdk';
import type {
  ConnectionRequestReceivedEvent,
  ConnectionAcceptedEvent,
  ConnectionRejectedEvent,
} from '@offline-protocol/mesh-sdk';

// MLS types (defined locally until SDK is rebuilt)
interface MlsEncryptedMessage {
  groupId: string;
  messageType: string;
  epoch: number;
  ciphertext: number[];
  senderId: string;
  timestampMs: number;
}

interface MlsWelcome {
  groupId: string;
  welcomeData: number[];
  inviterId: string;
  timestampMs: number;
}

// Extended protocol type with MLS methods
interface OfflineProtocolWithMls extends OfflineProtocol {
  initializeMlsWithSecureStorage(): Promise<void>;
  isMlsInitialized(): Promise<boolean>;
  mlsGetOrCreateKeyPackage(): Promise<{
    packageId: string;
    userId: string;
    keyPackageData: number[];
    createdAt: number;
    isSynced: boolean;
  }>;
  mlsImportKeyPackage(userId: string, keyPackageData: number[]): Promise<void>;
  mlsHasSession(otherUserId: string): Promise<boolean>;
  mlsCreateSession(otherUserId: string): Promise<MlsWelcome>;
  mlsJoinSession(welcome: MlsWelcome): Promise<{
    otherUserId: string;
    groupId: string;
    epoch: number;
    createdAt: number;
  }>;
  mlsEncryptForUser(
    otherUserId: string,
    plaintext: number[],
  ): Promise<MlsEncryptedMessage>;
  mlsDecrypt(encrypted: MlsEncryptedMessage): Promise<number[] | null>;
}
import { useOfflineProtocol } from '../hooks/useOfflineProtocol';
import { generateUserId } from '../utils/user';
import {
  DEFAULT_RELAY_SERVER_URL,
  HARDCODED_TOKEN,
  PRESENCE_MESSAGE_PREFIX,
  PRESENCE_REBROADCAST_INTERVAL_MS,
  PROCESSED_MESSAGE_RETENTION_MS,
  KEY_PACKAGE_MESSAGE_PREFIX,
  MLS_WELCOME_MESSAGE_PREFIX,
  ENCRYPTED_MESSAGE_PREFIX,
} from '../constants';
import type {
  DorsRuntimeConfig,
  FileTransferState,
  NativeRelayPriority,
  RelayPriorityInput,
  TransportMetricsSnapshot,
} from '../types/runtime';

export type ConnectionStatus =
  | 'none'
  | 'pending_sent'
  | 'pending_received'
  | 'connected'
  | 'rejected';

export interface IncomingConnectionRequest {
  sender: string;
  senderName: string;
  timestamp: number;
  keyPackage?: number[];
}

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
  isEncrypted?: boolean;
  replyToMsg?: string; // Message ID this message is replying to
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

// Relay/group types (relay connection is SDK-only; app only consumes events)
export interface OnlineMessage {
  id: string;
  sender: string;
  content: string;
  timestamp: Date;
  isFromMe: boolean;
  replyToMsg?: string;
}

export interface OnlineGroup {
  groupId: string;
  name: string;
  createdAt: Date;
}

export interface GroupMemberInfo {
  userId: string;
  role: 'admin' | 'member';
  joinedAt: Date;
}

export interface GroupDetails {
  groupId: string;
  name: string;
  createdBy: string;
  createdAt: Date;
  members: GroupMemberInfo[];
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

  // Connection request flow (request → accept/decline)
  incomingConnectionRequests: IncomingConnectionRequest[];
  getConnectionStatus: (peerId: string) => ConnectionStatus;
  sendConnectionRequest: (recipientId: string) => Promise<void>;
  acceptConnectionRequest: (senderId: string) => Promise<void>;
  rejectConnectionRequest: (senderId: string) => Promise<void>;

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

  // Relay/group state (only the SDK connects to the relay; app receives events)
  relayReady: boolean;
  groups: OnlineGroup[];
  groupDetails: Map<string, GroupDetails>;
  groupMessages: Map<string, OnlineMessage[]>;

  // Group actions (SDK sends over its single relay connection)
  createGroup: (name: string) => Promise<string>;
  sendGroupMessage: (
    groupId: string,
    content: string,
    replyToMsg?: string,
  ) => Promise<string>;
  addGroupMember: (groupId: string, username: string) => Promise<void>;
  removeGroupMember: (groupId: string, username: string) => Promise<void>;
  leaveGroup: (groupId: string) => Promise<void>;
  getGroupInfo: (groupId: string) => Promise<void>;
  getUserGroups: () => Promise<void>;

  // Actions
  initialize: () => Promise<boolean>;
  start: () => Promise<void>;
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
  cancelFileTransfer: (fileId: string) => Promise<boolean>;

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
  const [currentUserId] = useState(() => generateUserId());
  const [currentUserName, setCurrentUserName] = useState('Me');
  const [contacts, setContacts] = useState<Contact[]>([]);
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
  const peerKeyPackagesRef = useRef<Map<string, number[]>>(new Map());
  const keyPackageSentPeersRef = useRef<Set<string>>(new Set());

  // Connection request flow: request → accept/decline
  const [incomingConnectionRequests, setIncomingConnectionRequests] = useState<
    IncomingConnectionRequest[]
  >([]);
  const [connectionStatus, setConnectionStatus] = useState<
    Record<string, ConnectionStatus>
  >({});

  // Relay/group state (updated from protocol events; only SDK talks to relay)
  const [groups, setGroups] = useState<OnlineGroup[]>([]);
  const [groupDetails, setGroupDetails] = useState<Map<string, GroupDetails>>(
    new Map(),
  );
  const [groupMessages, setGroupMessages] = useState<
    Map<string, OnlineMessage[]>
  >(new Map());

  // Helper to convert string to byte array (React Native compatible)
  const stringToBytes = useCallback((str: string): number[] => {
    const bytes: number[] = [];
    for (let i = 0; i < str.length; i++) {
      const code = str.charCodeAt(i);
      if (code < 0x80) {
        bytes.push(code);
      } else if (code < 0x800) {
        bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
      } else if (code < 0x10000) {
        bytes.push(
          0xe0 | (code >> 12),
          0x80 | ((code >> 6) & 0x3f),
          0x80 | (code & 0x3f),
        );
      }
    }
    return bytes;
  }, []);

  // Helper to convert byte array to string (React Native compatible)
  const bytesToString = useCallback((bytes: number[]): string => {
    let result = '';
    let i = 0;
    while (i < bytes.length) {
      const byte = bytes[i];
      if (byte < 0x80) {
        result += String.fromCharCode(byte);
        i++;
      } else if ((byte & 0xe0) === 0xc0) {
        result += String.fromCharCode(
          ((byte & 0x1f) << 6) | (bytes[i + 1] & 0x3f),
        );
        i += 2;
      } else if ((byte & 0xf0) === 0xe0) {
        result += String.fromCharCode(
          ((byte & 0x0f) << 12) |
            ((bytes[i + 1] & 0x3f) << 6) |
            (bytes[i + 2] & 0x3f),
        );
        i += 3;
      } else {
        i++;
      }
    }
    return result;
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
      enabled: false,
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

  // Send key package to a peer for MLS session establishment
  const sendKeyPackageToPeer = useCallback(
    async (peerId: string) => {
      if (!protocol || !isMlsInitialized || peerId === currentUserId) {
        return;
      }

      // Only send key package once per peer
      if (keyPackageSentPeersRef.current.has(peerId)) {
        return;
      }

      try {
        const mlsProtocol = protocol as OfflineProtocolWithMls;
        const keyPackage = await mlsProtocol.mlsGetOrCreateKeyPackage();
        const payload = {
          type: 'keyPackage',
          userId: currentUserId,
          keyPackageData: keyPackage.keyPackageData,
          timestamp: Date.now(),
        };

        await protocolSendMessage(
          peerId,
          `${KEY_PACKAGE_MESSAGE_PREFIX}${JSON.stringify(payload)}`,
          MessagePriority.Low,
        );

        keyPackageSentPeersRef.current.add(peerId);
        console.log(`[ProtocolProvider] Sent key package to ${peerId}`);
      } catch (err) {
        console.warn(
          '[ProtocolProvider] Failed to send key package',
          peerId,
          err,
        );
      }
    },
    [protocol, isMlsInitialized, currentUserId, protocolSendMessage],
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
          MessagePriority.Low,
        );

        if (result) {
          setPresenceSentPeers(prev => {
            const lastSent = prev[peerId];
            if (lastSent && timestamp - lastSent < 500) {
              return prev;
            }
            return {
              ...prev,
              [peerId]: timestamp,
            };
          });

          // Connection is now request/accept/decline; no auto key package on presence
        }
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

  const getConnectionStatus = useCallback(
    (peerId: string): ConnectionStatus => {
      return connectionStatus[peerId] ?? 'none';
    },
    [connectionStatus],
  );

  const sendConnectionRequest = useCallback(
    async (recipientId: string) => {
      if (!protocol || recipientId === currentUserId) return;
      let keyPackage: number[] | undefined;
      if (isMlsInitialized) {
        try {
          const mlsProtocol = protocol as OfflineProtocolWithMls;
          const kp = await mlsProtocol.mlsGetOrCreateKeyPackage();
          keyPackage = kp.keyPackageData;
        } catch (e) {
          console.warn(
            '[ProtocolProvider] MLS key package for connection request:',
            e,
          );
        }
      }
      console.log(
        '[ProtocolProvider] sending connection request to',
        recipientId,
      );
      await protocol.sendConnectionRequest({
        recipient: recipientId,
        senderName: currentUserName,
        keyPackage,
      });
      setConnectionStatus(prev => ({ ...prev, [recipientId]: 'pending_sent' }));
    },
    [protocol, currentUserId, currentUserName, isMlsInitialized],
  );

  const acceptConnectionRequest = useCallback(
    async (senderId: string) => {
      if (!protocol || senderId === currentUserId) return;
      let keyPackage: number[] | undefined;
      if (isMlsInitialized) {
        try {
          const mlsProtocol = protocol as OfflineProtocolWithMls;
          const kp = await mlsProtocol.mlsGetOrCreateKeyPackage();
          keyPackage = kp.keyPackageData;
        } catch (e) {
          console.warn('[ProtocolProvider] MLS key package for accept:', e);
        }
      }
      await protocol.acceptConnectionRequest({
        recipient: senderId,
        accepterName: currentUserName,
        keyPackage,
      });
      setIncomingConnectionRequests(prev =>
        prev.filter(r => r.sender !== senderId),
      );
      setConnectionStatus(prev => ({ ...prev, [senderId]: 'connected' }));
    },
    [protocol, currentUserId, currentUserName, isMlsInitialized],
  );

  const rejectConnectionRequest = useCallback(
    async (senderId: string) => {
      if (!protocol || senderId === currentUserId) return;
      await protocol.rejectConnectionRequest({ recipient: senderId });
      setIncomingConnectionRequests(prev =>
        prev.filter(r => r.sender !== senderId),
      );
      setConnectionStatus(prev => ({ ...prev, [senderId]: 'rejected' }));
    },
    [protocol, currentUserId],
  );

  // Subscribe to relay/group events (SDK is the only relay client)
  useEffect(() => {
    if (!protocol) return;
    const onEvent = (event: ProtocolEvent) => {
      switch (event.type) {
        case 'group_message_received': {
          const e =
            event as import('@offline-protocol/mesh-sdk').GroupMessageReceivedEvent;
          const msg: OnlineMessage = {
            id: e.message_id,
            sender: e.sender,
            content: e.content,
            timestamp: new Date(e.timestamp),
            isFromMe: e.sender === currentUserId,
            replyToMsg: e.reply_to_msg_id,
          };
          setGroupMessages(prev => {
            const next = new Map(prev);
            const list = next.get(e.group_id) ?? [];
            const idx = list.findIndex(m => m.id === e.message_id);
            if (idx >= 0) {
              const copy = [...list];
              copy[idx] = msg;
              next.set(e.group_id, copy);
            } else {
              next.set(e.group_id, [...list, msg]);
            }
            return next;
          });
          break;
        }
        case 'group_created': {
          const e =
            event as import('@offline-protocol/mesh-sdk').GroupCreatedEvent;
          setGroups(prev => [
            ...prev,
            {
              groupId: e.group_id,
              name: e.name,
              createdAt: new Date(),
            },
          ]);
          break;
        }
        case 'user_groups': {
          const e =
            event as import('@offline-protocol/mesh-sdk').UserGroupsEvent;
          setGroups(
            e.groups.map(g => ({
              groupId: g.group_id,
              name: g.name,
              createdAt: new Date(g.created_at),
            })),
          );
          break;
        }
        case 'group_info': {
          const e =
            event as import('@offline-protocol/mesh-sdk').GroupInfoEvent;
          setGroupDetails(prev => {
            const next = new Map(prev);
            next.set(e.group_id, {
              groupId: e.group_id,
              name: e.name,
              createdBy: e.created_by,
              createdAt: new Date(e.created_at),
              members: e.members.map(m => ({
                userId: m.user_id,
                role: m.role,
                joinedAt: new Date(m.joined_at),
              })),
            });
            return next;
          });
          break;
        }
        case 'group_member_added':
        case 'group_member_removed':
          // Refresh group info when membership changes
          if (event.type === 'group_member_added') {
            const e =
              event as import('@offline-protocol/mesh-sdk').GroupMemberAddedEvent;
            if (protocol) void protocol.groupGetInfo(e.group_id);
          } else {
            const e =
              event as import('@offline-protocol/mesh-sdk').GroupMemberRemovedEvent;
            if (protocol) void protocol.groupGetInfo(e.group_id);
          }
          break;
        case 'group_error':
          console.warn(
            '[ProtocolProvider] Group error:',
            (event as import('@offline-protocol/mesh-sdk').GroupErrorEvent)
              .reason,
          );
          break;
        default:
          break;
      }
    };
    protocol.on('all', onEvent);
    return () => {
      protocol.off?.('all', onEvent);
    };
  }, [protocol, currentUserId]);

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

  // Start protocol
  const start = useCallback(async () => {
    try {
      await protocolStart();

      // Initialize MLS encryption after protocol starts
      if (protocol && !isMlsInitialized) {
        try {
          console.log('[ProtocolProvider] Initializing MLS encryption...');
          const mlsProtocol = protocol as OfflineProtocolWithMls;
          await mlsProtocol.initializeMlsWithSecureStorage();
          setIsMlsInitialized(true);
          console.log(
            '[ProtocolProvider] MLS encryption initialized successfully',
          );
        } catch (mlsError) {
          console.warn(
            '[ProtocolProvider] MLS initialization failed, continuing without encryption:',
            mlsError,
          );
        }
      }
    } catch (err) {
      console.error('Failed to start protocol:', err);
      Alert.alert('Connection Error', 'Failed to start the messaging service.');
    }
  }, [protocolStart, protocol, isMlsInitialized]);

  // Stop protocol
  const stop = useCallback(async () => {
    try {
      await protocolStop();
    } catch (err) {
      console.error('Failed to stop protocol:', err);
    }
  }, [protocolStop]);

  // Send message with optional encryption (only to connected peers)
  const sendMessage = useCallback(
    async (
      recipientId: string,
      content: string,
      priority: MessagePriority = MessagePriority.Medium,
      replyToMsg?: string,
    ) => {
      const status = connectionStatus[recipientId] ?? 'none';
      if (status !== 'connected') {
        throw new Error(
          'Not connected to this peer. Send a connection request and wait for them to accept.',
        );
      }
      try {
        console.log(
          `[ProtocolProvider] Sending message to ${recipientId}: "${content}" (priority: ${priority})`,
        );

        let messageToSend = content;
        let isEncrypted = false;

        // Try to encrypt the message if MLS is initialized
        if (protocol && isMlsInitialized) {
          try {
            const mlsProtocol = protocol as OfflineProtocolWithMls;
            // Check if we have a session or can create one
            const hasSession = await mlsProtocol.mlsHasSession(recipientId);

            if (!hasSession) {
              // Check if we have the peer's key package
              const peerKeyPackage =
                peerKeyPackagesRef.current.get(recipientId);
              if (peerKeyPackage) {
                console.log(
                  `[ProtocolProvider] Importing key package for ${recipientId}`,
                );
                await mlsProtocol.mlsImportKeyPackage(
                  recipientId,
                  peerKeyPackage,
                );

                // Create session and get welcome message
                const welcome = await mlsProtocol.mlsCreateSession(recipientId);

                // Send welcome message to recipient
                const welcomePayload = {
                  groupId: welcome.groupId,
                  welcomeData: welcome.welcomeData,
                  inviterId: welcome.inviterId,
                  timestampMs: welcome.timestampMs,
                };
                await protocolSendMessage(
                  recipientId,
                  `${MLS_WELCOME_MESSAGE_PREFIX}${JSON.stringify(
                    welcomePayload,
                  )}`,
                  MessagePriority.High,
                );
                console.log(
                  `[ProtocolProvider] Sent MLS welcome to ${recipientId}`,
                );
              }
            }

            // Try to encrypt if we now have a session
            const canEncrypt = await mlsProtocol.mlsHasSession(recipientId);
            if (canEncrypt) {
              const plainBytes = stringToBytes(content);
              const encrypted = await mlsProtocol.mlsEncryptForUser(
                recipientId,
                plainBytes,
              );

              // Wrap encrypted message with prefix
              const encryptedPayload = {
                groupId: encrypted.groupId,
                messageType: encrypted.messageType,
                epoch: encrypted.epoch,
                ciphertext: encrypted.ciphertext,
                senderId: encrypted.senderId,
                timestampMs: encrypted.timestampMs,
              };
              messageToSend = `${ENCRYPTED_MESSAGE_PREFIX}${JSON.stringify(
                encryptedPayload,
              )}`;
              isEncrypted = true;

              // Track this peer as encrypted
              setEncryptedPeers(prev => new Set(prev).add(recipientId));
              console.log(
                `[ProtocolProvider] Message encrypted for ${recipientId}`,
              );
            }
          } catch (encryptError) {
            console.warn(
              '[ProtocolProvider] Encryption failed, sending plaintext:',
              encryptError,
            );
          }
        }

        // Send message - SDK returns the final message ID
        const messageId = await protocolSendMessage(
          recipientId,
          messageToSend,
          priority,
          replyToMsg,
        );
        if (!messageId) {
          throw new Error('Message ID not returned');
        }
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

        setContacts(prevContacts => {
          if (prevContacts.some(contact => contact.id === recipientId)) {
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
    },
    [
      connectionStatus,
      protocolSendMessage,
      currentUserId,
      getPeerDisplayName,
      protocol,
      isMlsInitialized,
      stringToBytes,
    ],
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
      const chronologicalEvents = [...events].reverse();
      const discoveredPeers = new Set<string>();
      const newlyDiscoveredPeers = new Set<string>(); // Track peers discovered in this batch
      const receivedMessages: Message[] = [];
      const messageSenders = new Set<string>();
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
        console.log('[ProtocolProvider] processing event', event.type);
        switch (event.type) {
          case 'neighbor_discovered': {
            const peerId = (event as any).peer_id;
            if (peerId && peerId !== currentUserId) {
              discoveredPeers.add(peerId);
              //  Track newly discovered peers to send presence after event processing
              newlyDiscoveredPeers.add(peerId);
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
            const e = event as ConnectionRequestReceivedEvent;
            const timestamp =
              typeof e.timestamp === 'number'
                ? e.timestamp
                : typeof e.timestamp === 'string'
                ? parseInt(e.timestamp, 10) || Date.now()
                : Date.now();
            console.log(
              '[ProtocolProvider] connection_request_received',
              e.sender,
              e.sender_name,
            );
            setIncomingConnectionRequests(prev => [
              ...prev.filter(r => r.sender !== e.sender),
              {
                sender: e.sender,
                senderName: e.sender_name,
                timestamp,
                keyPackage: e.key_package,
              },
            ]);
            setConnectionStatus(prev => ({
              ...prev,
              [e.sender]: 'pending_received',
            }));
            if (e.key_package && e.key_package.length > 0) {
              peerKeyPackagesRef.current.set(e.sender, e.key_package);
            }
            // Ensure requester appears in contacts so we can show Accept/Decline
            setContacts(prev => {
              if (prev.some(c => c.id === e.sender)) return prev;
              return [
                ...prev,
                {
                  id: e.sender,
                  name:
                    e.sender_name ||
                    (e.sender.length > 4
                      ? `User ${e.sender.slice(-4)}`
                      : e.sender),
                  avatar: undefined,
                  isOnline: true,
                  lastSeen: now,
                  signalStrength: undefined,
                  distance: undefined,
                },
              ];
            });
            break;
          }
          case 'connection_accepted': {
            const e = event as ConnectionAcceptedEvent;
            setConnectionStatus(prev => ({
              ...prev,
              [e.accepted_by]: 'connected',
            }));
            if (e.key_package && e.key_package.length > 0) {
              peerKeyPackagesRef.current.set(e.accepted_by, e.key_package);
            }
            // Add accepter as contact on the requester's device so both devices have each other
            setContacts(prev => {
              if (prev.some(c => c.id === e.accepted_by)) return prev;
              return [
                ...prev,
                {
                  id: e.accepted_by,
                  name:
                    e.accepted_by_name ||
                    (e.accepted_by.length > 4
                      ? `User ${e.accepted_by.slice(-4)}`
                      : e.accepted_by),
                  avatar: undefined,
                  isOnline: true,
                  lastSeen: now,
                  signalStrength: undefined,
                  distance: undefined,
                },
              ];
            });
            break;
          }
          case 'connection_rejected': {
            const e = event as ConnectionRejectedEvent;
            setConnectionStatus(prev => ({
              ...prev,
              [e.rejected_by]: 'rejected',
            }));
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
            messageSenders.add(msgEvent.sender);

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

            // Handle key package messages for MLS
            if (rawContent.startsWith(KEY_PACKAGE_MESSAGE_PREFIX)) {
              try {
                const payload = JSON.parse(
                  rawContent.slice(KEY_PACKAGE_MESSAGE_PREFIX.length),
                );
                if (
                  payload?.keyPackageData &&
                  Array.isArray(payload.keyPackageData)
                ) {
                  peerKeyPackagesRef.current.set(
                    msgEvent.sender,
                    payload.keyPackageData,
                  );
                  console.log(
                    `[ProtocolProvider] Received key package from ${msgEvent.sender}`,
                  );
                }
              } catch (err) {
                console.warn(
                  '[ProtocolProvider] Failed to parse key package payload',
                  err,
                );
              }
              break;
            }

            // Handle MLS welcome messages
            if (rawContent.startsWith(MLS_WELCOME_MESSAGE_PREFIX)) {
              try {
                const payload = JSON.parse(
                  rawContent.slice(MLS_WELCOME_MESSAGE_PREFIX.length),
                );
                if (protocol && isMlsInitialized && payload?.welcomeData) {
                  const mlsProtocol = protocol as OfflineProtocolWithMls;
                  const welcome: MlsWelcome = {
                    groupId: payload.groupId,
                    welcomeData: payload.welcomeData,
                    inviterId: payload.inviterId,
                    timestampMs: payload.timestampMs,
                  };
                  await mlsProtocol.mlsJoinSession(welcome);
                  setEncryptedPeers(prev => new Set(prev).add(msgEvent.sender));
                  console.log(
                    `[ProtocolProvider] Joined MLS session with ${msgEvent.sender}`,
                  );
                }
              } catch (err) {
                console.warn(
                  '[ProtocolProvider] Failed to join MLS session',
                  err,
                );
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

            // Check if message is encrypted and try to decrypt
            let displayContent = rawContent;
            let isEncrypted = false;

            if (rawContent.startsWith(ENCRYPTED_MESSAGE_PREFIX)) {
              isEncrypted = true;
              try {
                const encryptedPayload = JSON.parse(
                  rawContent.slice(ENCRYPTED_MESSAGE_PREFIX.length),
                );
                if (protocol && isMlsInitialized) {
                  const mlsProtocol = protocol as OfflineProtocolWithMls;
                  const encrypted: MlsEncryptedMessage = {
                    groupId: encryptedPayload.groupId,
                    messageType: encryptedPayload.messageType,
                    epoch: encryptedPayload.epoch,
                    ciphertext: encryptedPayload.ciphertext,
                    senderId: encryptedPayload.senderId,
                    timestampMs: encryptedPayload.timestampMs,
                  };
                  const decryptedBytes = await mlsProtocol.mlsDecrypt(
                    encrypted,
                  );
                  if (decryptedBytes) {
                    displayContent = bytesToString(decryptedBytes);
                    console.log(
                      `[ProtocolProvider] Decrypted message from ${msgEvent.sender}`,
                    );
                  } else {
                    displayContent = '[Encrypted message - unable to decrypt]';
                    console.warn('[ProtocolProvider] Decryption returned null');
                  }
                } else {
                  displayContent = '[Encrypted message - MLS not initialized]';
                }
              } catch (decryptError) {
                console.warn(
                  '[ProtocolProvider] Failed to decrypt message:',
                  decryptError,
                );
                displayContent = '[Encrypted message - decryption failed]';
              }
            }

            const receivedMessage: Message = {
              id: messageId,
              senderId: msgEvent.sender,
              recipientId: msgEvent.recipient ?? currentUserId,
              content: displayContent,
              timestamp: msgEvent.timestamp || Date.now(),
              priority: normalizePriority(msgEvent.priority),
              status: 'delivered',
              isFromMe: false,
              isEncrypted,
              replyToMsg: msgEvent.reply_to_msg || msgEvent.replyToMsg,
            };
            receivedMessages.push(receivedMessage);
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

      setContacts(prevContacts => {
        const contactMap = new Map<string, Contact>(
          prevContacts.map(contact => [contact.id, contact]),
        );
        let changed = false;

        discoveredPeers.forEach(peerId => {
          if (!contactMap.has(peerId)) {
            contactMap.set(peerId, {
              id: peerId,
              name: resolvePeerName(peerId),
              avatar: undefined,
              isOnline: true,
              lastSeen: now,
              signalStrength: Math.random(),
              distance:
                Math.random() > 0.6
                  ? 'near'
                  : Math.random() > 0.3
                  ? 'medium'
                  : 'far',
            });
            changed = true;
          }
        });

        messageSenders.forEach(peerId => {
          if (!contactMap.has(peerId)) {
            contactMap.set(peerId, {
              id: peerId,
              name: resolvePeerName(peerId),
              avatar: undefined,
              isOnline: discoveredPeers.has(peerId),
              lastSeen: now,
              signalStrength: Math.random(),
              distance:
                Math.random() > 0.6
                  ? 'near'
                  : Math.random() > 0.3
                  ? 'medium'
                  : 'far',
            });
            changed = true;
          }
        });

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

      //  Send presence to newly discovered peers after event processing
      // This ensures usernames are synced immediately when peers are discovered
      // BUT: Only send if we haven't sent recently to prevent infinite loops
      // Use a ref to track in-flight presence sends to avoid duplicate sends
      newlyDiscoveredPeers.forEach(peerId => {
        const lastSent = presenceSentPeers[peerId];
        const timeSinceLastSent = now - (lastSent || 0);
        // Only send if we haven't sent in the last 5 seconds to prevent loops
        if (!lastSent || timeSinceLastSent > 5000) {
          // Update the timestamp immediately to prevent duplicate sends
          setPresenceSentPeers(prev => ({
            ...prev,
            [peerId]: now,
          }));
          sendPresenceToPeer(peerId).catch(err => {
            console.warn(
              `[ProtocolProvider] Failed to send presence to newly discovered peer ${peerId}:`,
              err,
            );
          });
        } else {
          console.log(
            `[ProtocolProvider] Skipping presence send to ${peerId} (sent ${timeSinceLastSent}ms ago, min interval: 5000ms)`,
          );
        }
      });
    };

    // Run the async event processing
    processEventsAsync().catch(err => {
      console.error('[ProtocolProvider] Error processing events:', err);
    });
  }, [
    events,
    currentUserId,
    peerProfiles,
    protocol,
    isMlsInitialized,
    bytesToString,
    sendPresenceToPeer,
    presenceSentPeers,
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

  // Broadcast presence to online peers periodically
  useEffect(() => {
    if (!isOnline) {
      return;
    }
    const now = Date.now();

    //  Send presence to all discovered peers, not just contacts
    // This ensures newly discovered peers get presence even before they're in contacts
    const allPeers = new Set<string>();

    // Add all contacts
    contacts.forEach(contact => {
      if (contact.isOnline) {
        allPeers.add(contact.id);
      }
    });

    // Also send to any discovered peers that might not be in contacts yet
    // (This will be populated from events in processEventsAsync)

    allPeers.forEach(peerId => {
      if (peerId === currentUserId) {
        return;
      }
      const lastSent = presenceSentPeers[peerId];
      if (!lastSent || now - lastSent > PRESENCE_REBROADCAST_INTERVAL_MS) {
        void sendPresenceToPeer(peerId);
      }
    });
  }, [
    contacts,
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

  const relayReady =
    isOnline && (activeTransports?.includes('internet') ?? false);

  const createGroup = useCallback(
    async (name: string): Promise<string> => {
      if (!protocol) throw new Error('Protocol not initialized');
      const json = await protocol.groupCreate(name);
      return json;
    },
    [protocol],
  );

  const sendGroupMessage = useCallback(
    async (
      groupId: string,
      content: string,
      replyToMsg?: string,
    ): Promise<string> => {
      if (!protocol) throw new Error('Protocol not initialized');
      const json = await protocol.groupSendMessage(
        groupId,
        content,
        replyToMsg ?? null,
      );
      return json;
    },
    [protocol],
  );

  const addGroupMember = useCallback(
    async (groupId: string, username: string): Promise<void> => {
      if (!protocol) throw new Error('Protocol not initialized');
      await protocol.groupAddMember(groupId, username);
    },
    [protocol],
  );

  const removeGroupMember = useCallback(
    async (groupId: string, username: string): Promise<void> => {
      if (!protocol) throw new Error('Protocol not initialized');
      await protocol.groupRemoveMember(groupId, username);
    },
    [protocol],
  );

  const leaveGroup = useCallback(
    async (groupId: string): Promise<void> => {
      if (!protocol) throw new Error('Protocol not initialized');
      await protocol.groupLeave(groupId);
    },
    [protocol],
  );

  const getGroupInfo = useCallback(
    async (groupId: string): Promise<void> => {
      if (!protocol) return;
      await protocol.groupGetInfo(groupId);
    },
    [protocol],
  );

  const getUserGroups = useCallback(async (): Promise<void> => {
    if (!protocol) return;
    await protocol.groupGetUserGroups();
  }, [protocol]);

  const contextValue: ProtocolContextType = {
    isInitialized,
    isOnline,
    currentUserId,
    currentUserName,
    contacts,
    chats,
    connectedPeersCount,
    incomingConnectionRequests,
    getConnectionStatus,
    sendConnectionRequest,
    acceptConnectionRequest,
    rejectConnectionRequest,
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
    relayReady,
    groups,
    groupDetails,
    groupMessages,
    createGroup,
    sendGroupMessage,
    addGroupMember,
    removeGroupMember,
    leaveGroup,
    getGroupInfo,
    getUserGroups,
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
