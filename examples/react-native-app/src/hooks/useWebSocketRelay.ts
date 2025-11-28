import { useState, useCallback, useRef, useEffect } from 'react';

const RELAY_SERVER_URL = 'wss://relay-server-production-31c7.up.railway.app/ws';

interface AuthenticatedResponse {
  type: 'Authenticated';
  user_id: string;
  username: string;
}

interface AuthErrorResponse {
  type: 'AuthError';
  reason: string;
}

interface MessageReceivedResponse {
  type: 'MessageReceived';
  sender: string;
  content: string;
  timestamp: string;
}

interface DeliveryErrorResponse {
  type: 'DeliveryError';
  recipient: string;
  reason: string;
}

interface PresenceStatusResponse {
  type: 'PresenceStatus';
  user_id: string;
  online: boolean;
}

interface PresenceStatusWithLastSeenResponse {
  type: 'PresenceStatusWithLastSeen';
  user_id: string;
  online: boolean;
  last_seen: string;
}

interface GroupMessageReceivedResponse {
  type: 'GroupMessageReceived';
  group_id: string;
  sender: string;
  content: string;
  timestamp: string;
}

interface GroupCreatedResponse {
  type: 'GroupCreated';
  group_id: string;
  name: string;
}

interface GroupMemberAddedResponse {
  type: 'GroupMemberAdded';
  group_id: string;
  user_id: string;
  added_by: string;
}

interface GroupMemberRemovedResponse {
  type: 'GroupMemberRemoved';
  group_id: string;
  user_id: string;
  removed_by: string;
}

interface GroupInfoResponse {
  type: 'GroupInfo';
  group_id: string;
  name: string;
  created_by: string;
  created_at: string;
  members: Array<{
    user_id: string;
    role: 'admin' | 'member';
    joined_at: string;
  }>;
}

interface UserGroupsResponse {
  type: 'UserGroups';
  groups: Array<{
    group_id: string;
    name: string;
    created_at: string;
  }>;
}

interface GroupErrorResponse {
  type: 'GroupError';
  reason: string;
}

interface TypingUpdateResponse {
  type: 'TypingUpdate';
  conversation_id: string;
  user_id: string;
  typing: boolean;
}

type ServerMessage =
  | AuthenticatedResponse
  | AuthErrorResponse
  | MessageReceivedResponse
  | DeliveryErrorResponse
  | PresenceStatusResponse
  | PresenceStatusWithLastSeenResponse
  | GroupMessageReceivedResponse
  | GroupCreatedResponse
  | GroupMemberAddedResponse
  | GroupMemberRemovedResponse
  | GroupInfoResponse
  | UserGroupsResponse
  | GroupErrorResponse
  | TypingUpdateResponse
  | { type: string; [key: string]: unknown };

