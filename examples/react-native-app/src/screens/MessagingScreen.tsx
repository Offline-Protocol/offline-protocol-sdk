import React, { useCallback, useMemo, useRef, useState } from 'react';
import {
  KeyboardAvoidingView,
  Platform,
  View,
  Text,
  TextInput,
  TouchableOpacity,
  TouchableWithoutFeedback,
  Keyboard,
  StyleSheet,
  ScrollView,
  useWindowDimensions,
  InputAccessoryView,
} from 'react-native';
import { SafeAreaView, useSafeAreaInsets } from 'react-native-safe-area-context';
import { MessagePriority, type ProtocolEvent } from '@offlineprotocol/react-native';
import { MessageList } from '../components/MessageList';
import type { FileTransferState } from '../types/runtime';

interface MessagingScreenProps {
  events: ProtocolEvent[];
  currentUserId: string;
  onSendMessage: (recipient: string, content: string, priority: MessagePriority) => Promise<void>;
  isStarted: boolean;
  fileTransfers?: FileTransferState[];
  onOpenChat?: (peerId: string) => void;
}

interface PriorityOption {
  label: string;
  value: MessagePriority;
  helper: string;
}

export function MessagingScreen({
  events,
  currentUserId,
  onSendMessage,
  isStarted,
  fileTransfers = [],
  onOpenChat,
}: MessagingScreenProps) {
  const [recipient, setRecipient] = useState('');
  const [message, setMessage] = useState('');
  const [priority, setPriority] = useState<MessagePriority>(MessagePriority.Medium);
  const [sending, setSending] = useState(false);

  const messageInputRef = useRef<TextInput | null>(null);

  const insets = useSafeAreaInsets();
  const { width } = useWindowDimensions();
  const isCompact = width < 768;
  const isVerySmall = width < 380;
  const keyboardVerticalOffset = Platform.OS === 'ios' ? Math.max(insets.top, 12) + 48 : 0;

  const discoveredPeers = useMemo(() => {
    const peers = new Set<string>();

    events.forEach((event) => {
      if (event.type === 'neighbor_discovered') {
        peers.add((event as any).peer_id);
      } else if (event.type === 'neighbor_lost') {
        peers.delete((event as any).peer_id);
      }
    });

    return Array.from(peers);
  }, [events]);

  // Get peer conversations with last message info
  const peerConversations = useMemo(() => {
    const conversations = new Map<string, {
      peerId: string;
      lastMessage?: string;
      lastMessageTime?: number;
      unreadCount: number;
      isOnline: boolean;
    }>();

    // Initialize with discovered peers
    discoveredPeers.forEach(peerId => {
      conversations.set(peerId, {
        peerId,
        unreadCount: 0,
        isOnline: true,
      });
    });

    // Process messages to get last message info
    [...events].reverse().forEach((event) => {
      if (event.type === 'message_sent') {
        const e = event as any;
        if (!conversations.has(e.recipient)) {
          conversations.set(e.recipient, {
            peerId: e.recipient,
            unreadCount: 0,
            isOnline: discoveredPeers.includes(e.recipient),
          });
        }
        const conv = conversations.get(e.recipient)!;
        if (!conv.lastMessageTime || e.timestamp > conv.lastMessageTime) {
          conv.lastMessage = `You: ${e.content}`;
          conv.lastMessageTime = e.timestamp;
        }
      } else if (event.type === 'message_received') {
        const e = event as any;
        if (!conversations.has(e.sender)) {
          conversations.set(e.sender, {
            peerId: e.sender,
            unreadCount: 0,
            isOnline: discoveredPeers.includes(e.sender),
          });
        }
        const conv = conversations.get(e.sender)!;
        if (!conv.lastMessageTime || e.timestamp > conv.lastMessageTime) {
          conv.lastMessage = e.content;
          conv.lastMessageTime = e.timestamp;
        }
      }
    });

    return Array.from(conversations.values())
      .sort((a, b) => (b.lastMessageTime || 0) - (a.lastMessageTime || 0));
  }, [events, discoveredPeers]);

  const activeTransfers = useMemo(
    () =>
      fileTransfers
        .filter((transfer) => transfer.status === 'pending')
        .sort((a, b) => b.lastUpdated - a.lastUpdated)
        .slice(0, 4),
    [fileTransfers]
  );

  const quickTemplates = useMemo(
    () => [
      'Ping received. Confirming your status?',
      'Need assistance near the rendezvous point.',
      'Heading to the fallback route. Acknowledge.',
      'Logistics update required — respond when synced.',
    ],
    []
  );

  const priorityOptions = useMemo<PriorityOption[]>(
    () => [
      { label: 'Low', value: MessagePriority.Low, helper: 'Background delivery' },
      { label: 'Medium', value: MessagePriority.Medium, helper: 'Balanced default' },
      { label: 'High', value: MessagePriority.High, helper: 'Accelerated routing' },
      { label: 'Critical', value: MessagePriority.Critical, helper: 'Bypass queue' },
    ],
    []
  );

  const priorityLabel = useMemo(() => {
    switch (priority) {
      case MessagePriority.Low:
        return 'Low';
      case MessagePriority.High:
        return 'High';
      case MessagePriority.Critical:
        return 'Critical';
      default:
        return 'Medium';
    }
  }, [priority]);

  const priorityDescription = useMemo(() => {
    switch (priority) {
      case MessagePriority.Low:
        return 'Queues for resilient delivery when bandwidth is tight.';
      case MessagePriority.High:
        return 'Moves ahead in the queue to reach peers faster.';
      case MessagePriority.Critical:
        return 'Skips batching and attempts immediate relay.';
      default:
        return 'Balances reliability with speed for general updates.';
    }
  }, [priority]);

  const isSendDisabled =
    !isStarted || sending || !recipient.trim().length || !message.trim().length;

  const handleSelectPeer = useCallback((peerId: string) => {
    setRecipient(peerId);
    requestAnimationFrame(() => {
      messageInputRef.current?.focus();
    });
  }, []);

  const handleInsertTemplate = useCallback(
    (template: string) => {
      if (!isStarted) {
        return;
      }

      setMessage(template);
      requestAnimationFrame(() => {
        messageInputRef.current?.focus();
      });
    },
    [isStarted]
  );

  const handleClearRecipient = useCallback(() => {
    setRecipient('');
  }, []);

  const handleSend = useCallback(async () => {
    const trimmedRecipient = recipient.trim();
    const trimmedMessage = message.trim();

    if (!isStarted || sending || !trimmedRecipient || !trimmedMessage) {
      return;
    }

    setSending(true);
    try {
      const messageId = await onSendMessage(trimmedRecipient, trimmedMessage, priority);
      if (messageId) {
      setMessage('');
      Keyboard.dismiss();
        console.log('Message sent successfully:', messageId);
      } else {
        console.warn('Failed to send message - no message ID returned');
      }
    } catch (error) {
      console.error('Error sending message:', error);
    } finally {
      setSending(false);
    }
  }, [isStarted, message, onSendMessage, priority, recipient, sending]);

  const sendHint = isSendDisabled
    ? 'Add a recipient and message to enable sending.'
    : 'Ready to dispatch over the offline mesh.';

  const formatConversationTime = (timestamp: number): string => {
    const date = new Date(timestamp);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / (1000 * 60));
    const diffHours = Math.floor(diffMs / (1000 * 60 * 60));
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

    if (diffMins < 1) return 'now';
    if (diffMins < 60) return `${diffMins}m`;
    if (diffHours < 24) return `${diffHours}h`;
    if (diffDays < 7) return `${diffDays}d`;
    return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
  };

  const inputAccessoryViewID = 'messageInputAccessory';

  const renderInputAccessory = () => (
    <InputAccessoryView nativeID={inputAccessoryViewID}>
      <View style={styles.inputAccessory}>
        <TouchableOpacity
          style={styles.accessoryButton}
          onPress={() => Keyboard.dismiss()}
        >
          <Text style={styles.accessoryButtonText}>Done</Text>
        </TouchableOpacity>
        {isStarted && recipient.trim() && message.trim() && (
          <TouchableOpacity
            style={[styles.accessoryButton, styles.accessoryButtonPrimary]}
            onPress={handleSend}
            disabled={sending}
          >
            <Text style={[styles.accessoryButtonText, styles.accessoryButtonTextPrimary]}>
              {sending ? 'Sending…' : 'Send'}
            </Text>
          </TouchableOpacity>
        )}
      </View>
    </InputAccessoryView>
  );

  const renderCompactComposer = () => (
    <View style={styles.compactComposerCard}>
      {/* Recipient row */}
      <View style={styles.compactInputRow}>
        <Text style={styles.compactInputLabel}>To:</Text>
        <TextInput
          style={[styles.compactRecipientInput, !isStarted && styles.compactInputDisabled]}
          value={recipient}
          onChangeText={setRecipient}
          placeholder={isStarted ? "Enter peer ID" : "Start protocol first"}
          placeholderTextColor="#94a3b8"
          editable={isStarted}
          autoCapitalize="none"
          autoCorrect={false}
          returnKeyType="next"
          onSubmitEditing={() => messageInputRef.current?.focus()}
          inputAccessoryViewID={isCompact ? inputAccessoryViewID : undefined}
        />
        {recipient ? (
          <TouchableOpacity
            style={styles.compactClearButton}
            onPress={handleClearRecipient}
          >
            <Text style={styles.compactClearText}>×</Text>
          </TouchableOpacity>
        ) : null}
      </View>

      {/* Message input row */}
      <View style={styles.compactMessageRow}>
        <TextInput
          ref={messageInputRef}
          style={[styles.compactMessageInput, !isStarted && styles.compactInputDisabled]}
          value={message}
          onChangeText={setMessage}
          placeholder={isStarted ? "Type message..." : "Start protocol to send"}
          placeholderTextColor="#94a3b8"
          editable={isStarted}
          multiline
          maxLength={500}
          returnKeyType="send"
          onSubmitEditing={handleSend}
          inputAccessoryViewID={isCompact ? inputAccessoryViewID : undefined}
        />
        <TouchableOpacity
          style={[styles.compactSendButton, isSendDisabled && styles.compactSendButtonDisabled]}
          onPress={handleSend}
          disabled={isSendDisabled}
        >
          <Text style={styles.compactSendButtonText}>
            {sending ? '••' : '→'}
          </Text>
        </TouchableOpacity>
      </View>

      {/* Priority selector */}
      <View style={styles.compactPriorityRow}>
        <Text style={styles.compactPriorityLabel}>Priority:</Text>
        <View style={styles.compactPriorityButtons}>
          {priorityOptions.map((option) => (
            <TouchableOpacity
              key={option.value}
              style={[
                styles.compactPriorityButton,
                priority === option.value && styles.compactPriorityButtonActive,
                !isStarted && styles.compactPriorityButtonDisabled
              ]}
              onPress={() => setPriority(option.value)}
              disabled={!isStarted}
            >
              <Text style={[
                styles.compactPriorityButtonText,
                priority === option.value && styles.compactPriorityButtonTextActive
              ]}>
                {option.label.charAt(0)}
              </Text>
            </TouchableOpacity>
          ))}
        </View>
      </View>
    </View>
  );

  const renderStatusCard = () => (
    <View style={[styles.headerCard, isCompact && styles.headerCardCompact]}>
      <View style={styles.headerRow}>
        <View style={styles.headerCopy}>
          <Text style={styles.screenTitle}>Offline Messenger</Text>
          <Text style={styles.screenSubtitle}>
            Exchange time-sensitive updates directly between nearby devices — no network required.
          </Text>
        </View>
        <View
          style={[
            styles.statusBadge,
            isStarted ? styles.statusBadgeActive : styles.statusBadgeIdle,
          ]}
        >
          <View
            style={[
              styles.statusDot,
              isStarted ? styles.statusDotActive : styles.statusDotIdle,
            ]}
          />
          <Text
            style={[
              styles.statusText,
              isStarted ? styles.statusTextActive : styles.statusTextIdle,
            ]}
          >
            {isStarted ? 'Running' : 'Stopped'}
          </Text>
        </View>
      </View>

      <View style={styles.metricsRow}>
        <View style={styles.metricBlock}>
          <Text style={styles.metricValue}>{discoveredPeers.length}</Text>
          <Text style={styles.metricLabel}>Nearby peers</Text>
        </View>
        <View style={styles.metricDivider} />
        <View style={styles.metricBlock}>
          <Text style={styles.metricValue}>{priorityLabel}</Text>
          <Text style={styles.metricLabel}>Send priority</Text>
        </View>
        <View style={styles.metricDivider} />
        <View style={styles.metricBlock}>
          <Text style={styles.metricValue}>{activeTransfers.length}</Text>
          <Text style={styles.metricLabel}>Active transfers</Text>
        </View>
      </View>

      <View>
        <View style={styles.sectionHeaderRow}>
          <Text style={styles.sectionTitle}>Nearby peers</Text>
          <Text style={styles.sectionCaption}>Tap a peer to target your next message.</Text>
        </View>
        {isStarted ? (
          discoveredPeers.length > 0 ? (
            <ScrollView
              horizontal
              showsHorizontalScrollIndicator={false}
              style={styles.peerScroll}
              contentContainerStyle={styles.peerScrollContent}
            >
              {discoveredPeers.map((peerId) => {
                const isActive = recipient === peerId;
                return (
                  <TouchableOpacity
                    key={peerId}
                    style={[styles.peerChip, isActive && styles.peerChipActive]}
                    onPress={() => handleSelectPeer(peerId)}
                  >
                    <Text
                      style={[styles.peerChipText, isActive && styles.peerChipTextActive]}
                      numberOfLines={1}
                    >
                      {peerId}
                    </Text>
                  </TouchableOpacity>
                );
              })}
            </ScrollView>
          ) : (
            <View style={styles.peerEmpty}>
              <Text style={styles.peerEmptyTitle}>No peers detected yet</Text>
              <Text style={styles.peerEmptyBody}>
                Keep Bluetooth enabled and remain within range. Newly discovered peers will appear
                here automatically.
              </Text>
            </View>
          )
        ) : (
          <View style={styles.peerEmpty}>
            <Text style={styles.peerEmptyTitle}>Protocol stopped</Text>
            <Text style={styles.peerEmptyBody}>
              Start the protocol from the home screen to begin discovering nearby devices.
            </Text>
          </View>
        )}
      </View>

      {activeTransfers.length > 0 ? (
        <View style={styles.transferGroup}>
          <View style={styles.sectionHeaderRow}>
            <Text style={styles.sectionTitle}>File transfers</Text>
            <Text style={styles.sectionCaption}>
              Monitoring {activeTransfers.length} in-flight transfer
              {activeTransfers.length === 1 ? '' : 's'}.
            </Text>
          </View>
          {activeTransfers.map((transfer) => {
            const progress = Math.min(Math.max(transfer.percentage ?? 0, 0), 100);
            return (
              <View key={transfer.fileId} style={styles.transferItem}>
                <View style={styles.transferHeader}>
                  <Text style={styles.transferName} numberOfLines={1}>
                    {transfer.fileName || transfer.fileId}
                  </Text>
                  <Text style={styles.transferPercent}>{progress}%</Text>
                </View>
                <View style={styles.transferTrack}>
                  <View style={[styles.transferFill, { width: `${progress}%` }]} />
                </View>
                <Text style={styles.transferMeta}>
                  {transfer.direction === 'outbound' ? 'Sending' : 'Receiving'} ·{' '}
                  {transfer.direction === 'outbound'
                    ? `To ${transfer.recipient ?? 'peer'}`
                    : `From ${transfer.sender ?? 'peer'}`}
                </Text>
              </View>
            );
          })}
        </View>
      ) : null}
    </View>
  );

  const renderComposerCard = () => (
    <View style={[styles.composerCard, isCompact && styles.composerCardCompact]}>
      <View>
        <View style={styles.sectionHeaderRow}>
          <Text style={styles.sectionTitle}>Compose message</Text>
          <Text style={styles.sectionCaption}>{priorityLabel} priority</Text>
        </View>
        <Text style={styles.sectionIntro}>{priorityDescription}</Text>
      </View>

      {/* Debug info for troubleshooting */}
      {__DEV__ && (
        <View style={{ padding: 8, backgroundColor: '#f0f0f0', borderRadius: 8 }}>
          <Text style={{ fontSize: 10, color: '#666' }}>
            Debug: Started={isStarted ? 'Yes' : 'No'} | Sending={sending ? 'Yes' : 'No'} | Peers={discoveredPeers.length}
          </Text>
        </View>
      )}

      <View style={styles.inputBlock}>
        <Text style={styles.inputLabel}>Recipient</Text>
        <View style={styles.inputRow}>
          <TextInput
            style={[styles.recipientInput, !isStarted && styles.recipientInputDisabled]}
            value={recipient}
            onChangeText={setRecipient}
            placeholder={
              isStarted
                ? discoveredPeers.length > 0 
                ? 'Select a peer or enter their user ID'
                  : 'Enter recipient user ID (e.g., user_abc123)'
                : 'Start the protocol to pick a peer'
            }
            placeholderTextColor="#94a3b8"
            editable={isStarted}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="next"
            onSubmitEditing={() => messageInputRef.current?.focus()}
            blurOnSubmit={false}
            inputAccessoryViewID={isCompact ? inputAccessoryViewID : undefined}
          />
          <TouchableOpacity
            style={[styles.clearButton, !recipient && styles.clearButtonDisabled]}
            onPress={handleClearRecipient}
            disabled={!recipient}
          >
            <Text
              style={[styles.clearButtonText, !recipient && styles.clearButtonTextDisabled]}
            >
              Clear
            </Text>
          </TouchableOpacity>
        </View>
      </View>

      <View style={styles.inputBlock}>
        <Text style={styles.inputLabel}>Message</Text>
        <TextInput
          ref={messageInputRef}
          style={[styles.messageInput, !isStarted && styles.messageInputDisabled]}
          value={message}
          onChangeText={setMessage}
          placeholder={
            isStarted ? 'Share status, requests, or updates…' : 'Start the protocol to compose'
          }
          placeholderTextColor="#94a3b8"
          editable={isStarted}
          multiline
          returnKeyType="done"
          blurOnSubmit={true}
          onSubmitEditing={handleSend}
          textAlignVertical="top"
          inputAccessoryViewID={isCompact ? inputAccessoryViewID : undefined}
        />
      </View>

      <View style={styles.inputBlock}>
        <Text style={styles.inputLabel}>Quick templates</Text>
        <View style={styles.templateRow}>
        {quickTemplates.map((template) => (
          <TouchableOpacity
            key={template}
              style={[styles.templateChip, !isStarted && styles.templateChipDisabled]}
              onPress={() => handleInsertTemplate(template)}
            disabled={!isStarted}
          >
              <Text
                style={[styles.templateText, !isStarted && styles.templateTextDisabled]}
                numberOfLines={1}
              >
              {template}
            </Text>
          </TouchableOpacity>
        ))}
        </View>
      </View>

      <View style={styles.priorityGroup}>
        <Text style={styles.inputLabel}>Priority</Text>
        <View style={styles.priorityRow}>
          {priorityOptions.map((option) => {
            const isActive = option.value === priority;
            return (
            <TouchableOpacity
              key={option.value}
              style={[
                styles.priorityChip,
                  isActive && styles.priorityChipActive,
                  !isStarted && styles.priorityChipDisabled,
              ]}
              onPress={() => setPriority(option.value)}
              disabled={!isStarted}
            >
              <Text
                style={[
                  styles.priorityChipText,
                    isActive && styles.priorityChipTextActive,
                    !isStarted && styles.priorityChipTextDisabled,
                ]}
              >
                {option.label}
              </Text>
                <Text
                  style={[
                    styles.priorityChipHelper,
                    isActive && styles.priorityChipHelperActive,
                    !isStarted && styles.priorityChipHelperDisabled,
                  ]}
                  numberOfLines={1}
                >
                  {option.helper}
                </Text>
            </TouchableOpacity>
            );
          })}
        </View>
        </View>

      <View style={styles.composerFooter}>
        <Text style={styles.priorityHint}>{sendHint}</Text>
        <TouchableOpacity
          style={[styles.sendButton, isSendDisabled && styles.sendButtonDisabled]}
          onPress={handleSend}
          disabled={isSendDisabled}
        >
          <Text style={styles.sendButtonText}>{sending ? 'Sending…' : 'Send'}</Text>
        </TouchableOpacity>
      </View>

      {!isStarted ? (
        <View style={styles.helperBanner}>
        <Text style={styles.helperText}>
            Start the protocol to enable peer discovery, message composition, and file transfers.
        </Text>
        </View>
      ) : null}
    </View>
  );

  const mobileHeaderComponent = (
    <View style={styles.mobileHeaderSection}>{renderStatusCard()}</View>
  );

  const mobileFooterComponent = (
    <View
      style={[
        styles.mobileFooterSection,
        { paddingBottom: Math.max(insets.bottom, 24) },
      ]}
    >
      {renderComposerCard()}
    </View>
  );

  return (
    <>
      {isCompact && renderInputAccessory()}
      <KeyboardAvoidingView
        style={styles.keyboard}
        behavior={Platform.OS === 'ios' ? 'padding' : 'height'}
        keyboardVerticalOffset={keyboardVerticalOffset}
      >
        <SafeAreaView style={styles.safeArea}>
          {isCompact ? (
            <View style={styles.conversationListContainer}>
              {/* Header */}
              <View style={styles.conversationHeader}>
                <Text style={styles.conversationTitle}>Messages</Text>
                <View style={styles.headerStats}>
                  <View style={[styles.statusIndicator, isStarted ? styles.statusOnline : styles.statusOffline]} />
                  <Text style={styles.headerStatsText}>
                    {isStarted ? `${discoveredPeers.length} online` : 'Offline'}
                  </Text>
                </View>
              </View>

              {/* Conversations List */}
              <View style={styles.conversationsList}>
                {peerConversations.length > 0 ? (
                  <FlatList
                    data={peerConversations}
                    keyExtractor={(item) => item.peerId}
                    renderItem={({ item }) => (
                      <TouchableOpacity
                        style={styles.conversationItem}
                        onPress={() => onOpenChat?.(item.peerId)}
                        activeOpacity={0.7}
                      >
                        <View style={styles.conversationLeft}>
                          <View style={styles.avatarContainer}>
                            <Text style={styles.avatarText}>
                              {item.peerId.slice(-2).toUpperCase()}
                            </Text>
                            {item.isOnline && <View style={styles.onlineBadge} />}
                          </View>
                        </View>
                        
                        <View style={styles.conversationCenter}>
                          <View style={styles.conversationHeader}>
                            <Text style={styles.peerDisplayName} numberOfLines={1}>
                              {item.peerId.slice(-8)}
                            </Text>
                            {item.lastMessageTime && (
                              <Text style={styles.conversationTime}>
                                {formatConversationTime(item.lastMessageTime)}
                              </Text>
                            )}
                          </View>
                          <Text style={styles.lastMessage} numberOfLines={2}>
                            {item.lastMessage || 'Tap to start messaging'}
                          </Text>
                        </View>

                        <View style={styles.conversationRight}>
                          <View style={styles.chevron}>
                            <Text style={styles.chevronText}>›</Text>
                          </View>
                        </View>
                      </TouchableOpacity>
                    )}
                    showsVerticalScrollIndicator={false}
                    contentContainerStyle={styles.conversationsListContent}
                  />
                ) : (
                  <View style={styles.emptyConversations}>
                    <Text style={styles.emptyTitle}>No conversations yet</Text>
                    <Text style={styles.emptySubtitle}>
                      {isStarted 
                        ? 'Discovered peers will appear here. Start a conversation by tapping on a peer.'
                        : 'Start the protocol to discover nearby peers and begin messaging.'
                      }
                    </Text>
                  </View>
                )}
              </View>

              {/* Quick compose button */}
              {isStarted && discoveredPeers.length > 0 && (
                <TouchableOpacity
                  style={styles.quickComposeButton}
                  onPress={() => {
                    // Open chat with first available peer
                    if (discoveredPeers.length > 0) {
                      onOpenChat?.(discoveredPeers[0]);
                    }
                  }}
                >
                  <Text style={styles.quickComposeButtonText}>+</Text>
                </TouchableOpacity>
              )}
            </View>
          ) : (
            <View style={[styles.container, isCompact && styles.containerCompact]}>
              {renderStatusCard()}
              <View style={styles.body}>
                <View style={styles.timelinePane}>
                  <MessageList
                    events={events}
                    currentUserId={currentUserId}
                    contentInsetBottom={72}
                  />
                </View>
                <View style={styles.composerPane}>{renderComposerCard()}</View>
              </View>
            </View>
          )}
        </SafeAreaView>
      </KeyboardAvoidingView>
    </>
  );
}

