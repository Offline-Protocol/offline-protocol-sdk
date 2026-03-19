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
import type {Contact, Neighbor, ConnectionRequest, ChatMessage, Chat, Group, DiscoveredService, ServiceLogEntry, ForwardInfo} from '../types';
import {
  PRESENCE_MESSAGE_PREFIX,
  PRESENCE_REBROADCAST_INTERVAL_MS,
  MAX_PRESENCE_SENDS_PER_TICK,
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

  // Actions
  initialize: (userId: string, userName: string) => Promise<void>;
  sendMessage: (recipientId: string, content: string, priority?: 'medium' | 'critical') => Promise<void>;
  sendConnectionRequest: (peerId: string) => Promise<void>;
  acceptConnectionRequest: (peerId: string) => Promise<void>;
  rejectConnectionRequest: (peerId: string) => Promise<void>;
  createGroup: (name: string, memberIds: string[]) => Promise<void>;
  sendGroupMessage: (groupId: string, content: string, priority?: 'medium' | 'critical') => Promise<void>;
  leaveGroup: (groupId: string) => Promise<void>;
  registerService: (serviceId: string, version: string) => Promise<void>;
  unregisterService: (serviceId: string) => Promise<void>;
  discoverServices: (serviceId?: string) => Promise<void>;
  sendServiceRequest: (provider: string, serviceId: string, method: string, body: string) => Promise<void>;
  forwardMessage: (message: ChatMessage, recipientId: string) => Promise<void>;
  forwardMessageToGroup: (message: ChatMessage, groupId: string) => Promise<void>;
  markChatRead: (peerId: string) => void;
  blockUser: (peerId: string) => Promise<void>;
  unblockUser: (peerId: string) => Promise<void>;
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

  const MAX_SERVICE_LOG = 100;
  const appendServiceLog = useCallback((entry: ServiceLogEntry) => {
    setServiceLog(prev => {
      const next = [...prev, entry];
      return next.length > MAX_SERVICE_LOG ? next.slice(-MAX_SERVICE_LOG) : next;
    });
  }, []);

  const protocolRef = useRef<OfflineProtocol | null>(null);
  const processedMessagesRef = useRef<Set<string>>(new Set());
  const contactsRef = useRef<Map<string, Contact>>(contacts);
  const neighborsRef = useRef<Map<string, Neighbor>>(neighbors);
  const userNameRef = useRef(userName);
  const userIdRef = useRef(userId);
  const blockedUsersRef = useRef<Set<string>>(new Set());

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
    const eventType = event.eventType || event.type;

    switch (eventType) {
      case 'neighbor_discovered': {
        const peerId = event.peerId || event.peer_id;
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
        setConnectionRequests(prev => {
          if (prev.some(r => r.peerId === peerId && r.direction === 'in')) {return prev;}
          return [...prev, {
            peerId,
            name: event.sender_name || event.userName || event.user_name || peerId,
            direction: 'in',
            timestamp: Date.now(),
          }];
        });
        break;
      }

      case 'connection_accepted': {
        const peerId = event.accepted_by || event.peerId || event.peer_id || event.byUserId || event.by_user_id;
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
            hasSession: existing?.hasSession || false,
            isBlocked: false,
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

      case 'secure_session_established': {
        const peerId = event.peerId || event.peer_id || event.otherUserId || event.other_user_id;
        if (!peerId) {break;}
        setContacts(prev => {
          const next = new Map(prev);
          const existing = next.get(peerId);
          next.set(peerId, {
            peerId,
            name: existing?.name || peerId,
            lastSeen: Date.now(),
            isNearby: neighborsRef.current.has(peerId),
            hasSession: true,
            isBlocked: false,
          });
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

        // Handle presence messages
        if (content.startsWith(PRESENCE_MESSAGE_PREFIX)) {
          const presenceData = content.slice(PRESENCE_MESSAGE_PREFIX.length);
          try {
            const parsed = JSON.parse(presenceData);
            setContacts(prev => {
              const next = new Map(prev);
              const existing = next.get(senderId);
              if (existing) {
                next.set(senderId, {
                  ...existing,
                  name: parsed.name || existing.name,
                  lastSeen: Date.now(),
                  isNearby: true,
                });
              }
              return next;
            });
          } catch { /* ignore malformed presence */ }
          break;
        }

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
        break;
      }

      case 'group_created': {
        const groupId = event.groupId || event.group_id;
        const groupName = event.groupName || event.group_name || event.name || 'Group';
        if (!groupId) {break;}
        setGroups(prev => {
          const next = new Map(prev);
          if (!next.has(groupId)) {
            next.set(groupId, {
              id: groupId,
              name: groupName,
              members: event.members || [userIdRef.current],
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
            next.set(groupId, {
              id: groupId,
              name: groupName || 'Group',
              members,
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
          const group = next.get(groupId);
          if (group) {
            next.set(groupId, {
              ...group,
              members: group.members.filter(m => m !== memberId),
            });
          }
          return next;
        });
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

      default:
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
  }, [handleEvent]);

  // ─── Presence Broadcasting ───────────────────────────────

  useEffect(() => {
    if (!isStarted || !protocolRef.current) {return;}

    const interval = setInterval(async () => {
      const proto = protocolRef.current;
      if (!proto) {return;}

      const presencePayload = JSON.stringify({
        name: userNameRef.current,
        timestamp: Date.now(),
      });
      const presenceContent = `${PRESENCE_MESSAGE_PREFIX}${presencePayload}`;

      let sendCount = 0;
      for (const [peerId, contact] of contactsRef.current) {
        if (sendCount >= MAX_PRESENCE_SENDS_PER_TICK) {break;}
        if (!contact.hasSession || !contact.isNearby || contact.isBlocked) {continue;}

        try {
          await proto.sendMessage({
            recipient: peerId,
            content: presenceContent,
            priority: MessagePriority.Low,
          });
          sendCount++;
        } catch {
          // Ignore presence send failures
        }
      }
    }, PRESENCE_REBROADCAST_INTERVAL_MS);

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
    setConnectionRequests(prev => prev.filter(r => r.peerId !== peerId));
    setContacts(prev => {
      const next = new Map(prev);
      const existing = next.get(peerId);
      next.set(peerId, {
        peerId,
        name: existing?.name || peerId,
        lastSeen: Date.now(),
        isNearby: neighborsRef.current.has(peerId),
        hasSession: existing?.hasSession || false,
        isBlocked: false,
      });
      return next;
    });
  }, []);

  const rejectConnectionRequestAction = useCallback(async (peerId: string) => {
    if (!protocolRef.current) {return;}
    await protocolRef.current.rejectConnectionRequest({recipient: peerId});
    setConnectionRequests(prev => prev.filter(r => r.peerId !== peerId));
  }, []);

  const createGroupAction = useCallback(async (name: string, memberIds: string[]) => {
    if (!protocolRef.current) {return;}
    const result = await protocolRef.current.meshCreateGroup(name);
    const groupId = result.groupId;

    const allMembers = [userIdRef.current, ...memberIds];

    setGroups(prev => {
      const next = new Map(prev);
      next.set(groupId, {
        id: groupId,
        name,
        members: allMembers,
        messages: [],
      });
      return next;
    });

    // Invite each member via the high-level protocol API
    // This sends MLS Welcome to the invitee and Commit to existing members
    for (const memberId of memberIds) {
      try {
        await protocolRef.current.meshInviteToGroup(groupId, memberId);
      } catch (err) {
        console.warn(`Failed to invite ${memberId} to group (key exchange may still be pending):`, err);
      }
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
        next.set(peerId, {...contact, isBlocked: true});
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
        next.set(peerId, {...contact, isBlocked: false, hasSession: false});
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
    initialize,
    sendMessage,
    sendConnectionRequest: sendConnectionRequestAction,
    acceptConnectionRequest: acceptConnectionRequestAction,
    rejectConnectionRequest: rejectConnectionRequestAction,
    createGroup: createGroupAction,
    sendGroupMessage: sendGroupMessageAction,
    leaveGroup: leaveGroupAction,
    registerService: registerServiceAction,
    unregisterService: unregisterServiceAction,
    discoverServices: discoverServicesAction,
    sendServiceRequest: sendServiceRequestAction,
    forwardMessage: forwardMessageAction,
    forwardMessageToGroup: forwardMessageToGroupAction,
    markChatRead,
    blockUser: blockUserAction,
    unblockUser: unblockUserAction,
  };

  return (
    <ProtocolContext.Provider value={value}>
      {children}
    </ProtocolContext.Provider>
  );
}