export interface OnlineMessage {
  id: string;
  sender: string;
  content: string;
  timestamp: Date;
  isFromMe: boolean;
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

type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'authenticated' | 'error';

interface UseWebSocketRelayOptions {
  onMessageReceived?: (message: OnlineMessage) => void;
  onGroupMessageReceived?: (groupId: string, message: OnlineMessage) => void;
  onPresenceUpdate?: (userId: string, isOnline: boolean, lastSeen?: Date) => void;
  onTypingUpdate?: (conversationId: string, userId: string, isTyping: boolean) => void;
  onError?: (error: string) => void;
}

export function useWebSocketRelay(options: UseWebSocketRelayOptions = {}) {
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');
  const [authenticatedUser, setAuthenticatedUser] = useState<OnlineUser | null>(null);
  const [messages, setMessages] = useState<OnlineMessage[]>([]);
  const [onlineUsers, setOnlineUsers] = useState<Map<string, OnlineUser>>(new Map());
  const [groups, setGroups] = useState<OnlineGroup[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [typingUsers, setTypingUsers] = useState<Map<string, Set<string>>>(new Map());

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const messageIdCounter = useRef(0);

  const generateMessageId = useCallback(() => {
    messageIdCounter.current += 1;
    return `msg_${Date.now()}_${messageIdCounter.current}`;
  }, []);

  const handleMessage = useCallback((event: WebSocketMessageEvent) => {
    try {
      const data: ServerMessage = JSON.parse(event.data);

      switch (data.type) {
        case 'Authenticated': {
          const authData = data as AuthenticatedResponse;
          setStatus('authenticated');
          setAuthenticatedUser({
            userId: authData.user_id,
            username: authData.username,
            isOnline: true,
          });
          setError(null);
          break;
        }

        case 'AuthError': {
          const authError = data as AuthErrorResponse;
          setStatus('error');
          setError(authError.reason);
          options.onError?.(authError.reason);
          break;
        }

        case 'MessageReceived': {
          const msgData = data as MessageReceivedResponse;
          const newMessage: OnlineMessage = {
            id: generateMessageId(),
            sender: msgData.sender,
            content: msgData.content,
            timestamp: new Date(msgData.timestamp),
            isFromMe: false,
          };
          setMessages(prev => [...prev, newMessage]);
          options.onMessageReceived?.(newMessage);
          break;
        }

        case 'DeliveryError': {
          const deliveryError = data as DeliveryErrorResponse;
          options.onError?.(`Failed to deliver to ${deliveryError.recipient}: ${deliveryError.reason}`);
          break;
        }

        case 'PresenceStatus': {
          const presence = data as PresenceStatusResponse;
          setOnlineUsers(prev => {
            const updated = new Map(prev);
            const existing = updated.get(presence.user_id);
            updated.set(presence.user_id, {
              userId: presence.user_id,
              username: existing?.username ?? presence.user_id,
              isOnline: presence.online,
              lastSeen: existing?.lastSeen,
            });
            return updated;
          });
          options.onPresenceUpdate?.(presence.user_id, presence.online);
          break;
        }

        case 'PresenceStatusWithLastSeen': {
          const presence = data as PresenceStatusWithLastSeenResponse;
          const lastSeen = new Date(presence.last_seen);
          setOnlineUsers(prev => {
            const updated = new Map(prev);
            const existing = updated.get(presence.user_id);
            updated.set(presence.user_id, {
              userId: presence.user_id,
              username: existing?.username ?? presence.user_id,
              isOnline: presence.online,
              lastSeen,
            });
            return updated;
          });
          options.onPresenceUpdate?.(presence.user_id, presence.online, lastSeen);
          break;
        }

        case 'GroupMessageReceived': {
          const groupMsg = data as GroupMessageReceivedResponse;
          const newMessage: OnlineMessage = {
            id: generateMessageId(),
            sender: groupMsg.sender,
            content: groupMsg.content,
            timestamp: new Date(groupMsg.timestamp),
            isFromMe: groupMsg.sender === authenticatedUser?.userId,
          };
          options.onGroupMessageReceived?.(groupMsg.group_id, newMessage);
          break;
        }

        case 'GroupCreated': {
          const groupCreated = data as GroupCreatedResponse;
          setGroups(prev => [
            ...prev,
            {
              groupId: groupCreated.group_id,
              name: groupCreated.name,
              createdAt: new Date(),
            },
          ]);
          break;
        }

        case 'UserGroups': {
          const userGroups = data as UserGroupsResponse;
          setGroups(
            userGroups.groups.map(g => ({
              groupId: g.group_id,
              name: g.name,
              createdAt: new Date(g.created_at),
            })),
          );
          break;
        }

        case 'GroupError': {
          const groupError = data as GroupErrorResponse;
          options.onError?.(groupError.reason);
          break;
        }

        case 'TypingUpdate': {
          const typing = data as TypingUpdateResponse;
          setTypingUsers(prev => {
            const updated = new Map(prev);
            const conversationTypers = updated.get(typing.conversation_id) ?? new Set();
            if (typing.typing) {
              conversationTypers.add(typing.user_id);
            } else {
              conversationTypers.delete(typing.user_id);
            }
            updated.set(typing.conversation_id, conversationTypers);
            return updated;
          });
          options.onTypingUpdate?.(typing.conversation_id, typing.user_id, typing.typing);
          break;
        }

        default:
          console.log('[WebSocketRelay] Unhandled message type:', data.type);
      }
    } catch (err) {
      console.error('[WebSocketRelay] Failed to parse message:', err);
    }
  }, [authenticatedUser?.userId, generateMessageId, options]);

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      return;
    }

    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = null;
    }