const styles = StyleSheet.create({
  keyboard: {
    flex: 1,
    backgroundColor: '#eaf1ff',
  },
  safeArea: {
    flex: 1,
  },
  container: {
    flex: 1,
    backgroundColor: '#eaf1ff',
    paddingHorizontal: 20,
    paddingVertical: 16,
    gap: 16,
  },
  containerCompact: {
    paddingHorizontal: 12,
    paddingVertical: 12,
    gap: 12,
  },
  headerCard: {
    backgroundColor: '#ffffff',
    borderRadius: 24,
    padding: 20,
    gap: 18,
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: 8 },
    shadowOpacity: 0.08,
    shadowRadius: 18,
    elevation: 5,
  },
  headerCardCompact: {
    borderRadius: 18,
    padding: 14,
    gap: 12,
  },
  headerRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 16,
  },
  headerCopy: {
    flex: 1,
  },
  screenTitle: {
    fontSize: 22,
    fontWeight: '800',
    color: '#0f172a',
  },
  screenSubtitle: {
    marginTop: 6,
    fontSize: 13,
    lineHeight: 20,
    color: '#475569',
  },
  statusBadge: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 999,
  },
  statusBadgeActive: {
    backgroundColor: 'rgba(22,163,74,0.16)',
  },
  statusBadgeIdle: {
    backgroundColor: 'rgba(148,163,184,0.28)',
  },
  statusDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    marginRight: 6,
  },
  statusDotActive: {
    backgroundColor: '#16a34a',
  },
  statusDotIdle: {
    backgroundColor: '#94a3b8',
  },
  statusText: {
    fontSize: 11,
    fontWeight: '700',
    letterSpacing: 0.6,
  },
  statusTextActive: {
    color: '#047857',
  },
  statusTextIdle: {
    color: '#475569',
  },
  metricsRow: {
    flexDirection: 'row',
    alignItems: 'stretch',
    borderRadius: 18,
    borderWidth: 1,
    borderColor: '#e2e8f0',
    backgroundColor: '#f8fafc',
    overflow: 'hidden',
  },
  metricBlock: {
    flex: 1,
    paddingVertical: 12,
    paddingHorizontal: 16,
    gap: 4,
  },
  metricValue: {
    fontSize: 18,
    fontWeight: '700',
    color: '#0f172a',
  },
  metricLabel: {
    fontSize: 12,
    color: '#64748b',
  },
  metricDivider: {
    width: StyleSheet.hairlineWidth,
    backgroundColor: '#e2e8f0',
  },
  sectionHeaderRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  sectionTitle: {
    fontSize: 14,
    fontWeight: '700',
    color: '#1f2937',
  },
  sectionCaption: {
    fontSize: 12,
    color: '#64748b',
  },
  sectionIntro: {
    fontSize: 12,
    color: '#64748b',
    marginTop: 4,
  },
  peerScroll: {
    marginTop: 12,
  },
  peerScrollContent: {
    gap: 10,
    paddingRight: 6,
  },
  peerChip: {
    borderRadius: 18,
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderWidth: 1,
    borderColor: '#cbd5f5',
    backgroundColor: '#e0e7ff',
  },
  peerChipActive: {
    borderColor: '#1d4ed8',
    backgroundColor: '#bfdbfe',
  },
  peerChipText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#1e3a8a',
  },
  peerChipTextActive: {
    color: '#1d4ed8',
  },
  peerEmpty: {
    borderRadius: 16,
    borderWidth: 1,
    borderColor: '#e2e8f0',
    backgroundColor: '#f8fafc',
    padding: 16,
    marginTop: 12,
  },
  peerEmptyTitle: {
    fontSize: 13,
    fontWeight: '600',
    color: '#334155',
    marginBottom: 4,
  },
  peerEmptyBody: {
    fontSize: 12,
    color: '#64748b',
    lineHeight: 18,
  },
  transferGroup: {
    gap: 12,
    marginTop: 12,
  },
  transferItem: {
    borderRadius: 16,
    borderWidth: 1,
    borderColor: '#e0e7ff',
    backgroundColor: '#f1f5ff',
    padding: 14,
    gap: 10,
  },
  transferHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
  },
  transferName: {
    flex: 1,
    fontSize: 13,
    fontWeight: '600',
    color: '#1e3a8a',
  },
  transferPercent: {
    fontSize: 12,
    fontWeight: '700',
    color: '#1d4ed8',
  },
  transferTrack: {
    height: 6,
    borderRadius: 999,
    backgroundColor: 'rgba(29,78,216,0.15)',
    overflow: 'hidden',
  },
  transferFill: {
    height: 6,
    borderRadius: 999,
    backgroundColor: '#2563eb',
  },
  transferMeta: {
    fontSize: 11,
    color: '#475569',
  },
  mobileHeaderSection: {
    backgroundColor: '#eaf1ff',
    paddingHorizontal: 12,
    paddingTop: 12,
    paddingBottom: 6,
  },
  mobileComposerSection: {
    backgroundColor: '#eaf1ff',
    paddingHorizontal: 12,
    paddingTop: 6,
    paddingBottom: 12,
  },
  messageHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 12,
    paddingBottom: 8,
    borderBottomWidth: 1,
    borderBottomColor: '#e2e8f0',
  },
  counterPill: {
    minWidth: 32,
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderRadius: 999,
    backgroundColor: '#e2e8f0',
    alignItems: 'center',
  },
  counterText: {
    fontSize: 12,
    fontWeight: '700',
    color: '#475569',
  },
  embeddedMessageList: {
    flex: 1,
    backgroundColor: 'transparent',
    shadowOpacity: 0,
    elevation: 0,
  },
  body: {
    flex: 1,
    flexDirection: 'row',
    gap: 16,
  },
  timelinePane: {
    flex: 1,
  },
  composerPane: {
    width: 360,
    flex: 0,
  },
  composerCard: {
    backgroundColor: '#ffffff',
    borderRadius: 22,
    padding: 18,
    gap: 16,
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: 6 },
    shadowOpacity: 0.08,
    shadowRadius: 16,
    elevation: 4,
  },
  composerCardCompact: {
    borderRadius: 20,
    padding: 16,
  },
  inputBlock: {
    gap: 8,
  },
  inputLabel: {
    fontSize: 13,
    fontWeight: '700',
    color: '#334155',
  },
  inputRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  recipientInput: {
    flex: 1,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: '#d0d7e6',
    backgroundColor: '#f8fafc',
    paddingHorizontal: 14,
    paddingVertical: 10,
    fontSize: 14,
    color: '#0f172a',
  },
  recipientInputDisabled: {
    color: '#94a3b8',
  },
  clearButton: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 14,
    backgroundColor: '#e2e8f0',
  },
  clearButtonDisabled: {
    backgroundColor: '#f1f5f9',
  },
  clearButtonText: {
    fontSize: 11,
    fontWeight: '700',
    color: '#334155',
    letterSpacing: 0.4,
  },
  clearButtonTextDisabled: {
    color: '#94a3b8',
  },
  messageInput: {
    minHeight: 100,
    maxHeight: 150,
    borderRadius: 20,
    borderWidth: 1,
    borderColor: '#d0d7e6',
    backgroundColor: '#f8fafc',
    paddingHorizontal: 16,
    paddingVertical: 14,
    fontSize: 16,
    lineHeight: 22,
    color: '#0f172a',
    textAlignVertical: 'top',
  },
  messageInputDisabled: {
    color: '#94a3b8',
  },
  templateRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
  },
  templateChip: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 16,
    backgroundColor: '#e0f2fe',
  },
  templateChipDisabled: {
    backgroundColor: '#f1f5f9',
  },
  templateText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#0369a1',
  },
  templateTextDisabled: {
    color: '#94a3b8',
  },
  priorityGroup: {
    gap: 8,
  },
  priorityRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
  },
  priorityChip: {
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 18,
    borderWidth: 1,
    borderColor: '#cbd5f5',
    backgroundColor: '#f1f5ff',
    minWidth: 96,
  },
  priorityChipActive: {
    borderColor: '#1d4ed8',
    backgroundColor: '#dbeafe',
  },
  priorityChipDisabled: {
    backgroundColor: '#f8fafc',
    borderColor: '#e2e8f0',
  },
  priorityChipText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#1e3a8a',
  },
  priorityChipTextActive: {
    color: '#1d4ed8',
  },
  priorityChipTextDisabled: {
    color: '#94a3b8',
  },
  priorityChipHelper: {
    fontSize: 10,
    marginTop: 4,
    color: '#475569',
  },
  priorityChipHelperActive: {
    color: '#1d4ed8',
  },
  priorityChipHelperDisabled: {
    color: '#94a3b8',
  },
  composerFooter: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
  },
  priorityHint: {
    flex: 1,
    fontSize: 12,
    color: '#64748b',
  },
  sendButton: {
    paddingHorizontal: 22,
    paddingVertical: 12,
    borderRadius: 20,
    backgroundColor: '#2563eb',
    shadowColor: '#1e3a8a',
    shadowOffset: { width: 0, height: 6 },
    shadowOpacity: 0.22,
    shadowRadius: 12,
    elevation: 5,
  },
  sendButtonDisabled: {
    backgroundColor: '#94a3b8',
    shadowOpacity: 0,
    elevation: 0,
  },
  sendButtonText: {
    fontSize: 16,
    fontWeight: '700',
    color: '#ffffff',
  },
  helperBanner: {
    borderRadius: 16,
    padding: 12,
    backgroundColor: '#fee2e2',
    borderWidth: 1,
    borderColor: '#fecaca',
  },
  helperText: {
    fontSize: 12,
    color: '#b91c1c',
    textAlign: 'center',
  },
  inputAccessory: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    alignItems: 'center',
    backgroundColor: '#f8fafc',
    borderTopWidth: 1,
    borderTopColor: '#e2e8f0',
    paddingHorizontal: 16,
    paddingVertical: 8,
    gap: 12,
  },
  accessoryButton: {
    paddingHorizontal: 16,
    paddingVertical: 8,
    borderRadius: 16,
    backgroundColor: '#e2e8f0',
  },
  accessoryButtonPrimary: {
    backgroundColor: '#2563eb',
  },
  accessoryButtonText: {
    fontSize: 14,
    fontWeight: '600',
    color: '#475569',
  },
  accessoryButtonTextPrimary: {
    color: '#ffffff',
  },
  // Mobile-friendly styles
  mobileContainer: {
    flex: 1,
    backgroundColor: '#f8fafc',
  },
  compactHeader: {
    backgroundColor: '#ffffff',
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: '#e2e8f0',
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.05,
    shadowRadius: 4,
    elevation: 2,
  },
  compactHeaderVerySmall: {
    paddingHorizontal: 12,
    paddingVertical: 10,
  },
  compactHeaderRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  compactHeaderLeft: {
    flex: 1,
  },
  compactHeaderRight: {
    alignItems: 'flex-end',
  },
  compactTitle: {
    fontSize: 18,
    fontWeight: '700',
    color: '#1f2937',
    marginBottom: 4,
  },
  compactStatus: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  compactStatusDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
  compactStatusText: {
    fontSize: 12,
    fontWeight: '500',
    color: '#6b7280',
  },
  compactMetric: {
    alignItems: 'center',
  },
  compactMetricValue: {
    fontSize: 16,
    fontWeight: '700',
    color: '#1f2937',
  },
  compactMetricLabel: {
    fontSize: 10,
    color: '#6b7280',
    marginTop: 2,
  },
  peerQuickSelect: {
    marginTop: 12,
    paddingTop: 12,
    borderTopWidth: 1,
    borderTopColor: '#f1f5f9',
  },
  peerQuickLabel: {
    fontSize: 12,
    fontWeight: '500',
    color: '#6b7280',
    marginBottom: 8,
  },
  peerQuickScroll: {
    flexGrow: 0,
  },
  peerQuickContent: {
    gap: 8,
  },
  peerQuickChip: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 16,
    backgroundColor: '#f1f5f9',
    borderWidth: 1,
    borderColor: '#e2e8f0',
  },
  peerQuickChipActive: {
    backgroundColor: '#dbeafe',
    borderColor: '#3b82f6',
  },
  peerQuickChipText: {
    fontSize: 12,
    fontWeight: '500',
    color: '#6b7280',
  },
  peerQuickChipTextActive: {
    color: '#2563eb',
    fontWeight: '600',
  },
  messageArea: {
    flex: 1,
    backgroundColor: '#ffffff',
    marginHorizontal: 8,
    marginVertical: 8,
    borderRadius: 16,
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.05,
    shadowRadius: 8,
    elevation: 2,
  },
  compactComposer: {
    backgroundColor: '#ffffff',
    borderTopWidth: 1,
    borderTopColor: '#e2e8f0',
    paddingHorizontal: 16,
    paddingVertical: 12,
    shadowColor: '#0f172a',
    shadowOffset: { width: 0, height: -2 },
    shadowOpacity: 0.05,
    shadowRadius: 8,
    elevation: 3,
  },
  compactComposerVerySmall: {
    paddingHorizontal: 12,
    paddingVertical: 10,
  },
  compactComposerCard: {
    gap: 12,
  },
  compactInputRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  compactInputLabel: {
    fontSize: 14,
    fontWeight: '500',
    color: '#374151',
    minWidth: 24,
  },
  compactRecipientInput: {
    flex: 1,
    borderWidth: 1,
    borderColor: '#d1d5db',
    borderRadius: 12,
    paddingHorizontal: 12,
    paddingVertical: 8,
    fontSize: 14,
    color: '#1f2937',
    backgroundColor: '#f9fafb',
  },
  compactInputDisabled: {
    backgroundColor: '#f3f4f6',
    color: '#9ca3af',
  },
  compactClearButton: {
    width: 24,
    height: 24,
    borderRadius: 12,
    backgroundColor: '#ef4444',
    alignItems: 'center',
    justifyContent: 'center',
  },
  compactClearText: {
    fontSize: 16,
    fontWeight: '600',
    color: '#ffffff',
  },
  compactMessageRow: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
  },
  compactMessageInput: {
    flex: 1,
    borderWidth: 1,
    borderColor: '#d1d5db',
    borderRadius: 16,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 15,
    color: '#1f2937',
    backgroundColor: '#f9fafb',
    maxHeight: 80,
    textAlignVertical: 'top',
  },
  compactSendButton: {
    width: 36,
    height: 36,
    borderRadius: 18,
    backgroundColor: '#3b82f6',
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: '#3b82f6',
    shadowOffset: { width: 0, height: 2 },
    shadowOpacity: 0.25,
    shadowRadius: 4,
    elevation: 2,
  },
  compactSendButtonDisabled: {
    backgroundColor: '#d1d5db',
    shadowOpacity: 0,
    elevation: 0,
  },
  compactSendButtonText: {
    fontSize: 18,
    fontWeight: '600',
    color: '#ffffff',
  },
  compactPriorityRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  compactPriorityLabel: {
    fontSize: 12,
    fontWeight: '500',
    color: '#6b7280',
    minWidth: 50,
  },
  compactPriorityButtons: {
    flexDirection: 'row',
    gap: 6,
  },
  compactPriorityButton: {
    width: 28,
    height: 28,
    borderRadius: 14,
    backgroundColor: '#f3f4f6',
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: '#e5e7eb',
  },
  compactPriorityButtonActive: {
    backgroundColor: '#dbeafe',
    borderColor: '#3b82f6',
  },
  compactPriorityButtonDisabled: {
    backgroundColor: '#f9fafb',
    borderColor: '#f3f4f6',
  },
  compactPriorityButtonText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#6b7280',
  },
  compactPriorityButtonTextActive: {
    color: '#2563eb',
  },
  // Conversation list styles
  conversationListContainer: {
    flex: 1,
    backgroundColor: '#ffffff',
  },
  conversationHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 20,
    paddingVertical: 16,
    borderBottomWidth: 1,
    borderBottomColor: '#f0f0f0',
    backgroundColor: '#ffffff',
  },
  conversationTitle: {
    fontSize: 24,
    fontWeight: '700',
    color: '#1f2937',
  },
  headerStats: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  statusIndicator: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  statusOnline: {
    backgroundColor: '#10b981',
  },
  statusOffline: {
    backgroundColor: '#ef4444',
  },
  headerStatsText: {
    fontSize: 14,
    fontWeight: '500',
    color: '#6b7280',
  },
  conversationsList: {
    flex: 1,
  },
  conversationsListContent: {
    paddingBottom: 100,
  },
  conversationItem: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 20,
    paddingVertical: 16,
    borderBottomWidth: 1,
    borderBottomColor: '#f9fafb',
    backgroundColor: '#ffffff',
  },
  conversationLeft: {
    marginRight: 12,
  },
  avatarContainer: {
    position: 'relative',
  },
  avatarText: {
    width: 50,
    height: 50,
    borderRadius: 25,
    backgroundColor: '#e0e7ff',
    color: '#3730a3',
    fontSize: 16,
    fontWeight: '600',
    textAlign: 'center',
    lineHeight: 50,
    overflow: 'hidden',
  },
  onlineBadge: {
    position: 'absolute',
    bottom: 2,
    right: 2,
    width: 14,
    height: 14,
    borderRadius: 7,
    backgroundColor: '#10b981',
    borderWidth: 2,
    borderColor: '#ffffff',
  },
  conversationCenter: {
    flex: 1,
    gap: 4,
  },
  conversationHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  peerDisplayName: {
    flex: 1,
    fontSize: 16,
    fontWeight: '600',
    color: '#1f2937',
  },
  conversationTime: {
    fontSize: 12,
    color: '#9ca3af',
    marginLeft: 8,
  },
  lastMessage: {
    fontSize: 14,
    color: '#6b7280',
    lineHeight: 18,
  },
  conversationRight: {
    marginLeft: 12,
  },
  chevron: {
    width: 24,
    height: 24,
    alignItems: 'center',
    justifyContent: 'center',
  },
  chevronText: {
    fontSize: 18,
    color: '#d1d5db',
    fontWeight: '300',
  },
  emptyConversations: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 32,
  },
  emptyTitle: {
    fontSize: 20,
    fontWeight: '600',
    color: '#374151',
    marginBottom: 12,
    textAlign: 'center',
  },
  emptySubtitle: {
    fontSize: 16,
    color: '#6b7280',
    textAlign: 'center',
    lineHeight: 24,
  },
  quickComposeButton: {
    position: 'absolute',
    bottom: 30,
    right: 20,
    width: 56,
    height: 56,
    borderRadius: 28,
    backgroundColor: '#075e54',
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: '#075e54',
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.3,
    shadowRadius: 8,
    elevation: 8,
  },
  quickComposeButtonText: {
    fontSize: 24,
    fontWeight: '300',
    color: '#ffffff',
  },
});

