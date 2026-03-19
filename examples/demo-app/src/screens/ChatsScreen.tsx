import React, {useState, useRef, useEffect} from 'react';
import {
  View,
  Text,
  FlatList,
  TouchableOpacity,
  StyleSheet,
  Alert,
} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {Avatar} from '../components/Avatar';
import {PresenceIndicator} from '../components/PresenceIndicator';
import {MessageBubble} from '../components/MessageBubble';
import {QuickMessages} from '../components/QuickMessages';
import {formatRelativeTime, formatUserId} from '../utils';
import type {Chat, ChatMessage, PresenceStatus} from '../types';

interface ChatsScreenProps {
  initialPeerId?: string | null;
  onClearInitialPeer?: () => void;
}

export function ChatsScreen({initialPeerId, onClearInitialPeer}: ChatsScreenProps) {
  const [selectedPeerId, setSelectedPeerId] = useState<string | null>(null);
  const {chats, contacts, groups, sendMessage, markChatRead, userId, unblockUser, forwardMessage, forwardMessageToGroup, typingPeers} = useProtocol();
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
    const isBlocked = contact?.isBlocked ?? false;

    const handleSend = (text: string, priority: 'medium' | 'critical') => {
      sendMessage(selectedPeerId, text, priority);
      // Scroll to bottom after a short delay
      setTimeout(() => {
        listRef.current?.scrollToEnd({animated: true});
      }, 100);
    };

    const handleForward = (msg: ChatMessage) => {
      const contactList = Array.from(contacts.values()).filter(
        c => c.hasSession && !c.isBlocked && c.peerId !== selectedPeerId,
      );
      const groupList = Array.from(groups.values());

      const buttons: any[] = [];

      contactList.forEach(c => {
        buttons.push({
          text: c.name || c.peerId,
          onPress: () => forwardMessage(msg, c.peerId),
        });
      });

      groupList.forEach(g => {
        buttons.push({
          text: `[Group] ${g.name}`,
          onPress: () => forwardMessageToGroup(msg, g.id),
        });
      });

      if (buttons.length === 0) {
        Alert.alert('No Recipients', 'No contacts or groups available to forward to.');
        return;
      }

      buttons.push({text: 'Cancel', style: 'cancel'});
      Alert.alert('Forward to...', msg.content.slice(0, 60), buttons);
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
            <Text style={[styles.chatHeaderName, isBlocked && styles.blockedHeaderName]} numberOfLines={1}>
              {contactName}
            </Text>
            {contact && !isBlocked && (
              <PresenceIndicator
                presenceStatus={contact.presenceStatus}
                lastSeen={contact.lastSeen}
                isTyping={typingPeers.has(selectedPeerId)}
              />
            )}
            {isBlocked && (
              <Text style={styles.blockedLabel}>Blocked</Text>
            )}
          </View>
          <Text style={styles.headerLock}>{isBlocked ? '🚫' : '🔒'}</Text>
        </View>

        {/* Messages */}
        <View style={{flex: 1, opacity: isBlocked ? 0.4 : 1}}>
          <FlatList
            ref={listRef}
            data={messages}
            renderItem={({item}) => (
              <MessageBubble
                message={item}
                onLongPress={() => handleForward(item)}
              />
            )}
            keyExtractor={item => item.id}
            contentContainerStyle={styles.messageList}
            scrollEnabled={!isBlocked}
            onContentSizeChange={() => {
              if (messages.length > 0) {
                listRef.current?.scrollToEnd({animated: false});
              }
            }}
            ListEmptyComponent={
              <View style={styles.emptyChat}>
                <Text style={styles.emptyChatText}>
                  {isBlocked
                    ? `You have blocked ${contactName}`
                    : `Send an encrypted message to ${contactName}`}
                </Text>
              </View>
            }
          />
        </View>

        {/* Bottom area: Quick Messages or Blocked Banner */}
        {isBlocked ? (
          <View style={styles.blockedBanner}>
            <Text style={styles.blockedBannerText}>
              You blocked this user. Unblock to send messages.
            </Text>
            <TouchableOpacity
              style={styles.unblockButton}
              onPress={() => unblockUser(selectedPeerId)}>
              <Text style={styles.unblockButtonText}>Unblock</Text>
            </TouchableOpacity>
          </View>
        ) : (
          <QuickMessages onSend={handleSend} />
        )}
      </View>
    );
  }

  // ─── Chat List ───────────────────────────────────────────

  const chatList: (Chat & {contactName: string; isNearby: boolean; lastSeen: number; isBlocked: boolean; presenceStatus: PresenceStatus})[] = [];

  // Include chats with messages
  for (const [peerId, chat] of chats) {
    const contact = contacts.get(peerId);
    chatList.push({
      ...chat,
      contactName: contact?.name || formatUserId(peerId),
      isNearby: contact?.isNearby || false,
      lastSeen: contact?.lastSeen || 0,
      isBlocked: contact?.isBlocked || false,
      presenceStatus: contact?.presenceStatus || 'offline',
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
        isBlocked: false,
        presenceStatus: contact.presenceStatus,
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
            style={[styles.chatRow, item.isBlocked && styles.chatRowBlocked]}
            onPress={() => handleOpenChat(item.peerId)}>
            <Avatar userId={item.peerId} name={item.contactName} size={48} />
            <View style={styles.chatRowContent}>
              <View style={styles.chatRowTop}>
                <Text style={[styles.chatName, item.isBlocked && styles.chatNameBlocked]} numberOfLines={1}>
                  {item.contactName}
                </Text>
                {item.isBlocked ? (
                  <Text style={styles.chatBlockedBadge}>Blocked</Text>
                ) : lastMsg ? (
                  <Text style={styles.chatTime}>
                    {formatRelativeTime(lastMsg.timestamp)}
                  </Text>
                ) : null}
              </View>
              <View style={styles.chatRowBottom}>
                <View style={styles.chatPreviewRow}>
                  {!item.isBlocked && (
                    <PresenceIndicator
                      presenceStatus={item.presenceStatus}
                      lastSeen={item.lastSeen}
                      isTyping={typingPeers.has(item.peerId)}
                      compact
                    />
                  )}
                  <Text style={styles.chatPreview} numberOfLines={1}>
                    {item.isBlocked
                      ? 'Tap to view or unblock'
                      : lastMsg
                        ? `${lastMsg.isOutgoing ? 'You: ' : ''}${lastMsg.content}`
                        : 'Tap to send a message'}
                  </Text>
                </View>
                {!item.isBlocked && item.unreadCount > 0 && (
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
  chatRowBlocked: {
    opacity: 0.6,
  },
  chatNameBlocked: {
    color: '#8E8E93',
  },
  chatBlockedBadge: {
    fontSize: 11,
    color: '#FF3B30',
    fontWeight: '600',
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
  blockedHeaderName: {
    color: '#8E8E93',
  },
  blockedLabel: {
    fontSize: 12,
    color: '#FF3B30',
    fontWeight: '500',
  },
  blockedBanner: {
    backgroundColor: '#FFF0F0',
    paddingHorizontal: 16,
    paddingVertical: 14,
    borderTopWidth: 1,
    borderTopColor: '#FFD5D5',
    alignItems: 'center',
    gap: 10,
  },
  blockedBannerText: {
    fontSize: 14,
    color: '#8E8E93',
    textAlign: 'center',
  },
  unblockButton: {
    backgroundColor: '#007AFF',
    paddingHorizontal: 24,
    paddingVertical: 10,
    borderRadius: 20,
  },
  unblockButtonText: {
    color: '#FFFFFF',
    fontSize: 14,
    fontWeight: '600',
  },
});
