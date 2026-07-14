import React, {
  createContext,
  useContext,
  useState,
  useCallback,
  useRef,
  useEffect,
} from 'react';
import {
  OfflineProtocol,
  MeshServices,
  MessagePriority,
} from '@offline-protocol/mesh-sdk';
import type {
  TelemetryRecord,
  MetricsFrame,
  TransportStateTelemetryEvent,
  RoutingDecision,
  DeviceCapabilitySnapshot,
} from '@offline-protocol/mesh-sdk';
import type {Contact, Neighbor, ConnectionRequest, ChatMessage, Chat, Group, GroupRole, DiscoveredService, ServiceLogEntry, ForwardInfo, PresenceStatus} from '../types';
import {
  PRESENCE_BROADCAST_INTERVAL_MS,
  TYPING_INDICATOR_TIMEOUT_MS,
  NEARBY_THRESHOLD_MS,
  PROTOCOL_CONFIG,
} from '../constants';

// ─── Context Shape ───────────────────────────────────────────

interface ProtocolContextValue {
  // State
  protocol: OfflineProtocol | null;
  meshServices: MeshServices | null;
  isStarted: boolean;
  userId: string;
  userName: string;
  neighbors: Map<string, Neighbor>;
  contacts: Map<string, Contact>;
  connectionRequests: ConnectionRequest[];
  chats: Map<string, Chat>;
  groups: Map<string, Group>;
  registeredServices: string[];
  discoveredServices: DiscoveredService[];
  serviceLog: ServiceLogEntry[];
  /** Map of peerId → timestamp when they started typing */
  typingPeers: Map<string, number>;

  // Telemetry state (populated by installed TelemetrySink).
  // Shape is intentionally narrow: only fields that feed aggregate,
  // anonymized visualizations in DiagnosticsScreen are persisted.
  latestMetrics: MetricsFrame | null;
  metricsHistory: MetricsFrame[];
  transportTimeline: TransportStateTelemetryEvent[];
  routingDecisions: RoutingDecision[];
  deviceCapability: DeviceCapabilitySnapshot | null;
  deviceCapabilityHistory: DeviceCapabilitySnapshot[];
  /** Count of received/delivered messages keyed by observed hop_count. */
  hopCountHistogram: Record<number, number>;

  // Actions
  initialize: (userId: string, userName: string) => Promise<void>;
  sendMessage: (recipientId: string, content: string, priority?: 'medium' | 'critical') => Promise<void>;
  sendConnectionRequest: (peerId: string) => Promise<void>;
  acceptConnectionRequest: (peerId: string) => Promise<void>;
  rejectConnectionRequest: (peerId: string) => Promise<void>;
  cancelConnectionRequest: (peerId: string) => Promise<void>;
  createGroup: (name: string, memberIds: string[]) => Promise<void>;
  sendGroupMessage: (groupId: string, content: string, priority?: 'medium' | 'critical') => Promise<void>;
  leaveGroup: (groupId: string) => Promise<void>;
  inviteToGroup: (groupId: string, userId: string) => Promise<void>;
  removeFromGroup: (groupId: string, userId: string) => Promise<void>;
  setMemberRole: (groupId: string, userId: string, role: GroupRole) => Promise<void>;
  getGroupRoles: (groupId: string) => Promise<Record<string, string>>;
  getMemberRole: (groupId: string, userId: string) => GroupRole;
  registerService: (serviceId: string, version: string) => Promise<void>;
  unregisterService: (serviceId: string) => Promise<void>;
  discoverServices: (serviceId?: string) => Promise<void>;
  sendServiceRequest: (provider: string, serviceId: string, method: string, body: string) => Promise<void>;
  forwardMessage: (message: ChatMessage, recipientId: string) => Promise<void>;
  forwardMessageToGroup: (message: ChatMessage, groupId: string) => Promise<void>;
  markChatRead: (peerId: string) => void;
  blockUser: (peerId: string) => Promise<void>;
  unblockUser: (peerId: string) => Promise<void>;
  sendTypingIndicator: (recipientId: string, isTyping: boolean) => Promise<void>;
}

// Bounded-buffer caps for telemetry streams. Hoisted to module scope so they
// don't need to live in the `useCallback([])` dep array of `handleTelemetry`.
//
// Sizing rationale:
// - METRICS_HISTORY: 60 frames × 2 s cadence = 120 s. Long enough that
//   partition/transport-time distributions have signal but short enough to
//   stay fresh on a phone.
// - TIMELINE/DECISIONS: 50 is enough to compute stable link-flap and DORS
//   driver aggregates without growing unbounded under flap storms.
// - DEVICE_HISTORY: deviceCapability only emits on change; 60 samples covers
//   an entire battery cycle comfortably.
const MAX_METRICS_HISTORY = 60;
const MAX_TIMELINE = 50;
const MAX_DECISIONS = 50;
const MAX_DEVICE_HISTORY = 60;
const MAX_SERVICE_LOG = 100;

