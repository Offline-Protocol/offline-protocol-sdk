export type ConnectionStatus =
  | 'none'
  | 'pending_sent'
  | 'pending_received'
  | 'accepted'
  | 'rejected';

export interface Neighbor {
  peerId: string;
  transport: string;
  discoveredAt: number;
  connectionStatus: ConnectionStatus;
  displayName?: string;
}

export interface ChatMessage {
  id: string;
  senderId: string;
  recipientId?: string;
  content: string;
  timestamp: number;
  status: 'sending' | 'sent' | 'delivered' | 'failed';
  isOutgoing: boolean;
}

export interface Chat {
  peerId: string;
  messages: ChatMessage[];
  unreadCount: number;
}

export interface LogEntry {
  id: string;
  timestamp: number;
  level: 'info' | 'warning' | 'error' | 'debug';
  message: string;
}

export type TabName = 'peers' | 'chat' | 'logs';
