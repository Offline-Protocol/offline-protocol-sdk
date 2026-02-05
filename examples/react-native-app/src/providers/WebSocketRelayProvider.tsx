import React, { createContext, useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { DEFAULT_RELAY_SERVER_URL } from '../constants';

// Types
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
  message_id: string;
  reply_to_msg?: string;
}

interface GroupMessageSentResponse {
  type: 'GroupMessageSent';
  group_id: string;
  message_id: string;
  timestamp: string;
}

// Temporary storage for sent message content until we get GroupMessageSent
interface PendingSentMessage {
  content: string;
  replyToMsg?: string;
  timestamp: Date;
  groupId: string;
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
  | GroupMessageSentResponse
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

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'authenticated' | 'error';

export interface WebSocketRelayContextValue {
  status: ConnectionStatus;
  authenticatedUser: OnlineUser | null;
  messages: OnlineMessage[];
  onlineUsers: Map<string, OnlineUser>;
  groups: OnlineGroup[];
  groupDetails: Map<string, GroupDetails>;
  groupMessages: Map<string, OnlineMessage[]>;
  typingUsers: Map<string, Set<string>>;
  error: string | null;

  connect: () => void;
  disconnect: () => void;
  authenticate: (token: string) => boolean;
  send: (message: Record<string, unknown>) => boolean;
  sendMessage: (recipientId: string, content: string) => boolean;
  checkPresence: (username: string) => boolean;
  setTyping: (conversationId: string) => boolean;
  clearTyping: (conversationId: string) => boolean;
  createGroup: (name: string) => boolean;
  sendGroupMessage: (
    groupId: string,
    content: string,
    replyToMsg?: string,
  ) => boolean;
  addGroupMember: (groupId: string, username: string) => boolean;
  removeGroupMember: (groupId: string, username: string) => boolean;
  leaveGroup: (groupId: string) => boolean;
  getGroupInfo: (groupId: string) => boolean;
  getUserGroups: () => boolean;
  clearMessages: () => void;
  clearGroupMessages: (groupId: string) => void;
}

export const WebSocketRelayContext = createContext<WebSocketRelayContextValue | null>(null);

interface WebSocketRelayProviderProps {
  children: React.ReactNode;
  onMessageReceived?: (message: OnlineMessage) => void;
  onGroupMessageReceived?: (groupId: string, message: OnlineMessage) => void;
  onPresenceUpdate?: (userId: string, isOnline: boolean, lastSeen?: Date) => void;
  onTypingUpdate?: (conversationId: string, userId: string, isTyping: boolean) => void;
  onError?: (error: string) => void;
}

export function WebSocketRelayProvider({ 
  children,
  onMessageReceived,
  onGroupMessageReceived,
  onPresenceUpdate,
  onTypingUpdate,
  onError,
}: WebSocketRelayProviderProps) {
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');
  const [authenticatedUser, setAuthenticatedUser] = useState<OnlineUser | null>(
    null,
  );
  const [messages, setMessages] = useState<OnlineMessage[]>([]);
  const [onlineUsers, setOnlineUsers] = useState<Map<string, OnlineUser>>(
    new Map(),
  );
  const [groups, setGroups] = useState<OnlineGroup[]>([]);
  const [groupDetails, setGroupDetails] = useState<Map<string, GroupDetails>>(
    new Map(),
  );
  const [groupMessages, setGroupMessages] = useState<
    Map<string, OnlineMessage[]>
  >(new Map());
  const [error, setError] = useState<string | null>(null);
  const [typingUsers, setTypingUsers] = useState<Map<string, Set<string>>>(
    new Map(),
  );
  // Store pending sent messages temporarily until we get GroupMessageSent with message_id
  const pendingSentMessages = useRef<Map<string, PendingSentMessage>>(
    new Map(),
  );

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const messageIdCounter = useRef(0);

  const generateMessageId = useCallback(() => {
    messageIdCounter.current += 1;
    return `msg_${Date.now()}_${messageIdCounter.current}`;
  }, []);

  const handleMessage = useCallback(
    (event: WebSocketMessageEvent) => {
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
            onError?.(authError.reason);
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
            onMessageReceived?.(newMessage);
            break;
          }

          case 'DeliveryError': {
            const deliveryError = data as DeliveryErrorResponse;
            onError?.(
              `Failed to deliver to ${deliveryError.recipient}: ${deliveryError.reason}`,
            );
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
            onPresenceUpdate?.(presence.user_id, presence.online);
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
            onPresenceUpdate?.(presence.user_id, presence.online, lastSeen);
            break;
          }

          case 'GroupMessageReceived': {
            const groupMsg = data as GroupMessageReceivedResponse;
            console.log('[WebSocketRelay] Group message received:', groupMsg);
            const newMessage: OnlineMessage = {
              id: groupMsg.message_id, // Always use server-provided message ID
              sender: groupMsg.sender,
              content: groupMsg.content,
              timestamp: new Date(groupMsg.timestamp),
              isFromMe:
                groupMsg.sender === authenticatedUser?.userId ||
                groupMsg.sender === authenticatedUser?.username,
              replyToMsg: groupMsg.reply_to_msg,
            };
            // Store in groupMessages - update existing message if it exists (from GroupMessageSent)
            setGroupMessages(prev => {
              const updated = new Map(prev);
              const existing = updated.get(groupMsg.group_id) || [];
              const existingIndex = existing.findIndex(
                m => m.id === groupMsg.message_id,
              );

              if (existingIndex >= 0) {
                // Update existing message (e.g., if it was added via GroupMessageSent with empty content)
                const updatedMessages = [...existing];
                updatedMessages[existingIndex] = newMessage;
                updated.set(groupMsg.group_id, updatedMessages);
              } else {
                // Add new message if it doesn't exist
                updated.set(groupMsg.group_id, [...existing, newMessage]);
              }
              return updated;
            });
            onGroupMessageReceived?.(groupMsg.group_id, newMessage);
            break;
          }

          case 'GroupMessageSent': {
            const sentMsg = data as GroupMessageSentResponse;
            console.log(
              '[WebSocketRelay] Group message sent confirmation:',
              sentMsg,
            );

            // Find the most recent pending message for this group (within last 10 seconds)
            // This matches the sent message with the GroupMessageSent response
            const now = Date.now();
            let pendingMessage: PendingSentMessage | undefined;
            let pendingKey: string | undefined;

            for (const [
              key,
              pending,
            ] of pendingSentMessages.current.entries()) {
              if (
                pending.groupId === sentMsg.group_id &&
                now - pending.timestamp.getTime() < 10000 // Within 10 seconds
              ) {
                // Use the most recent one
                if (
                  !pendingMessage ||
                  pending.timestamp > pendingMessage.timestamp
                ) {
                  pendingMessage = pending;
                  pendingKey = key;
                }
              }
            }

            // Create message with server-provided message_id (never use temporary IDs)
            const newMessage: OnlineMessage = {
              id: sentMsg.message_id, // Always use server-provided message_id
              sender:
                authenticatedUser?.username ||
                authenticatedUser?.userId ||
                'me',
              content: pendingMessage?.content || '', // Use stored content (will be updated by GroupMessageReceived if needed)
              timestamp: new Date(sentMsg.timestamp),
              isFromMe: true,
              replyToMsg: pendingMessage?.replyToMsg,
            };

            // Remove matched pending message from storage
            if (pendingKey) {
              pendingSentMessages.current.delete(pendingKey);
            }

            // Add to group messages with server-provided message_id
            setGroupMessages(prev => {
              const updated = new Map(prev);
              const existing = updated.get(sentMsg.group_id) || [];
              // Avoid duplicates by checking message id
              if (!existing.some(m => m.id === sentMsg.message_id)) {
                updated.set(sentMsg.group_id, [...existing, newMessage]);
              }
              return updated;
            });
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

          case 'GroupInfo': {
            const groupInfo = data as GroupInfoResponse;
            console.log(
              '[WebSocketRelay] Group info received:',
              JSON.stringify(groupInfo, null, 2),
            );
            console.log(
              '[WebSocketRelay] Members raw:',
              JSON.stringify(groupInfo.members, null, 2),
            );
            const details: GroupDetails = {
              groupId: groupInfo.group_id,
              name: groupInfo.name,
              createdBy: groupInfo.created_by,
              createdAt: new Date(groupInfo.created_at),
              members: (groupInfo.members || []).map(m => ({
                userId: m.user_id || (m as any).username || 'unknown',
                role: m.role || 'member',
                joinedAt: m.joined_at ? new Date(m.joined_at) : new Date(),
              })),
            };
            console.log(
              '[WebSocketRelay] Processed group details:',
              JSON.stringify(details, null, 2),
            );
            setGroupDetails(prev => {
              const updated = new Map(prev);
              updated.set(groupInfo.group_id, details);
              return updated;
            });
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
            onError?.(groupError.reason);
            break;
          }

          case 'TypingUpdate': {
            const typing = data as TypingUpdateResponse;
            setTypingUsers(prev => {
              const updated = new Map(prev);
              const conversationTypers =
                updated.get(typing.conversation_id) ?? new Set();
              if (typing.typing) {
                conversationTypers.add(typing.user_id);
              } else {
                conversationTypers.delete(typing.user_id);
              }
              updated.set(typing.conversation_id, conversationTypers);
              return updated;
            });
            onTypingUpdate?.(
              typing.conversation_id,
              typing.user_id,
              typing.typing,
            );
            break;
          }

          default:
            console.log('[WebSocketRelay] Unhandled message type:', data.type);
        }
      } catch (err) {
        console.error('[WebSocketRelay] Failed to parse message:', err);
      }
    },
    [
      authenticatedUser?.userId,
      authenticatedUser?.username,
      generateMessageId,
      onMessageReceived,
      onGroupMessageReceived,
      onPresenceUpdate,
      onTypingUpdate,
      onError,
    ],
  );

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      console.log('[WebSocketRelay] Already connected');
      return;
    }

    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = null;
    }

    setStatus('connecting');
    setError(null);

    try {
      console.log('[WebSocketRelay] Connecting to:', DEFAULT_RELAY_SERVER_URL);
      const ws = new WebSocket(DEFAULT_RELAY_SERVER_URL);

      ws.onopen = () => {
        console.log('[WebSocketRelay] Connected');
        setStatus('connected');
      };

      ws.onmessage = handleMessage;

      ws.onerror = _event => {
        // React Native's WebSocket error event is an Event-like object; logging it as an error
        // is noisy and not very actionable. Keep it concise.
        console.warn('[WebSocketRelay] WebSocket error event', _event);
        setStatus('error');
        const msg = `WebSocket connection error. Check if server is running at ${DEFAULT_RELAY_SERVER_URL}`;
        setError(msg);
        onError?.(msg);
      };

      ws.onclose = event => {
        const closeCode = event.code;
        const closeReason = event.reason || 'No reason provided';
        console.log('[WebSocketRelay] Closed:', closeCode, closeReason);

        // Code 1006 = Abnormal Closure (connection closed without proper close handshake)
        // This usually means the server isn't running or isn't accessible
        if (closeCode === 1006) {
          const errorMsg = `Connection failed. Server may not be running at ${DEFAULT_RELAY_SERVER_URL}. Check:\n1. Server is running\n2. IP address is correct\n3. Device and server are on same network`;
          setError(errorMsg);
          onError?.(errorMsg);
        }

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
  }, [handleMessage, onError]);

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
      console.log('[WebSocketRelay] ✅ Message sent:', message.type);
      return true;
    } catch (err) {
      console.error('[WebSocketRelay] Send error:', err);
      return false;
    }
  }, []);

  const authenticate = useCallback(
    (token: string) => {
      if (status !== 'connected') {
        console.error(
          '[WebSocketRelay] Cannot authenticate: not connected (status:',
          status,
          ')',
        );
        return false;
      }

      return send({ type: 'Authenticate', token });
    },
    [send, status],
  );

  const sendMessage = useCallback(
    (recipientId: string, content: string) => {
      if (status !== 'authenticated') {
        console.error(
          '[WebSocketRelay] Cannot send message: not authenticated',
        );
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
    },
    [authenticatedUser?.userId, generateMessageId, send, status],
  );

  const checkPresence = useCallback(
    (username: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'CheckPresence',
        username,
      });
    },
    [send, status],
  );

  const setTyping = useCallback(
    (conversationId: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'SetTyping',
        conversation_id: conversationId,
      });
    },
    [send, status],
  );

  const clearTyping = useCallback(
    (conversationId: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'ClearTyping',
        conversation_id: conversationId,
      });
    },
    [send, status],
  );

  const createGroup = useCallback(
    (name: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'CreateGroup',
        name,
      });
    },
    [send, status],
  );

  const sendGroupMessage = useCallback(
    (groupId: string, content: string, replyToMsg?: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      const payload: Record<string, unknown> = {
        type: 'SendGroupMessage',
        group_id: groupId,
        content,
      };

      if (replyToMsg) {
        payload.reply_to_msg = replyToMsg;
      }

      // Store pending message content temporarily (will be matched when GroupMessageSent arrives)
      // We use a unique key based on groupId and timestamp to match with GroupMessageSent response
      const timestamp = Date.now();
      const pendingKey = `${groupId}_${timestamp}_${Math.random()}`;
      pendingSentMessages.current.set(pendingKey, {
        content,
        replyToMsg,
        timestamp: new Date(),
        groupId,
      });

      // Clean up old pending messages (older than 30 seconds)
      const now = Date.now();
      for (const [key, pending] of pendingSentMessages.current.entries()) {
        if (now - pending.timestamp.getTime() > 30000) {
          pendingSentMessages.current.delete(key);
        }
      }

      return send(payload);
    },
    [send, status],
  );

  const addGroupMember = useCallback(
    (groupId: string, username: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'AddGroupMember',
        group_id: groupId,
        username,
      });
    },
    [send, status],
  );

  const removeGroupMember = useCallback(
    (groupId: string, username: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'RemoveGroupMember',
        group_id: groupId,
        username,
      });
    },
    [send, status],
  );

  const leaveGroup = useCallback(
    (groupId: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'LeaveGroup',
        group_id: groupId,
      });
    },
    [send, status],
  );

  const getGroupInfo = useCallback(
    (groupId: string) => {
      if (status !== 'authenticated') {
        return false;
      }

      return send({
        type: 'GetGroupInfo',
        group_id: groupId,
      });
    },
    [send, status],
  );

  const getUserGroups = useCallback(() => {
    if (status !== 'authenticated') {
      return false;
    }

    return send({ type: 'GetUserGroups' });
  }, [send, status]);

  const clearMessages = useCallback(() => {
    setMessages([]);
  }, []);

  const clearGroupMessages = useCallback((groupId: string) => {
    setGroupMessages(prev => {
      const updated = new Map(prev);
      updated.delete(groupId);
      return updated;
    });
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

  const value = useMemo<WebSocketRelayContextValue>(
    () => ({
      status,
      authenticatedUser,
      messages,
      onlineUsers,
      groups,
      groupDetails,
      groupMessages,
      typingUsers,
      error,

      connect,
      disconnect,
      authenticate,
      send,
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
      clearGroupMessages,
    }),
    [
      status,
      authenticatedUser,
      messages,
      onlineUsers,
      groups,
      groupDetails,
      groupMessages,
      typingUsers,
      error,
      connect,
      disconnect,
      authenticate,
      send,
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
      clearGroupMessages,
    ],
  );

  return (
    <WebSocketRelayContext.Provider value={value}>
      {children}
    </WebSocketRelayContext.Provider>
  );
}