// Protocol event types whose payload carries a final-delivery `hop_count` we
// want to fold into the anonymized hop-count distribution.
const HOP_COUNT_EVENT_TYPES: ReadonlySet<string> = new Set([
  'message_received',
  'message_delivered',
]);

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
  const [meshServices, setMeshServices] = useState<MeshServices | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [userId, setUserId] = useState('');
  const [userName, setUserName] = useState('');
  const [neighbors, setNeighbors] = useState<Map<string, Neighbor>>(new Map());
  const [contacts, setContacts] = useState<Map<string, Contact>>(new Map());
  const [connectionRequests, setConnectionRequests] = useState<ConnectionRequest[]>([]);
  const [chats, setChats] = useState<Map<string, Chat>>(new Map());
  const [groups, setGroups] = useState<Map<string, Group>>(new Map());
  const [registeredServices, setRegisteredServices] = useState<string[]>([]);
  const [discoveredServices, setDiscoveredServices] = useState<DiscoveredService[]>([]);
  const [serviceLog, setServiceLog] = useState<ServiceLogEntry[]>([]);
  const [typingPeers, setTypingPeers] = useState<Map<string, number>>(new Map());

  // ─── Telemetry state ──────────────────────────────────────
  const [latestMetrics, setLatestMetrics] = useState<MetricsFrame | null>(null);
  const [metricsHistory, setMetricsHistory] = useState<MetricsFrame[]>([]);
  const [transportTimeline, setTransportTimeline] = useState<TransportStateTelemetryEvent[]>([]);
  const [routingDecisions, setRoutingDecisions] = useState<RoutingDecision[]>([]);
  const [deviceCapability, setDeviceCapability] = useState<DeviceCapabilitySnapshot | null>(null);
  const [deviceCapabilityHistory, setDeviceCapabilityHistory] = useState<DeviceCapabilitySnapshot[]>([]);
  const [hopCountHistogram, setHopCountHistogram] = useState<Record<number, number>>({});

  const appendServiceLog = useCallback((entry: ServiceLogEntry) => {
    setServiceLog(prev => {
      const next = [...prev, entry];
      return next.length > MAX_SERVICE_LOG ? next.slice(-MAX_SERVICE_LOG) : next;
    });
  }, []);

  const protocolRef = useRef<OfflineProtocol | null>(null);
  const telemetryUnsubscribeRef = useRef<(() => void) | null>(null);
  const processedMessagesRef = useRef<Set<string>>(new Set());
  const contactsRef = useRef<Map<string, Contact>>(contacts);
  const neighborsRef = useRef<Map<string, Neighbor>>(neighbors);
  const userNameRef = useRef(userName);
  const userIdRef = useRef(userId);
  const blockedUsersRef = useRef<Set<string>>(new Set());
  // Peers with an established MLS session. autoKeyExchange establishes these
  // under the hood on discovery, independent of the app-level accept — so when a
  // peer is later accepted into contacts, its hasSession can reflect a session
  // that already converged before the accept (needed for group creation).
  const sessionPeersRef = useRef<Set<string>>(new Set());

  // Keep refs in sync
  useEffect(() => { contactsRef.current = contacts; }, [contacts]);
  useEffect(() => { neighborsRef.current = neighbors; }, [neighbors]);
  useEffect(() => { userNameRef.current = userName; }, [userName]);
  useEffect(() => { userIdRef.current = userId; }, [userId]);

  const parseForwardInfo = (event: any): ForwardInfo | undefined => {
    const fi = event.forward_info || event.forwardInfo;
    if (!fi) {return undefined;}
    return {
      originalSender: fi.original_sender || fi.originalSender || '',
      originalMessageId: fi.original_message_id || fi.originalMessageId || '',
      originalTimestamp: fi.original_timestamp || fi.originalTimestamp || 0,
      forwardCount: fi.forward_count || fi.forwardCount || 1,
    };
  };

  const buildRawEventJson = (event: any): string => {
    // Build a JSON that matches the Rust `Message` struct for forwarding.
    // The Rust layer deserializes this into `offline_protocol_core::Message`,
    // which requires fields: id, sender, recipient, app_id, priority, ttl,
    // hop_count, timestamp, content (and optionally forwarded_from).
    const msgId = event.message_id || event.messageId || event.id;
    const fwd = event.forwarded_from || event.forward_info || event.forwardInfo;
    const obj: any = {
      id: msgId,
      sender: event.sender || event.senderId || event.sender_id,
      recipient: event.recipient || event.recipientId || event.recipient_id || userIdRef.current,
      app_id: PROTOCOL_CONFIG.appId,
      priority: (event.priority || 'medium').toLowerCase(),
      ttl: event.ttl ?? 8,
      hop_count: event.hop_count ?? 0,
      timestamp: event.timestamp || Date.now(),
      content: event.content || event.message || '',
    };
    if (fwd) {
      obj.forwarded_from = {
        original_sender: fwd.original_sender || fwd.originalSender,
        original_message_id: fwd.original_message_id || fwd.originalMessageId,
        original_timestamp: fwd.original_timestamp || fwd.originalTimestamp,
        forward_count: fwd.forward_count ?? fwd.forwardCount ?? 1,
      };
    }
    return JSON.stringify(obj);
  };

  // ─── Event Handlers ──────────────────────────────────────

  const handleEvent = useCallback((event: any) => {
    const eventType = event.type;

    switch (eventType) {
      case 'neighbor_discovered': {
        const peerId = event.peerId || event.peer_id;
        console.log('[ProtocolContext] neighbor_discovered peerId:', peerId);
        if (!peerId || blockedUsersRef.current.has(peerId)) {break;}
        setNeighbors(prev => {
          const next = new Map(prev);
          next.set(peerId, {
            peerId,
            transport: event.transport || 'ble',
            rssi: event.rssi,
            discoveredAt: Date.now(),
          });
          return next;
        });
        // Also update contact presence if they exist
        setContacts(prev => {
          if (!prev.has(peerId)) {return prev;}
          const next = new Map(prev);
          const contact = next.get(peerId)!;
          next.set(peerId, {...contact, isNearby: true, lastSeen: Date.now()});
          return next;
        });
        break;
      }

      case 'neighbor_lost': {
        const peerId = event.peerId || event.peer_id;
        if (!peerId) {break;}
        setNeighbors(prev => {
          const next = new Map(prev);
          next.delete(peerId);
          return next;
        });
        setContacts(prev => {
          if (!prev.has(peerId)) {return prev;}
          const next = new Map(prev);
          const contact = next.get(peerId)!;
          next.set(peerId, {...contact, isNearby: false});
          return next;
        });
        break;
      }

      case 'connection_request_received': {
        const peerId = event.sender || event.peerId || event.peer_id || event.fromUserId || event.from_user_id;
        if (!peerId || blockedUsersRef.current.has(peerId)) {break;}
        const peerName = event.sender_name || event.userName || event.user_name || peerId;
        // Design: ALWAYS require an explicit manual Accept — never
        // auto-accept, even on a mutual request. Just record the incoming
        // request so it appears in the pending list for the user to accept.
        setConnectionRequests(prev => {
          if (prev.some(r => r.peerId === peerId && r.direction === 'in')) {return prev;}
          return [...prev, {
            peerId,
            name: peerName,
            direction: 'in',
            timestamp: Date.now(),
          }];
        });
        break;
      }

      case 'connection_accepted': {
        const peerId = event.accepted_by || event.peerId || event.peer_id || event.byUserId || event.by_user_id;
        console.log('[ProtocolContext] connection_accepted peerId:', peerId);
        if (!peerId) {break;}
        setConnectionRequests(prev => prev.filter(r => r.peerId !== peerId));
        setContacts(prev => {
          const next = new Map(prev);
          const existing = next.get(peerId);
          next.set(peerId, {
            peerId,
            name: existing?.name || event.accepted_by_name || event.userName || event.user_name || peerId,
            lastSeen: Date.now(),
            isNearby: neighborsRef.current.has(peerId),
            hasSession: existing?.hasSession || sessionPeersRef.current.has(peerId),
            isBlocked: false,
            presenceStatus: existing?.presenceStatus || 'offline',
          });
          return next;
        });
        break;
      }

      case 'connection_rejected': {
        const peerId = event.rejected_by || event.peerId || event.peer_id || event.byUserId || event.by_user_id;
        if (!peerId) {break;}
        setConnectionRequests(prev => prev.filter(r => r.peerId !== peerId));
        break;
      }

      case 'connection_request_cancelled': {
        const peerId = event.cancelled_by || event.peerId || event.peer_id;
        if (!peerId) {break;}
        setConnectionRequests(prev => prev.filter(r => r.peerId !== peerId));
        break;
      }

      case 'secure_session_established': {
        const peerId = event.peerId || event.peer_id || event.otherUserId || event.other_user_id;
        console.log('[ProtocolContext] secure_session_established raw event:', JSON.stringify(event), 'resolved peerId:', peerId);
        if (!peerId) {break;}
        // Record the session regardless of contact status — the peer may not be
        // an accepted contact yet (autoKeyExchange establishes on discovery).
        sessionPeersRef.current.add(peerId);
        // Design: a secure session establishing under the hood must NOT
        // auto-create a "connected" contact. Only mark hasSession on a contact
        // the user has already connected to via an accepted request. The MLS
        // session still exists; the peer stays unconnected in the UI until Accept.
        setContacts(prev => {
          const existing = prev.get(peerId);
          if (!existing) {return prev;}
          const next = new Map(prev);
          next.set(peerId, {...existing, hasSession: true, lastSeen: Date.now()});
          return next;
        });
        break;
      }

      case 'message_received': {
        const msgId = event.messageId || event.message_id || event.id;
        const senderId = event.sender || event.senderId || event.sender_id || event.fromUserId || event.from_user_id;
        const content = event.content || event.message || '';

        if (!senderId || !msgId) {break;}
        if (blockedUsersRef.current.has(senderId)) {break;}
        if (processedMessagesRef.current.has(msgId)) {break;}
        processedMessagesRef.current.add(msgId);

        // Regular chat message
        const chatMsg: ChatMessage = {
          id: msgId,
          senderId,
          content,
          timestamp: event.timestamp || Date.now(),
          status: 'delivered',
          isOutgoing: false,
          forwardInfo: parseForwardInfo(event),
          rawEventJson: buildRawEventJson(event),
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

        // Update contact last seen
        setContacts(prev => {
          if (!prev.has(senderId)) {return prev;}
          const next = new Map(prev);
          const contact = next.get(senderId)!;
          next.set(senderId, {...contact, lastSeen: Date.now(), isNearby: true});
          return next;
        });
        break;
      }

      case 'message_sent': {
        const msgId = event.messageId || event.message_id;
        if (!msgId) {break;}
        setChats(prev => {
          const next = new Map(prev);
          for (const [peerId, chat] of next) {
            const msgIndex = chat.messages.findIndex(m => m.id === msgId);
            if (msgIndex >= 0) {
              const msgs = [...chat.messages];
              msgs[msgIndex] = {...msgs[msgIndex], status: 'sent'};
              next.set(peerId, {...chat, messages: msgs});
              break;
            }
          }
          return next;
        });
        break;
      }

      case 'message_delivered': {
        const msgId = event.messageId || event.message_id;
        if (!msgId) {break;}
        setChats(prev => {
          const next = new Map(prev);
          for (const [peerId, chat] of next) {
            const msgIndex = chat.messages.findIndex(m => m.id === msgId);
            if (msgIndex >= 0) {
              const msgs = [...chat.messages];
              msgs[msgIndex] = {...msgs[msgIndex], status: 'delivered'};
              next.set(peerId, {...chat, messages: msgs});
              break;
            }
          }
          return next;
        });
        setGroups(prev => {
          const next = new Map(prev);
          for (const [groupId, group] of next) {
            const msgIndex = group.messages.findIndex(m => m.id === msgId);
            if (msgIndex >= 0) {
              const msgs = [...group.messages];
              msgs[msgIndex] = {...msgs[msgIndex], status: 'delivered'};
              next.set(groupId, {...group, messages: msgs});
              break;
            }
          }
          return next;
        });
        break;
      }

      case 'message_failed': {
        const msgId = event.messageId || event.message_id;
        if (!msgId) {break;}
        setChats(prev => {
          const next = new Map(prev);
          for (const [peerId, chat] of next) {
            const msgIndex = chat.messages.findIndex(m => m.id === msgId);
            if (msgIndex >= 0) {
              const msgs = [...chat.messages];
              msgs[msgIndex] = {...msgs[msgIndex], status: 'failed'};
              next.set(peerId, {...chat, messages: msgs});
              break;
            }
          }
          return next;
        });
        setGroups(prev => {
          const next = new Map(prev);
          for (const [groupId, group] of next) {
            const msgIndex = group.messages.findIndex(m => m.id === msgId);
            if (msgIndex >= 0) {
              const msgs = [...group.messages];
              msgs[msgIndex] = {...msgs[msgIndex], status: 'failed'};
              next.set(groupId, {...group, messages: msgs});
              break;
            }
          }
          return next;
        });
        break;
      }

      case 'group_created': {
        const groupId = event.groupId || event.group_id;
        const groupName = event.groupName || event.group_name || event.name || 'Group';
        if (!groupId) {break;}
        setGroups(prev => {
          const next = new Map(prev);
          if (!next.has(groupId)) {
            const members = event.members || [userIdRef.current];
            next.set(groupId, {
              id: groupId,
              name: groupName,
              members,
              roles: {[userIdRef.current]: 'admin'},
              messages: [],
            });
          }
          return next;
        });
        break;
      }

      case 'group_message_received': {
        const groupId = event.groupId || event.group_id;
        const msgId = event.messageId || event.message_id || event.id;
        const senderId = event.sender || event.senderId || event.sender_id;
        const content = event.content || event.message || '';
        if (!groupId || !msgId) {break;}
        if (blockedUsersRef.current.has(senderId)) {break;}
        if (processedMessagesRef.current.has(msgId)) {break;}
        processedMessagesRef.current.add(msgId);

        const chatMsg: ChatMessage = {
          id: msgId,
          senderId,
          groupId,
          content,
          timestamp: event.timestamp || Date.now(),
          status: 'delivered',
          isOutgoing: senderId === userIdRef.current,
          forwardInfo: parseForwardInfo(event),
          rawEventJson: buildRawEventJson(event),
        };

        setGroups(prev => {
          const next = new Map(prev);
          const group = next.get(groupId) || {
            id: groupId,
            name: event.groupName || event.group_name || 'Group',
            members: [userIdRef.current],
            roles: {},
            messages: [],
          };
          next.set(groupId, {
            ...group,
            messages: [...group.messages, chatMsg],
          });
          return next;
        });
        break;
      }

      case 'group_message_sent': {
        const groupId = event.groupId || event.group_id;
        const messageIds: string[] = event.messageIds || event.message_ids || [];
        if (!groupId || messageIds.length === 0) {break;}
        setGroups(prev => {
          const next = new Map(prev);
          const group = next.get(groupId);
          if (group) {
            const msgs = group.messages.map(m =>
              messageIds.includes(m.id) && m.status === 'sending'
                ? {...m, status: 'sent' as const}
                : m,
            );
            next.set(groupId, {...group, messages: msgs});
          }
          return next;
        });
        break;
      }

      case 'group_member_added': {
        const groupId = event.groupId || event.group_id;
        const memberId = event.memberId || event.member_id || event.userId || event.user_id;
        const addedBy = event.addedBy || event.added_by;
        const groupName = event.groupName || event.group_name || null;
        if (!groupId || !memberId) {break;}
        setGroups(prev => {
          const next = new Map(prev);
          const group = next.get(groupId);
          if (group) {
            if (!group.members.includes(memberId)) {
              next.set(groupId, {
                ...group,
                // Update name if we now have it (e.g. from a late-arriving event)
                name: groupName || group.name,
                members: [...group.members, memberId],
              });
            }
          } else {
            // Auto-create group when we're being added (invitee side)
            const members = [memberId];
            if (addedBy && addedBy !== memberId) {members.push(addedBy);}
            if (!members.includes(userIdRef.current)) {members.push(userIdRef.current);}
            // The inviter (addedBy) is likely admin; invitee is member
            const roles: Record<string, GroupRole> = {};
            if (addedBy) {roles[addedBy] = 'admin';}
            roles[memberId] = 'member';
            next.set(groupId, {
              id: groupId,
              name: groupName || 'Group',
              members,
              roles,
              messages: [],
            });
          }
          return next;
        });
        break;
      }

      case 'group_member_removed': {
        const groupId = event.groupId || event.group_id;
        const memberId = event.memberId || event.member_id || event.userId || event.user_id;
        if (!groupId || !memberId) {break;}
        setGroups(prev => {
          const next = new Map(prev);
          if (memberId === userIdRef.current) {
            next.delete(groupId);
          } else {
            const group = next.get(groupId);
            if (group) {
              const {[memberId]: _, ...remainingRoles} = group.roles;
              next.set(groupId, {
                ...group,
                members: group.members.filter(m => m !== memberId),
                roles: remainingRoles,
              });
            }
          }
          return next;
        });
        break;
      }

      case 'group_role_changed': {
        const groupId = event.groupId || event.group_id;
        const targetUserId = event.userId || event.user_id;
        const newRole = (event.newRole || event.new_role || 'member') as GroupRole;
        if (!groupId || !targetUserId) {break;}
        setGroups(prev => {
          const next = new Map(prev);
          const group = next.get(groupId);
          if (group) {
            next.set(groupId, {
              ...group,
              roles: {...group.roles, [targetUserId]: newRole},
            });
          }
          return next;
        });
        break;
      }

      case 'group_info': {
        const groupId = event.groupId || event.group_id;
        const groupName = event.name || event.groupName || event.group_name || 'Group';
        const members: Array<{user_id: string; role?: string}> = event.members || [];
        if (!groupId) {break;}
        setGroups(prev => {
          const next = new Map(prev);
          const memberIds = members.map(m => m.user_id);
          const roles: Record<string, GroupRole> = {};
          for (const m of members) {
            roles[m.user_id] = m.role === 'admin' ? 'admin' : 'member';
          }
          const existing = next.get(groupId);
          next.set(groupId, {
            id: groupId,
            name: groupName,
            members: memberIds,
            roles,
            messages: existing?.messages || [],
          });
          return next;
        });
        break;
      }

      case 'user_groups': {
        const groupSummaries: Array<{group_id: string; name: string}> = event.groups || [];
        setGroups(prev => {
          const next = new Map(prev);
          for (const g of groupSummaries) {
            if (!next.has(g.group_id)) {
              next.set(g.group_id, {
                id: g.group_id,
                name: g.name || 'Group',
                members: [userIdRef.current],
                roles: {},
                messages: [],
              });
            }
          }
          return next;
        });
        break;
      }

      case 'group_error': {
        const reason = event.reason || 'Unknown group error';
        console.warn('[GroupError]', reason);
        break;
      }

      case 'group_message_partial_failure': {
        const groupId = event.groupId || event.group_id;
        const failedMembers: string[] = event.failedMembers || event.failed_members || [];
        if (!groupId || failedMembers.length === 0) {break;}
        console.warn(`[GroupPartialFailure] group=${groupId} failed=${failedMembers.join(',')}`);
        break;
      }

      case 'group_epoch_fork_detected': {
        const groupId = event.groupId || event.group_id;
        if (!groupId) {break;}
        console.warn(`[EpochFork] Detected in group=${groupId}`);
        break;
      }

      case 'group_epoch_fork_resolved': {
        const groupId = event.groupId || event.group_id;
        const failedMembers: string[] = event.failedMembers || event.failed_members || [];
        if (!groupId) {break;}
        if (failedMembers.length > 0) {
          console.warn(`[EpochFork] Resolved group=${groupId}, unreachable=${failedMembers.join(',')}`);
        }
        break;
      }

      case 'service_discovered': {
        const serviceId = event.serviceId || event.service_id;
        const provider = event.provider_peer_id || event.provider || event.providerId || event.provider_id;
        const version = event.version || '1.0';
        if (!serviceId || !provider) {break;}
        setDiscoveredServices(prev => {
          if (prev.some(s => s.serviceId === serviceId && s.provider === provider)) {return prev;}
          return [...prev, {serviceId, provider, version}];
        });
        break;
      }

      case 'service_request_received': {
        const requestId = event.requestId || event.request_id;
        const requester = event.sender || event.requester || event.requesterId || event.requester_id;
        const serviceId = event.serviceId || event.service_id;
        const body = event.body || event.message || '';
        if (!requestId) {break;}

        appendServiceLog({
          type: 'request',
          from: requester || 'unknown',
          body: `[${serviceId}] ${body}`,
          timestamp: Date.now(),
        });

        // Auto-respond to ping requests
        if (serviceId === 'ping.v1' && protocolRef.current) {
          const svc = new MeshServices();
          svc.respondToServiceRequest(
            requestId,
            requester,
            serviceId,
            'ok',
            'pong',
          ).catch(console.warn);
        }
        break;
      }

      case 'service_response_received': {
        const body = event.body || event.message || '';
        const provider = event.provider_peer_id || event.provider || event.providerId || event.provider_id || 'unknown';
        appendServiceLog({
          type: 'response',
          from: provider,
          body,
          timestamp: Date.now(),
        });
        break;
      }

      case 'presence_updated': {
        const peerId = event.peer_id || event.peerId;
        const status: PresenceStatus = event.status || 'offline';
        if (!peerId || blockedUsersRef.current.has(peerId)) {break;}
        setContacts(prev => {
          const next = new Map(prev);
          const existing = next.get(peerId);
          if (existing) {
            next.set(peerId, {
              ...existing,
              presenceStatus: status,
              lastSeen: Date.now(),
              isNearby: status === 'online' || existing.isNearby,
            });
          }
          return next;
        });
        break;
      }

      case 'typing_indicator_received': {
        const senderId = event.sender || event.peer_id || event.peerId;
        const isTyping = event.is_typing ?? event.isTyping ?? false;
        if (!senderId || blockedUsersRef.current.has(senderId)) {break;}
        setTypingPeers(prev => {
          const next = new Map(prev);
          if (isTyping) {
            next.set(senderId, Date.now());
          } else {
            next.delete(senderId);
          }
          return next;
        });
        break;
      }

      case 'read_receipt_received': {
        const senderId = event.sender || event.peer_id || event.peerId;
        const messageIds: string[] = event.message_ids || event.messageIds || [];
        if (!senderId || messageIds.length === 0) {break;}
        const idSet = new Set(messageIds);
        setChats(prev => {
          const next = new Map(prev);
          const chat = next.get(senderId);
          if (chat) {
            let changed = false;
            const msgs = chat.messages.map(m => {
              if (m.isOutgoing && idSet.has(m.id) && m.status !== 'read' && m.status !== 'failed') {
                changed = true;
                return {...m, status: 'read' as const};
              }
              return m;
            });
            if (changed) {
              next.set(senderId, {...chat, messages: msgs});
            }
          }
          return next;
        });
        break;
      }

      case 'message_relayed': {
        const msgId = event.message_id || event.messageId;
        const sender = event.sender || 'unknown';
        const recipient = event.recipient || 'unknown';
        appendServiceLog({
          type: 'response',
          from: 'relay',
          body: `Relayed ${msgId} from ${sender} to ${recipient}`,
          timestamp: Date.now(),
        });
        break;
      }

      case 'message_deferred': {
        const msgId = event.message_id || event.messageId;
        if (!msgId) {break;}
        // Update message status to indicate it's queued
        setChats(prev => {
          const next = new Map(prev);
          for (const [peerId, chat] of next) {
            const msgIndex = chat.messages.findIndex(m => m.id === msgId);
            if (msgIndex >= 0) {
              const msgs = [...chat.messages];
              // Keep 'sending' status — the message is queued, not failed
              if (msgs[msgIndex].status === 'sending') {
                msgs[msgIndex] = {...msgs[msgIndex]};
              }
              next.set(peerId, {...chat, messages: msgs});
              break;
            }
          }
          return next;
        });
        break;
      }

      default:
        break;
    }
  }, []);

  // ─── Telemetry handler ───────────────────────────────────

  const handleTelemetry = useCallback((rec: TelemetryRecord) => {
    switch (rec.category) {
      case 'metricsFrame': {
        setLatestMetrics(rec.frame);
        setMetricsHistory(prev => {
          const next = [...prev, rec.frame];
          return next.length > MAX_METRICS_HISTORY ? next.slice(-MAX_METRICS_HISTORY) : next;
        });
        break;
      }
      case 'transportState': {
        setTransportTimeline(prev => {
          const next = [rec.event, ...prev];
          return next.length > MAX_TIMELINE ? next.slice(0, MAX_TIMELINE) : next;
        });
        break;
      }
      case 'routingDecision': {
        setRoutingDecisions(prev => {
          const next = [rec.decision, ...prev];
          return next.length > MAX_DECISIONS ? next.slice(0, MAX_DECISIONS) : next;
        });
        break;
      }
      case 'deviceCapability': {
        setDeviceCapability(rec.snapshot);
        setDeviceCapabilityHistory(prev => {
          const next = [...prev, rec.snapshot];
          return next.length > MAX_DEVICE_HISTORY ? next.slice(-MAX_DEVICE_HISTORY) : next;
        });
        break;
      }
      case 'protocol': {
        // Only fold events that carry a final-delivery hop_count into the
        // hop-distribution histogram — everything else is dropped here to
        // avoid per-event context re-renders (every useProtocol() consumer
        // would otherwise re-render on every message).
        try {
          const parsed = JSON.parse(rec.eventJson);
          if (HOP_COUNT_EVENT_TYPES.has(parsed?.type)) {
            const hop = parsed.hop_count;
            if (typeof hop === 'number' && Number.isFinite(hop) && hop >= 0) {
              const bucket = Math.min(Math.floor(hop), 15);
              setHopCountHistogram(prev => ({
                ...prev,
                [bucket]: (prev[bucket] ?? 0) + 1,
              }));
            }
          }
        } catch {
          /* malformed envelope — skip silently, not actionable in UI */
        }
        break;
      }
      case 'mls':
      case 'extension':
      default:
        // Unused in aggregate diagnostics. Intentional no-op.
        break;
    }
  }, []);

  // ─── Initialize Protocol ─────────────────────────────────

  const initialize = useCallback(async (uid: string, uname: string) => {
    setUserId(uid);
    setUserName(uname);

    const config = {
      ...PROTOCOL_CONFIG,
      userId: uid,
    };

    const proto = new OfflineProtocol(config);
    protocolRef.current = proto;
    setProtocol(proto);

    // Register event handlers
    proto.on('all', handleEvent);

    // Start protocol
    await proto.start();
    setIsStarted(true);

    // Initialize mesh services
    const svc = new MeshServices();
    setMeshServices(svc);

    // Install telemetry sink — push delivery, fast cadence, full DORS detail.
    // routingDiagnostic gives per-transport score breakdowns the demo renders
    // as bar charts; mls verbosity stays at 'lifecycle' so we get session ready
    // / decryption-failed without per-op spam; pull queue disabled because we
    // consume via callback only.
    try {
      // Capture the unsubscribe so cleanup can drop the JS-side listener —
      // `uninstallTelemetrySink()` detaches the native sink but leaves
      // listeners bound by design (see SDK docstring).
      const unsubscribe = await proto.installTelemetrySink(
        {
          routingDiagnostic: true,
          metricsCadenceMs: 2000,
          mlsVerbosity: 'lifecycle',
          enablePollQueue: false,
        },
        handleTelemetry,
      );
      telemetryUnsubscribeRef.current = unsubscribe;
    } catch (err) {
      console.warn('[ProtocolContext] installTelemetrySink failed:', err);
    }
  }, [handleEvent, handleTelemetry]);

  // ─── Presence Broadcasting ───────────────────────────────

  useEffect(() => {
    if (!isStarted || !protocolRef.current) {return;}

    const broadcastPresence = async () => {
      const proto = protocolRef.current;
      if (!proto) {return;}

      for (const [peerId, contact] of contactsRef.current) {
        if (!contact.hasSession || contact.isBlocked) {continue;}
        try {
          await proto.sendPresenceUpdate(peerId, 'online');
        } catch {
          // Ignore presence send failures
        }
      }
    };

    // Send initial presence immediately
    broadcastPresence();

    const interval = setInterval(broadcastPresence, PRESENCE_BROADCAST_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [isStarted]);

  // ─── Stale neighbor cleanup ──────────────────────────────

  useEffect(() => {
    if (!isStarted) {return;}

    const interval = setInterval(() => {
      const now = Date.now();
      setNeighbors(prev => {
        let changed = false;
        const next = new Map(prev);
        for (const [peerId, neighbor] of next) {
          if (now - neighbor.discoveredAt > NEARBY_THRESHOLD_MS * 2) {
            next.delete(peerId);
            changed = true;
          }
        }
        return changed ? next : prev;
      });

      setContacts(prev => {
        let changed = false;
        const next = new Map(prev);
        for (const [peerId, contact] of next) {
          if (contact.isNearby && now - contact.lastSeen > NEARBY_THRESHOLD_MS) {
            next.set(peerId, {...contact, isNearby: false});
            changed = true;
          }
        }
        return changed ? next : prev;
      });

      // Expire stale typing indicators
      setTypingPeers(prev => {
        let changed = false;
        const next = new Map(prev);
        for (const [peerId, ts] of next) {
          if (now - ts > TYPING_INDICATOR_TIMEOUT_MS) {
            next.delete(peerId);
            changed = true;
          }
        }
        return changed ? next : prev;
      });

      // Prevent unbounded growth of processed message IDs
      if (processedMessagesRef.current.size > 1000) {
        processedMessagesRef.current.clear();
      }
    }, NEARBY_THRESHOLD_MS);

    return () => clearInterval(interval);
  }, [isStarted]);

  // ─── Cleanup ─────────────────────────────────────────────

  useEffect(() => {
    return () => {
      if (protocolRef.current) {
        telemetryUnsubscribeRef.current?.();
        telemetryUnsubscribeRef.current = null;
        protocolRef.current.uninstallTelemetrySink().catch(() => {});
        protocolRef.current.removeAllListeners();
        protocolRef.current.stop().catch(console.warn);
      }
    };
  }, []);

  // ─── Actions ─────────────────────────────────────────────

  const sendMessage = useCallback(async (recipientId: string, content: string, priority: 'medium' | 'critical' = 'medium') => {
    if (!protocolRef.current) {return;}
    if (blockedUsersRef.current.has(recipientId)) {return;}

    const msgPriority = priority === 'critical' ? MessagePriority.Critical : MessagePriority.Medium;
    let msgId: string;
    try {
      msgId = await protocolRef.current.sendMessage({
        recipient: recipientId,
        content,
        priority: msgPriority,
      });
    } catch {
      return;
    }

    const chatMsg: ChatMessage = {
      id: msgId,
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
  }, []);

  const sendConnectionRequestAction = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.sendConnectionRequest({
      recipient: peerId,
      senderName: userNameRef.current,
    });
    setConnectionRequests(prev => {
      if (prev.some(r => r.peerId === peerId)) {return prev;}
      return [...prev, {
        peerId,
        name: peerId,
        direction: 'out',
        timestamp: Date.now(),
      }];
    });
  }, []);

  const acceptConnectionRequestAction = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.acceptConnectionRequest({
      recipient: peerId,
      accepterName: userNameRef.current,
    });
    setConnectionRequests(prev => {
      const request = prev.find(r => r.peerId === peerId);
      const peerName = request?.name || peerId;
      setContacts(prevContacts => {
        const next = new Map(prevContacts);
        const existing = next.get(peerId);
        next.set(peerId, {
          peerId,
          name: existing?.name || peerName,
          lastSeen: Date.now(),
          isNearby: neighborsRef.current.has(peerId),
          hasSession: existing?.hasSession || sessionPeersRef.current.has(peerId),
          isBlocked: false,
          presenceStatus: existing?.presenceStatus || 'offline',
        });
        return next;
      });
      return prev.filter(r => r.peerId !== peerId);
    });
  }, []);

  const rejectConnectionRequestAction = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.rejectConnectionRequest({recipient: peerId});
    setConnectionRequests(prev => prev.filter(r => r.peerId !== peerId));
  }, []);

  const cancelConnectionRequestAction = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.cancelConnectionRequest({recipient: peerId});
    setConnectionRequests(prev => prev.filter(r => r.peerId !== peerId));
  }, []);

  const createGroupAction = useCallback(async (name: string, memberIds: string[]) => {
    if (!protocolRef.current) {return;}
    const result = await protocolRef.current.meshCreateGroup(name);
    const groupId = result.groupId;

    const allMembers = [userIdRef.current, ...memberIds];

    setGroups(prev => {
      const next = new Map(prev);
      const roles: Record<string, GroupRole> = {[userIdRef.current]: 'admin'};
      for (const mid of memberIds) {
        roles[mid] = 'member';
      }
      next.set(groupId, {
        id: groupId,
        name,
        members: allMembers,
        roles,
        messages: [],
      });
      return next;
    });

    // Invite each member via the high-level protocol API
    // This sends MLS Welcome to the invitee and Commit to existing members
    const failedInvites: string[] = [];
    for (const memberId of memberIds) {
      try {
        await protocolRef.current.meshInviteToGroup(groupId, memberId);
      } catch (err) {
        console.warn(`Failed to invite ${memberId} to group (key exchange may still be pending):`, err);
        failedInvites.push(memberId);
      }
    }

    // Remove members whose invites failed — they're not actually in MLS
    if (failedInvites.length > 0) {
      setGroups(prev => {
        const next = new Map(prev);
        const group = next.get(groupId);
        if (group) {
          const failedSet = new Set(failedInvites);
          const filteredRoles = {...group.roles};
          for (const fm of failedInvites) { delete filteredRoles[fm]; }
          next.set(groupId, {
            ...group,
            members: group.members.filter(m => !failedSet.has(m)),
            roles: filteredRoles,
          });
        }
        return next;
      });
    }
  }, []);

  const sendGroupMessageAction = useCallback(async (groupId: string, content: string, priority: 'medium' | 'critical' = 'medium') => {
    if (!protocolRef.current) {return;}

    const msgIds = await protocolRef.current.meshSendGroupMessage(groupId, content, priority);
    const msgId = msgIds[0] || `grp-${Date.now()}`;

    const chatMsg: ChatMessage = {
      id: msgId,
      senderId: userIdRef.current,
      groupId,
      content,
      timestamp: Date.now(),
      status: 'sending',
      isOutgoing: true,
    };

    setGroups(prev => {
      const next = new Map(prev);
      const group = next.get(groupId);
      if (group) {
        next.set(groupId, {...group, messages: [...group.messages, chatMsg]});
      }
      return next;
    });
  }, []);

  const leaveGroupAction = useCallback(async (groupId: string) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.meshLeaveGroup(groupId);
    setGroups(prev => {
      const next = new Map(prev);
      next.delete(groupId);
      return next;
    });
  }, []);

  const inviteToGroupAction = useCallback(async (groupId: string, memberId: string) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.meshInviteToGroup(groupId, memberId);
    setGroups(prev => {
      const next = new Map(prev);
      const group = next.get(groupId);
      if (group && !group.members.includes(memberId)) {
        next.set(groupId, {
          ...group,
          members: [...group.members, memberId],
          roles: {...group.roles, [memberId]: 'member'},
        });
      }
      return next;
    });
  }, []);

  const removeFromGroupAction = useCallback(async (groupId: string, memberId: string) => {
    if (!protocolRef.current) {return;}
    let protocolError: any = null;
    try {
      await protocolRef.current.meshRemoveFromGroup(groupId, memberId);
    } catch (err) {
      protocolError = err;
    }
    // Always update local state — handles phantom members (invite failed
    // but member was added optimistically) and normal removals alike.
    setGroups(prev => {
      const next = new Map(prev);
      const group = next.get(groupId);
      if (group) {
        const {[memberId]: _, ...remainingRoles} = group.roles;
        next.set(groupId, {
          ...group,
          members: group.members.filter(m => m !== memberId),
          roles: remainingRoles,
        });
      }
      return next;
    });
    // Re-throw so callers can still handle real errors (e.g. last-admin guard)
    if (protocolError) {throw protocolError;}
  }, []);

  const setMemberRoleAction = useCallback(async (groupId: string, targetUserId: string, role: GroupRole) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.meshSetMemberRole(groupId, targetUserId, role);
    setGroups(prev => {
      const next = new Map(prev);
      const group = next.get(groupId);
      if (group) {
        next.set(groupId, {
          ...group,
          roles: {...group.roles, [targetUserId]: role},
        });
      }
      return next;
    });
  }, []);

  const getGroupRolesAction = useCallback(async (groupId: string): Promise<Record<string, string>> => {
    if (!protocolRef.current) {return {};}
    const roles = await protocolRef.current.meshGetGroupRoles(groupId);
    setGroups(prev => {
      const next = new Map(prev);
      const group = next.get(groupId);
      if (group) {
        const typedRoles: Record<string, GroupRole> = {};
        for (const [uid, r] of Object.entries(roles)) {
          typedRoles[uid] = r === 'admin' ? 'admin' : 'member';
        }
        next.set(groupId, {...group, roles: typedRoles});
      }
      return next;
    });
    return roles;
  }, []);

  const getMemberRoleAction = useCallback((groupId: string, targetUserId: string): GroupRole => {
    const group = groups.get(groupId);
    return group?.roles[targetUserId] || 'member';
  }, [groups]);

  const registerServiceAction = useCallback(async (serviceId: string, version: string) => {
    if (!meshServices) {return;}
    await meshServices.registerService(serviceId, version);
    setRegisteredServices(prev =>
      prev.includes(serviceId) ? prev : [...prev, serviceId],
    );
  }, [meshServices]);

  const unregisterServiceAction = useCallback(async (serviceId: string) => {
    if (!meshServices) {return;}
    await meshServices.unregisterService(serviceId);
    setRegisteredServices(prev => prev.filter(s => s !== serviceId));
  }, [meshServices]);

  const discoverServicesAction = useCallback(async (serviceId?: string) => {
    if (!meshServices) {return;}
    setDiscoveredServices([]);
    await meshServices.discoverServices(serviceId);
  }, [meshServices]);

  const sendServiceRequestAction = useCallback(async (
    provider: string,
    serviceId: string,
    method: string,
    body: string,
  ) => {
    if (!meshServices) {return;}
    await meshServices.sendServiceRequest(provider, serviceId, method, body);
    appendServiceLog({
      type: 'request',
      from: 'me',
      body: `[${serviceId}] ${method}: ${body}`,
      timestamp: Date.now(),
    });
  }, [meshServices]);

  const markChatRead = useCallback((peerId: string) => {
    setChats(prev => {
      const chat = prev.get(peerId);
      if (!chat || chat.unreadCount === 0) {return prev;}

      // Send read receipts for unread incoming messages
      const unreadIds = chat.messages
        .filter(m => !m.isOutgoing && m.status === 'delivered')
        .map(m => m.id);
      if (unreadIds.length > 0 && protocolRef.current) {
        protocolRef.current.sendReadReceipt(peerId, unreadIds).catch(() => {});
      }

      const next = new Map(prev);
      next.set(peerId, {...chat, unreadCount: 0});
      return next;
    });
  }, []);

  const blockUserAction = useCallback(async (peerId: string) => {
    blockedUsersRef.current.add(peerId);
    if (protocolRef.current) {
      try {
        await protocolRef.current.blockUser(peerId);
      } catch {
        // Protocol-level blocking failed, still keep UI-level block
      }
    }
    setContacts(prev => {
      const next = new Map(prev);
      const contact = next.get(peerId);
      if (contact) {
        next.set(peerId, {...contact, isBlocked: true, presenceStatus: 'offline' as PresenceStatus});
      }
      return next;
    });
  }, []);

  const unblockUserAction = useCallback(async (peerId: string) => {
    blockedUsersRef.current.delete(peerId);
    if (protocolRef.current) {
      try {
        await protocolRef.current.unblockUser(peerId);
      } catch {
        // Protocol-level unblocking failed, still update UI
      }
    }
    // Unblocking clears the MLS session at the protocol level, so mark
    // hasSession false — a fresh key exchange will re-establish it.
    setContacts(prev => {
      const next = new Map(prev);
      const contact = next.get(peerId);
      if (contact) {
        next.set(peerId, {...contact, isBlocked: false, hasSession: false, presenceStatus: 'offline' as PresenceStatus});
      }
      return next;
    });
  }, []);

  const forwardMessageAction = useCallback(async (message: ChatMessage, recipientId: string) => {
    if (!protocolRef.current) {return;}
    if (blockedUsersRef.current.has(recipientId)) {return;}

    const originalJson = message.rawEventJson || JSON.stringify({
      id: message.id,
      sender: message.senderId,
      recipient: message.recipientId || userIdRef.current,
      app_id: PROTOCOL_CONFIG.appId,
      priority: 'medium',
      ttl: 8,
      hop_count: 0,
      content: message.content,
      timestamp: message.timestamp,
    });

    let msgId: string;
    try {
      msgId = await protocolRef.current.forwardMessage({
        originalMessageJson: originalJson,
        newRecipient: recipientId,
      });
    } catch (err) {
      console.warn('Failed to forward message:', err);
      return;
    }

    const fwdInfo: ForwardInfo = message.forwardInfo
      ? {...message.forwardInfo, forwardCount: message.forwardInfo.forwardCount + 1}
      : {
          originalSender: message.senderId,
          originalMessageId: message.id,
          originalTimestamp: message.timestamp,
          forwardCount: 1,
        };

    const chatMsg: ChatMessage = {
      id: msgId,
      senderId: userIdRef.current,
      recipientId,
      content: message.content,
      timestamp: Date.now(),
      status: 'sending',
      isOutgoing: true,
      forwardInfo: fwdInfo,
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
  }, []);

  const forwardMessageToGroupAction = useCallback(async (message: ChatMessage, groupId: string) => {
    if (!protocolRef.current) {return;}

    const originalJson = message.rawEventJson || JSON.stringify({
      id: message.id,
      sender: message.senderId,
      recipient: message.recipientId || userIdRef.current,
      app_id: PROTOCOL_CONFIG.appId,
      priority: 'medium',
      ttl: 8,
      hop_count: 0,
      content: message.content,
      timestamp: message.timestamp,
    });

    let msgIds: string[];
    try {
      msgIds = await protocolRef.current.meshForwardMessageToGroup({
        originalMessageJson: originalJson,
        groupId,
      });
    } catch (err) {
      console.warn('Failed to forward message to group:', err);
      return;
    }

    const fwdInfo: ForwardInfo = message.forwardInfo
      ? {...message.forwardInfo, forwardCount: message.forwardInfo.forwardCount + 1}
      : {
          originalSender: message.senderId,
          originalMessageId: message.id,
          originalTimestamp: message.timestamp,
          forwardCount: 1,
        };

    const msgId = msgIds[0] || `fwd-grp-${Date.now()}`;
    const chatMsg: ChatMessage = {
      id: msgId,
      senderId: userIdRef.current,
      groupId,
      content: message.content,
      timestamp: Date.now(),
      status: 'sending',
      isOutgoing: true,
      forwardInfo: fwdInfo,
    };

    setGroups(prev => {
      const next = new Map(prev);
      const group = next.get(groupId);
      if (group) {
        next.set(groupId, {...group, messages: [...group.messages, chatMsg]});
      }
      return next;
    });
  }, []);

  const sendTypingIndicatorAction = useCallback(async (recipientId: string, isTyping: boolean) => {
    if (!protocolRef.current) {return;}
    if (blockedUsersRef.current.has(recipientId)) {return;}
    try {
      await protocolRef.current.sendTypingIndicator(recipientId, recipientId, isTyping);
    } catch {
      // Ignore typing indicator failures
    }
  }, []);

  // ─── Context Value ───────────────────────────────────────

  const value: ProtocolContextValue = {
    protocol,
    meshServices,
    isStarted,
    userId,
    userName,
    neighbors,
    contacts,
    connectionRequests,
    chats,
    groups,
    registeredServices,
    discoveredServices,
    serviceLog,
    typingPeers,
    latestMetrics,
    metricsHistory,
    transportTimeline,
    routingDecisions,
    deviceCapability,
    deviceCapabilityHistory,
    hopCountHistogram,
    initialize,
    sendMessage,
    sendConnectionRequest: sendConnectionRequestAction,
    acceptConnectionRequest: acceptConnectionRequestAction,
    rejectConnectionRequest: rejectConnectionRequestAction,
    cancelConnectionRequest: cancelConnectionRequestAction,
    createGroup: createGroupAction,
    sendGroupMessage: sendGroupMessageAction,
    leaveGroup: leaveGroupAction,
    inviteToGroup: inviteToGroupAction,
    removeFromGroup: removeFromGroupAction,
    setMemberRole: setMemberRoleAction,
    getGroupRoles: getGroupRolesAction,
    getMemberRole: getMemberRoleAction,
    registerService: registerServiceAction,
    unregisterService: unregisterServiceAction,
    discoverServices: discoverServicesAction,
    sendServiceRequest: sendServiceRequestAction,
    forwardMessage: forwardMessageAction,
    forwardMessageToGroup: forwardMessageToGroupAction,
    markChatRead,
    blockUser: blockUserAction,
    unblockUser: unblockUserAction,
    sendTypingIndicator: sendTypingIndicatorAction,
  };

  return (
    <ProtocolContext.Provider value={value}>
      {children}
    </ProtocolContext.Provider>
  );
}
