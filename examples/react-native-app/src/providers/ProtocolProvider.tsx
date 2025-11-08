import React, { createContext, useContext, useCallback, useState, useEffect } from 'react';
import { Alert, Platform } from 'react-native';
import { MessagePriority, type ProtocolEvent } from '@offlineprotocol/react-native';
import { useOfflineProtocol } from '../hooks/useOfflineProtocol';
import { generateUserId } from '../utils/user';

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
  batteryLevel: number;
  
  // Actions
  initialize: () => Promise<void>;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  sendMessage: (recipientId: string, content: string, priority?: MessagePriority) => Promise<void>;
  markAsRead: (chatId: string) => void;
  updateUserName: (name: string) => void;
  
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

  const {
    protocol,
    isStarted: isOnline,
    error,
    events,
    insights,
    batteryLevel,
    start: protocolStart,
    stop: protocolStop,
    sendMessage: protocolSendMessage,
    requestPermissions,
  } = useOfflineProtocol({
    appId: 'offline-messenger',
    userId: currentUserId,
    transports: {
      ble: {
        enabled: true,
      },
      internet: {
        enabled: false,
        serverAddress: 'wss://relay.example.com',
        autoReconnect: true,
      },
      wifiDirect: {
        enabled: Platform.OS === 'android',
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
    },
    relay: {
      allowRelay: true,
      maxRelayHops: 3,
      relayPriority: 'medium',
    },
  });

  // Initialize protocol
  const initialize = useCallback(async () => {
    try {
      await requestPermissions();
      setIsInitialized(true);
    } catch (err) {
      console.error('Failed to initialize protocol:', err);
      Alert.alert('Initialization Error', 'Failed to initialize the messaging protocol. Please check permissions.');
    }
  }, [requestPermissions]);

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
      await protocolSendMessage(recipientId, content, priority);
      console.log(`[ProtocolProvider] Message sent successfully to ${recipientId}`);
      
      // Add message to local chat
      const messageId = `msg_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
      const newMessage: Message = {
        id: messageId,
        senderId: currentUserId,
        recipientId,
        content,
        timestamp: Date.now(),
        priority,
        status: 'sending',
        isFromMe: true,
      };

      setChats(prevChats => {
        const existingChatIndex = prevChats.findIndex(chat => chat.peerId === recipientId);
        
        if (existingChatIndex >= 0) {
          const updatedChats = [...prevChats];
          updatedChats[existingChatIndex] = {
            ...updatedChats[existingChatIndex],
            lastMessage: newMessage,
            messages: [...updatedChats[existingChatIndex].messages, newMessage],
          };
          return updatedChats;
        } else {
          // Create new chat
          const newChat: Chat = {
            id: recipientId,
            peerId: recipientId,
            peerName: `User ${recipientId.slice(-4)}`,
            lastMessage: newMessage,
            unreadCount: 0,
            isOnline: false,
            messages: [newMessage],
          };
          return [...prevChats, newChat];
        }
      });
    } catch (err) {
      console.error('Failed to send message:', err);
      Alert.alert('Send Error', 'Failed to send message. Please try again.');
    }
  }, [protocolSendMessage, currentUserId]);

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

  // Process protocol events to update contacts and messages
  useEffect(() => {
    const processEvents = () => {
      const discoveredPeers = new Set<string>();
      const receivedMessages: Message[] = [];

      events.forEach((event) => {
        switch (event.type) {
          case 'neighbor_discovered':
            const discoveredPeerId = (event as any).peer_id;
            console.log(`[ProtocolProvider] Neighbor discovered: ${discoveredPeerId}`);
            discoveredPeers.add(discoveredPeerId);
            break;
          
          case 'neighbor_lost':
            const lostPeerId = (event as any).peer_id;
            console.log(`[ProtocolProvider] Neighbor lost: ${lostPeerId}`);
            discoveredPeers.delete(lostPeerId);
            break;
          
          case 'message_received':
            const msgEvent = event as any;
            console.log(`[ProtocolProvider] Received message from ${msgEvent.sender}: "${msgEvent.content}"`);
            const receivedMessage: Message = {
              id: `msg_${msgEvent.timestamp}_${Math.random().toString(36).substr(2, 9)}`,
              senderId: msgEvent.sender,
              recipientId: currentUserId,
              content: msgEvent.content,
              timestamp: msgEvent.timestamp || Date.now(),
              priority: msgEvent.priority || MessagePriority.Medium,
              status: 'delivered',
              isFromMe: false,
            };
            receivedMessages.push(receivedMessage);
            break;
        }
      });

      // Update contacts
      setContacts(prevContacts => {
        const updatedContacts = [...prevContacts];
        const existingPeerIds = new Set(prevContacts.map(c => c.id));

        // Add new discovered peers
        discoveredPeers.forEach(peerId => {
          if (!existingPeerIds.has(peerId)) {
            updatedContacts.push({
              id: peerId,
              name: `User ${peerId.slice(-4)}`,
              isOnline: true,
              lastSeen: Date.now(),
              signalStrength: Math.random(),
              distance: Math.random() > 0.6 ? 'near' : Math.random() > 0.3 ? 'medium' : 'far',
            });
          }
        });

        // Update online status
        return updatedContacts.map(contact => ({
          ...contact,
          isOnline: discoveredPeers.has(contact.id),
          lastSeen: discoveredPeers.has(contact.id) ? Date.now() : contact.lastSeen,
        }));
      });

      // Update chats with received messages
      if (receivedMessages.length > 0) {
        setChats(prevChats => {
          const updatedChats = [...prevChats];
          
          receivedMessages.forEach(message => {
            const existingChatIndex = updatedChats.findIndex(
              chat => chat.peerId === message.senderId
            );
            
            if (existingChatIndex >= 0) {
              updatedChats[existingChatIndex] = {
                ...updatedChats[existingChatIndex],
                lastMessage: message,
                unreadCount: updatedChats[existingChatIndex].unreadCount + 1,
                messages: [...updatedChats[existingChatIndex].messages, message],
              };
            } else {
              // Create new chat for new sender
              const newChat: Chat = {
                id: message.senderId,
                peerId: message.senderId,
                peerName: `User ${message.senderId.slice(-4)}`,
                lastMessage: message,
                unreadCount: 1,
                isOnline: discoveredPeers.has(message.senderId),
                messages: [message],
              };
              updatedChats.push(newChat);
            }
          });
          
          return updatedChats;
        });
      }
    };

    processEvents();
  }, [events, currentUserId]);

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
    initialize,
    start,
    stop,
    sendMessage,
    markAsRead,
    updateUserName,
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
