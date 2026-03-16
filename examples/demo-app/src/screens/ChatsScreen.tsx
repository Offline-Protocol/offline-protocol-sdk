import React, {useState, useRef, useEffect} from 'react';
import {
  View,
  Text,
  FlatList,
  TouchableOpacity,
  StyleSheet,
} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {Avatar} from '../components/Avatar';
import {PresenceIndicator} from '../components/PresenceIndicator';
import {MessageBubble} from '../components/MessageBubble';
import {QuickMessages} from '../components/QuickMessages';
import {formatRelativeTime, formatUserId} from '../utils';
import type {Chat} from '../types';

interface ChatsScreenProps {
  initialPeerId?: string | null;
  onClearInitialPeer?: () => void;
}

export function ChatsScreen({initialPeerId, onClearInitialPeer}: ChatsScreenProps) {
  const [selectedPeerId, setSelectedPeerId] = useState<string | null>(null);
  const {chats, contacts, sendMessage, markChatRead, userId} = useProtocol();
  const listRef = useRef<FlatList>(null);

  // Handle navigation from People tab
  useEffect(() => {
    if (initialPeerId) {
      setSelectedPeerId(initialPeerId);
      markChatRead(initialPeerId);
      onClearInitialPeer?.();
    }
  }, [initialPeerId, markChatRead, onClearInitialPeer]);

  // ─── Chat Detail ─────────────────────────────────────────

  if (selectedPeerId) {
    const chat = chats.get(selectedPeerId);
    const contact = contacts.get(selectedPeerId);
    const messages = chat?.messages || [];
    const contactName = contact?.name || formatUserId(selectedPeerId);

    const handleSend = (text: string, priority: 'medium' | 'critical') => {
      sendMessage(selectedPeerId, text, priority);
      // Scroll to bottom after a short delay
      setTimeout(() => {
        listRef.current?.scrollToEnd({animated: true});
      }, 100);
    };

    return (
      <View style={styles.container}>
        {/* Header */}
        <View style={styles.chatHeader}>
          <TouchableOpacity
            style={styles.backButton}
            onPress={() => setSelectedPeerId(null)}>
            <Text style={styles.backText}>{'‹ Back'}</Text>
          </TouchableOpacity>
          <View style={styles.chatHeaderInfo}>
            <Text style={styles.chatHeaderName} numberOfLines={1}>
              {contactName}
            </Text>
            {contact && (
              <PresenceIndicator
                isNearby={contact.isNearby}
                lastSeen={contact.lastSeen}
              />
            )}
          </View>
          <Text style={styles.headerLock}>🔒</Text>
        </View>

        {/* Messages */}
        <FlatList
          ref={listRef}
          data={messages}
          renderItem={({item}) => <MessageBubble message={item} />}
          keyExtractor={item => item.id}
          contentContainerStyle={styles.messageList}
          onContentSizeChange={() => {
            if (messages.length > 0) {
              listRef.current?.scrollToEnd({animated: false});
            }
          }}
          ListEmptyComponent={
            <View style={styles.emptyChat}>
              <Text style={styles.emptyChatText}>
                Send an encrypted message to {contactName}
              </Text>
            </View>
          }
        />

        {/* Quick Messages */}
        <QuickMessages onSend={handleSend} />
      </View>
    );
  }

  // ─── Chat List ───────────────────────────────────────────

  const chatList: (Chat & {contactName: string; isNearby: boolean; lastSeen: number})[] = [];

  // Include chats with messages
  for (const [peerId, chat] of chats) {
    const contact = contacts.get(peerId);
    chatList.push({
      ...chat,
      contactName: contact?.name || formatUserId(peerId),
      isNearby: contact?.isNearby || false,
      lastSeen: contact?.lastSeen || 0,
    });
  }

  // Include contacts without chats yet
  for (const [peerId, contact] of contacts) {
    if (!chats.has(peerId) && contact.hasSession && !contact.isBlocked) {
      chatList.push({
        peerId,
        messages: [],
        unreadCount: 0,
        contactName: contact.name,
        isNearby: contact.isNearby,
        lastSeen: contact.lastSeen,
      });
    }
  }

  // Sort by last message time (most recent first)
  chatList.sort((a, b) => {
    const aTime = a.messages.length > 0 ? a.messages[a.messages.length - 1].timestamp : 0;
    const bTime = b.messages.length > 0 ? b.messages[b.messages.length - 1].timestamp : 0;
    return bTime - aTime;
  });

  const handleOpenChat = (peerId: string) => {
    setSelectedPeerId(peerId);
    markChatRead(peerId);
  };

  if (chatList.length === 0) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyEmoji}>💬</Text>
        <Text style={styles.emptyTitle}>No Chats Yet</Text>
        <Text style={styles.emptySubtitle}>
          Connect with nearby peers in the People tab to start chatting.
        </Text>
      </View>
    );
  }

  return (
    <FlatList
      data={chatList}
      renderItem={({item}) => {
        const lastMsg = item.messages.length > 0 ? item.messages[item.messages.length - 1] : null;

        return (
          <TouchableOpacity
            style={styles.chatRow}
            onPress={() => handleOpenChat(item.peerId)}>
            <Avatar userId={item.peerId} name={item.contactName} size={48} />
            <View style={styles.chatRowContent}>
              <View style={styles.chatRowTop}>
                <Text style={styles.chatName} numberOfLines={1}>{item.contactName}</Text>
                {lastMsg && (
                  <Text style={styles.chatTime}>
                    {formatRelativeTime(lastMsg.timestamp)}
                  </Text>
                )}
              </View>
              <View style={styles.chatRowBottom}>
                <View style={styles.chatPreviewRow}>
                  <PresenceIndicator
                    isNearby={item.isNearby}
                    lastSeen={item.lastSeen}
                    compact
                  />
                  <Text style={styles.chatPreview} numberOfLines={1}>
                    {lastMsg
                      ? `${lastMsg.isOutgoing ? 'You: ' : ''}${lastMsg.content}`
                      : 'Tap to send a message'}
                  </Text>
                </View>
                {item.unreadCount > 0 && (
                  <View style={styles.unreadBadge}>
                    <Text style={styles.unreadText}>
                      {item.unreadCount > 9 ? '9+' : item.unreadCount}
                    </Text>
                  </View>
                )}
              </View>
            </View>
          </TouchableOpacity>
        );
      }}
      keyExtractor={item => item.peerId}
      contentContainerStyle={styles.list}
    />
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#FFFFFF',
  },
  list: {
    paddingBottom: 16,
  },
  chatRow: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 12,
  },
  chatRowContent: {
    flex: 1,
    gap: 3,
  },
  chatRowTop: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  chatName: {
    fontSize: 16,
    fontWeight: '600',
    color: '#1C1C1E',
    flex: 1,
  },
  chatTime: {
    fontSize: 12,
    color: '#8E8E93',
  },
  chatRowBottom: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  chatPreviewRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    flex: 1,
  },
  chatPreview: {
    fontSize: 14,
    color: '#8E8E93',
    flex: 1,
  },
  unreadBadge: {
    backgroundColor: '#007AFF',
    borderRadius: 10,
    minWidth: 20,
    height: 20,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 6,
  },
  unreadText: {
    color: '#FFFFFF',
    fontSize: 11,
    fontWeight: '700',
  },
  // Chat detail styles
  chatHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#FFFFFF',
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
    gap: 8,
  },
  backButton: {
    paddingVertical: 4,
    paddingRight: 8,
  },
  backText: {
    fontSize: 17,
    color: '#007AFF',
    fontWeight: '500',
  },
  chatHeaderInfo: {
    flex: 1,
    alignItems: 'center',
  },
  chatHeaderName: {
    fontSize: 16,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  headerLock: {
    fontSize: 16,
  },
  messageList: {
    flexGrow: 1,
    paddingVertical: 8,
  },
  emptyChat: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
  },
  emptyChatText: {
    fontSize: 14,
    color: '#8E8E93',
    textAlign: 'center',
  },
  empty: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
  },
  emptyEmoji: {
    fontSize: 48,
    marginBottom: 16,
  },
  emptyTitle: {
    fontSize: 18,
    fontWeight: '600',
    color: '#1C1C1E',
    marginBottom: 8,
  },
  emptySubtitle: {
    fontSize: 14,
    color: '#8E8E93',
    textAlign: 'center',
    lineHeight: 20,
  },
});
