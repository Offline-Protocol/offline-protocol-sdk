export type PresenceStatus = 'online' | 'away' | 'offline';

export interface Contact {
  peerId: string;
  name: string;
  lastSeen: number;
  isNearby: boolean;
  hasSession: boolean;
  isBlocked: boolean;
  presenceStatus: PresenceStatus;
}

export interface Neighbor {
  peerId: string;
  transport: string;
  rssi?: number;
  discoveredAt: number;
}

export interface ConnectionRequest {
  peerId: string;
  name: string;
  direction: 'in' | 'out';
  timestamp: number;
}

export interface ForwardInfo {
  originalSender: string;
  originalMessageId: string;
  originalTimestamp: number;
  forwardCount: number;
}

export interface ChatMessage {
  id: string;
  senderId: string;
  recipientId?: string;
  groupId?: string;
  content: string;
  timestamp: number;
  status: 'sending' | 'sent' | 'delivered' | 'read' | 'failed';
  isOutgoing: boolean;
  forwardInfo?: ForwardInfo;
  /** Raw event JSON — used as input when forwarding this message */
  rawEventJson?: string;
}

export interface Chat {
  peerId: string;
  messages: ChatMessage[];
  unreadCount: number;
}

export type GroupRole = 'admin' | 'member';

export interface GroupMember {
  userId: string;
  role: GroupRole;
}

export interface Group {
  id: string;
  name: string;
  members: string[];
  /** Per-member role map (userId -> role). Empty for legacy groups. */
  roles: Record<string, GroupRole>;
  messages: ChatMessage[];
}

export interface DiscoveredService {
  serviceId: string;
  provider: string;
  version: string;
}

export interface ServiceLogEntry {
  type: 'request' | 'response';
  from: string;
  body: string;
  timestamp: number;
}

export type TabName = 'people' | 'chats' | 'groups' | 'services';
