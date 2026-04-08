import React, {useState, useRef, useEffect} from 'react';
import {
  View,
  Text,
  TextInput,
  TouchableOpacity,
  FlatList,
  StyleSheet,
  Alert,
} from 'react-native';
import {useProtocol} from '../context/ProtocolContext';
import {MessageBubble} from '../components/MessageBubble';
import {formatUserId, generateAvatarColor, getUserInitials} from '../utils';
import type {Chat, ChatMessage} from '../types';

export function ChatScreen() {
  const [selectedPeerId, setSelectedPeerId] = useState<string | null>(null);
  const [peerInput, setPeerInput] = useState('');
  const [messageText, setMessageText] = useState('');
  const {chats, sendMessage, markChatRead, userId} = useProtocol();
  const listRef = useRef<FlatList>(null);

  // Mark as read when viewing a chat
  useEffect(() => {
    if (selectedPeerId) {
      markChatRead(selectedPeerId);
    }
  }, [selectedPeerId, markChatRead]);

  // ─── Chat Detail ─────────────────────────────────────────

  if (selectedPeerId) {
    const chat = chats.get(selectedPeerId);
    const messages = chat?.messages || [];

    const handleSend = async () => {
      if (!messageText.trim()) {return;}
      try {
        await sendMessage(selectedPeerId, messageText.trim());
        setMessageText('');
        setTimeout(() => {
          listRef.current?.scrollToEnd({animated: true});
        }, 100);
      } catch (error: any) {
        Alert.alert('Send Error', error.message);
      }
    };

    return (
      <View style={styles.container}>
        {/* Header */}
        <View style={styles.chatHeader}>
          <TouchableOpacity
            style={styles.backButton}
            onPress={() => setSelectedPeerId(null)}>
            <Text style={styles.backText}>Back</Text>
          </TouchableOpacity>
          <Text style={styles.chatHeaderName} numberOfLines={1}>
            {formatUserId(selectedPeerId)}
          </Text>
          <View style={styles.headerSpacer} />
        </View>

        {/* Messages */}
        <FlatList
          ref={listRef}
          data={messages}
          keyExtractor={item => item.id}
          renderItem={({item}) => (
            <MessageBubble message={item} isOwnMessage={item.isOutgoing} />
          )}
          contentContainerStyle={styles.messageList}
          onContentSizeChange={() => listRef.current?.scrollToEnd({animated: false})}
          ListEmptyComponent={
            <Text style={styles.emptyChat}>
              Send a message to start the conversation.
            </Text>
          }
        />

        {/* Input */}
        <View style={styles.inputRow}>
          <TextInput
            style={styles.messageInput}
            placeholder="Type a message..."
            value={messageText}
            onChangeText={setMessageText}
            onSubmitEditing={handleSend}
            returnKeyType="send"
          />
          <TouchableOpacity style={styles.sendButton} onPress={handleSend}>
            <Text style={styles.sendButtonText}>Send</Text>
          </TouchableOpacity>
        </View>
      </View>
    );
  }

  // ─── Chat List ───────────────────────────────────────────

  const chatList = Array.from(chats.values());

  const handleStartChat = () => {
    if (!peerInput.trim()) {
      Alert.alert('Error', 'Enter a peer ID to start chatting.');
      return;
    }
    setSelectedPeerId(peerInput.trim());
    setPeerInput('');
  };

  const renderChat = ({item}: {item: Chat}) => {
    const lastMsg = item.messages[item.messages.length - 1];
    const color = generateAvatarColor(item.peerId);
    const initials = getUserInitials(item.peerId);

    return (
      <TouchableOpacity
        style={styles.chatRow}
        onPress={() => setSelectedPeerId(item.peerId)}>
        <View style={[styles.avatar, {backgroundColor: color}]}>
          <Text style={styles.avatarText}>{initials}</Text>
        </View>
        <View style={styles.chatInfo}>
          <Text style={styles.chatPeerId} numberOfLines={1}>
            {formatUserId(item.peerId)}
          </Text>
          {lastMsg && (
            <Text style={styles.chatPreview} numberOfLines={1}>
              {lastMsg.isOutgoing ? 'You: ' : ''}{lastMsg.content}
            </Text>
          )}
        </View>
        {item.unreadCount > 0 && (
          <View style={styles.unreadBadge}>
            <Text style={styles.unreadText}>
              {item.unreadCount > 9 ? '9+' : item.unreadCount}
            </Text>
          </View>
        )}
      </TouchableOpacity>
    );
  };

  return (
    <View style={styles.container}>
      {/* Peer Input */}
      <View style={styles.peerInputSection}>
        <TextInput
          style={styles.peerInput}
          placeholder="Enter peer's User ID"
          value={peerInput}
          onChangeText={setPeerInput}
          autoCapitalize="none"
          autoCorrect={false}
          onSubmitEditing={handleStartChat}
          returnKeyType="go"
        />
        <TouchableOpacity style={styles.chatButton} onPress={handleStartChat}>
          <Text style={styles.chatButtonText}>Chat</Text>
        </TouchableOpacity>
      </View>

      {/* Your ID */}
      <View style={styles.yourIdSection}>
        <Text style={styles.yourIdLabel}>Your ID:</Text>
        <Text style={styles.yourIdValue} selectable>{userId}</Text>
      </View>

      {/* Chat List */}
      {chatList.length === 0 ? (
        <View style={styles.emptyContainer}>
          <Text style={styles.emptyTitle}>No Chats Yet</Text>
          <Text style={styles.emptySubtitle}>
            Share your User ID with a peer and start chatting!
          </Text>
        </View>
      ) : (
        <FlatList
          data={chatList}
          keyExtractor={item => item.peerId}
          renderItem={renderChat}
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  // ─── Peer Input ────────────────────────────────────────
  peerInputSection: {
    flexDirection: 'row',
    padding: 12,
    gap: 8,
    backgroundColor: '#FFFFFF',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  peerInput: {
    flex: 1,
    backgroundColor: '#F2F2F7',
    borderRadius: 10,
    paddingHorizontal: 14,
    paddingVertical: 10,
    fontSize: 14,
  },
  chatButton: {
    backgroundColor: '#7B1FA2',
    borderRadius: 10,
    paddingHorizontal: 16,
    justifyContent: 'center',
  },
  chatButtonText: {
    color: '#FFFFFF',
    fontWeight: '600',
    fontSize: 14,
  },
  yourIdSection: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 8,
    backgroundColor: '#F2F2F7',
    gap: 6,
  },
  yourIdLabel: {
    fontSize: 12,
    color: '#8E8E93',
    fontWeight: '600',
  },
  yourIdValue: {
    fontSize: 12,
    color: '#7B1FA2',
    fontFamily: 'monospace',
    flex: 1,
  },
  // ─── Chat List ─────────────────────────────────────────
  chatRow: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 12,
    backgroundColor: '#FFFFFF',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  avatar: {
    width: 44,
    height: 44,
    borderRadius: 22,
    alignItems: 'center',
    justifyContent: 'center',
  },
  avatarText: {
    color: '#FFFFFF',
    fontSize: 16,
    fontWeight: '700',
  },
  chatInfo: {
    flex: 1,
    marginLeft: 12,
  },
  chatPeerId: {
    fontSize: 15,
    fontWeight: '600',
    color: '#1C1C1E',
  },
  chatPreview: {
    fontSize: 13,
    color: '#8E8E93',
    marginTop: 2,
  },
  unreadBadge: {
    backgroundColor: '#7B1FA2',
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
  // ─── Chat Detail ───────────────────────────────────────
  chatHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 12,
    paddingVertical: 10,
    backgroundColor: '#FFFFFF',
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#E5E5E5',
  },
  backButton: {
    paddingRight: 12,
  },
  backText: {
    fontSize: 16,
    color: '#7B1FA2',
    fontWeight: '600',
  },
  chatHeaderName: {
    flex: 1,
    fontSize: 16,
    fontWeight: '700',
    color: '#1C1C1E',
    textAlign: 'center',
  },
  headerSpacer: {
    width: 48,
  },
  messageList: {
    padding: 12,
    flexGrow: 1,
  },
  emptyChat: {
    textAlign: 'center',
    color: '#8E8E93',
    marginTop: 40,
    fontSize: 14,
  },
  inputRow: {
    flexDirection: 'row',
    padding: 12,
    gap: 8,
    backgroundColor: '#FFFFFF',
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: '#E5E5E5',
  },
  messageInput: {
    flex: 1,
    backgroundColor: '#F2F2F7',
    borderRadius: 20,
    paddingHorizontal: 16,
    paddingVertical: 10,
    fontSize: 15,
  },
  sendButton: {
    backgroundColor: '#7B1FA2',
    borderRadius: 20,
    paddingHorizontal: 20,
    justifyContent: 'center',
  },
  sendButtonText: {
    color: '#FFFFFF',
    fontWeight: '600',
  },
  // ─── Empty State ───────────────────────────────────────
  emptyContainer: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
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