    setStatus('connecting');
    setError(null);

    try {
      const ws = new WebSocket(RELAY_SERVER_URL);

      ws.onopen = () => {
        console.log('[WebSocketRelay] Connected');
        setStatus('connected');
      };

      ws.onmessage = handleMessage;

      ws.onerror = (event) => {
        console.error('[WebSocketRelay] Error:', event);
        setStatus('error');
        setError('WebSocket connection error');
      };

      ws.onclose = (event) => {
        console.log('[WebSocketRelay] Closed:', event.code, event.reason);
        setStatus('disconnected');
        setAuthenticatedUser(null);
        wsRef.current = null;
      };

      wsRef.current = ws;
    } catch (err) {
      console.error('[WebSocketRelay] Failed to connect:', err);
      setStatus('error');
      setError('Failed to connect to relay server');
    }
  }, [handleMessage]);

  const disconnect = useCallback(() => {
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = null;
    }

    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    setStatus('disconnected');
    setAuthenticatedUser(null);
    setError(null);
  }, []);

  const send = useCallback((message: Record<string, unknown>) => {
    if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) {
      console.error('[WebSocketRelay] Cannot send: not connected');
      return false;
    }

    try {
      wsRef.current.send(JSON.stringify(message));
      return true;
    } catch (err) {
      console.error('[WebSocketRelay] Send error:', err);
      return false;
    }
  }, []);

  const authenticate = useCallback((token: string) => {
    if (status !== 'connected') {
      console.error('[WebSocketRelay] Cannot authenticate: not connected');
      return false;
    }

    return send({ type: 'Authenticate', token });
  }, [send, status]);

  const sendMessage = useCallback((recipientId: string, content: string) => {
    if (status !== 'authenticated') {
      console.error('[WebSocketRelay] Cannot send message: not authenticated');
      return false;
    }

    const success = send({
      type: 'SendMessage',
      recipient: recipientId,
      content,
    });

    if (success) {
      const sentMessage: OnlineMessage = {
        id: generateMessageId(),
        sender: authenticatedUser?.userId ?? 'me',
        content,
        timestamp: new Date(),
        isFromMe: true,
      };
      setMessages(prev => [...prev, sentMessage]);
    }

    return success;
  }, [authenticatedUser?.userId, generateMessageId, send, status]);

  const checkPresence = useCallback((userId: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'CheckPresence',
      user_id: userId,
    });
  }, [send, status]);

  const setTyping = useCallback((conversationId: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'SetTyping',
      conversation_id: conversationId,
    });
  }, [send, status]);

  const clearTyping = useCallback((conversationId: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'ClearTyping',
      conversation_id: conversationId,
    });
  }, [send, status]);

  const createGroup = useCallback((groupId: string, name: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'CreateGroup',
      group_id: groupId,
      name,
    });
  }, [send, status]);

  const sendGroupMessage = useCallback((groupId: string, content: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'SendGroupMessage',
      group_id: groupId,
      content,
    });
  }, [send, status]);

  const addGroupMember = useCallback((groupId: string, userId: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'AddGroupMember',
      group_id: groupId,
      user_id: userId,
    });
  }, [send, status]);

  const removeGroupMember = useCallback((groupId: string, userId: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'RemoveGroupMember',
      group_id: groupId,
      user_id: userId,
    });
  }, [send, status]);

  const leaveGroup = useCallback((groupId: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'LeaveGroup',
      group_id: groupId,
    });
  }, [send, status]);

  const getGroupInfo = useCallback((groupId: string) => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({
      type: 'GetGroupInfo',
      group_id: groupId,
    });
  }, [send, status]);

  const getUserGroups = useCallback(() => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({ type: 'GetUserGroups' });
  }, [send, status]);

  const clearMessages = useCallback(() => {
    setMessages([]);
  }, []);

  useEffect(() => {
    return () => {
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
      }
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, []);

  return {
    status,
    authenticatedUser,
    messages,
    onlineUsers,
    groups,
    typingUsers,
    error,

    connect,
    disconnect,
    authenticate,
    sendMessage,
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
    clearMessages,
  };
}

