import React, { useState, useEffect, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  ScrollView,
  TouchableOpacity,
  RefreshControl,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Icon } from '../components/Icon';
import { useTheme } from '../hooks/useTheme';
import { useProtocol } from '../hooks/useProtocol';

interface DiagnosticItemProps {
  label: string;
  value: string | boolean | number;
  status: 'success' | 'warning' | 'error' | 'neutral';
  description?: string;
}

function DiagnosticItem({ label, value, status, description }: DiagnosticItemProps) {
  const { theme } = useTheme();

  const statusColor = {
    success: theme.colors.success,
    warning: theme.colors.warning,
    error: theme.colors.error,
    neutral: theme.colors.textSecondary,
  }[status];

  const statusIcon = {
    success: 'checkmark-circle',
    warning: 'warning',
    error: 'close-circle',
    neutral: 'information-circle',
  }[status];

  const displayValue = typeof value === 'boolean' ? (value ? 'Yes' : 'No') : String(value);

  return (
    <View style={[styles.diagnosticItem, { borderBottomColor: theme.colors.border }]}>
      <View style={styles.diagnosticHeader}>
        <Icon name={statusIcon} size={20} color={statusColor} />
        <Text style={[styles.diagnosticLabel, { color: theme.colors.text }]}>{label}</Text>
      </View>
      <Text style={[styles.diagnosticValue, { color: statusColor }]}>{displayValue}</Text>
      {description && (
        <Text style={[styles.diagnosticDescription, { color: theme.colors.textSecondary }]}>
          {description}
        </Text>
      )}
    </View>
  );
}

interface MlsDiagnosticsScreenProps {
  onBack: () => void;
}

export function MlsDiagnosticsScreen({ onBack }: MlsDiagnosticsScreenProps) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const {
    protocol,
    isMlsInitialized,
    encryptedPeers,
    contacts,
    chats,
    isOnline,
  } = useProtocol();

  const [refreshing, setRefreshing] = useState(false);
  const [diagnostics, setDiagnostics] = useState<{
    keyPackageCount: number;
    sessionCount: number;
    groupCount: number;
    encryptedMessagesCount: number;
    totalMessagesCount: number;
  }>({
    keyPackageCount: 0,
    sessionCount: 0,
    groupCount: 0,
    encryptedMessagesCount: 0,
    totalMessagesCount: 0,
  });

  const refreshDiagnostics = useCallback(async () => {
    if (!protocol) return;

    try {
      // Count encrypted messages from chats
      let encryptedCount = 0;
      let totalCount = 0;
      chats.forEach(chat => {
        chat.messages.forEach(msg => {
          totalCount++;
          if (msg.isEncrypted) encryptedCount++;
        });
      });

      setDiagnostics({
        keyPackageCount: 0, // Would need MLS API to get this
        sessionCount: encryptedPeers.size,
        groupCount: 0, // Would need MLS API to get this
        encryptedMessagesCount: encryptedCount,
        totalMessagesCount: totalCount,
      });
    } catch (error) {
      console.error('[MlsDiagnostics] Failed to refresh:', error);
    }
  }, [protocol, chats, encryptedPeers]);

  useEffect(() => {
    refreshDiagnostics();
  }, [refreshDiagnostics]);

  const handleRefresh = async () => {
    setRefreshing(true);
    await refreshDiagnostics();
    setRefreshing(false);
  };

  const encryptionPercentage = diagnostics.totalMessagesCount > 0
    ? Math.round((diagnostics.encryptedMessagesCount / diagnostics.totalMessagesCount) * 100)
    : 0;

  const encryptedPeersList = Array.from(encryptedPeers);
  const unencryptedPeers = contacts.filter(c => !encryptedPeers.has(c.id));

  return (
    <View style={[styles.container, { backgroundColor: theme.colors.background }]}>
      {/* Header */}
      <View style={[styles.header, { backgroundColor: theme.colors.surface, paddingTop: insets.top }]}>
        <TouchableOpacity style={styles.backButton} onPress={onBack}>
          <Icon name="arrow-back" size={24} color={theme.colors.primary} />
        </TouchableOpacity>
        <Text style={[styles.headerTitle, { color: theme.colors.text }]}>
          MLS Encryption Diagnostics
        </Text>
        <View style={{ width: 40 }} />
      </View>

      <ScrollView
        style={styles.content}
        refreshControl={
          <RefreshControl refreshing={refreshing} onRefresh={handleRefresh} />
        }
      >
        {/* Overall Status */}
        <View style={[styles.section, { backgroundColor: theme.colors.surface }]}>
          <Text style={[styles.sectionTitle, { color: theme.colors.text }]}>
            Encryption Status
          </Text>

          <DiagnosticItem
            label="MLS Initialized"
            value={isMlsInitialized}
            status={isMlsInitialized ? 'success' : 'error'}
            description={isMlsInitialized
              ? 'MLS encryption is active and ready'
              : 'MLS failed to initialize - messages will be sent unencrypted'
            }
          />

          <DiagnosticItem
            label="Protocol Online"
            value={isOnline}
            status={isOnline ? 'success' : 'warning'}
            description={isOnline
              ? 'Transport layer is active'
              : 'Protocol is offline - start the messenger to enable encryption'
            }
          />

          <DiagnosticItem
            label="Encrypted Sessions"
            value={diagnostics.sessionCount}
            status={diagnostics.sessionCount > 0 ? 'success' : 'neutral'}
            description={`Active MLS sessions with ${diagnostics.sessionCount} peer(s)`}
          />
        </View>

        {/* Message Encryption Stats */}
        <View style={[styles.section, { backgroundColor: theme.colors.surface }]}>
          <Text style={[styles.sectionTitle, { color: theme.colors.text }]}>
            Message Encryption
          </Text>

          <DiagnosticItem
            label="Encrypted Messages"
            value={`${diagnostics.encryptedMessagesCount} / ${diagnostics.totalMessagesCount}`}
            status={encryptionPercentage === 100 ? 'success' :
                   encryptionPercentage >= 50 ? 'warning' : 'error'}
            description={`${encryptionPercentage}% of messages are end-to-end encrypted`}
          />

          {/* Encryption Progress Bar */}
          <View style={styles.progressContainer}>
            <View style={[styles.progressBar, { backgroundColor: theme.colors.border }]}>
              <View
                style={[
                  styles.progressFill,
                  {
                    width: `${encryptionPercentage}%`,
                    backgroundColor: encryptionPercentage === 100
                      ? theme.colors.success
                      : encryptionPercentage >= 50
                        ? theme.colors.warning
                        : theme.colors.error
                  }
                ]}
              />
            </View>
            <Text style={[styles.progressText, { color: theme.colors.textSecondary }]}>
              {encryptionPercentage}% encrypted
            </Text>
          </View>
        </View>

        {/* Encrypted Peers */}
        <View style={[styles.section, { backgroundColor: theme.colors.surface }]}>
          <Text style={[styles.sectionTitle, { color: theme.colors.text }]}>
            Encrypted Peers ({encryptedPeersList.length})
          </Text>

          {encryptedPeersList.length === 0 ? (
            <Text style={[styles.emptyText, { color: theme.colors.textSecondary }]}>
              No encrypted sessions established yet.
              Start a conversation to establish encryption.
            </Text>
          ) : (
            encryptedPeersList.map(peerId => {
              const contact = contacts.find(c => c.id === peerId);
              return (
                <View key={peerId} style={[styles.peerItem, { borderBottomColor: theme.colors.border }]}>
                  <Icon name="lock-closed" size={16} color={theme.colors.success} />
                  <View style={styles.peerInfo}>
                    <Text style={[styles.peerName, { color: theme.colors.text }]}>
                      {contact?.name || `User ${peerId.slice(-4)}`}
                    </Text>
                    <Text style={[styles.peerId, { color: theme.colors.textSecondary }]}>
                      {peerId.slice(0, 8)}...
                    </Text>
                  </View>
                  <Icon name="checkmark-circle" size={20} color={theme.colors.success} />
                </View>
              );
            })
          )}
        </View>

        {/* Unencrypted Peers */}
        {unencryptedPeers.length > 0 && (
          <View style={[styles.section, { backgroundColor: theme.colors.surface }]}>
            <Text style={[styles.sectionTitle, { color: theme.colors.text }]}>
              Pending Encryption ({unencryptedPeers.length})
            </Text>
            <Text style={[styles.sectionDescription, { color: theme.colors.textSecondary }]}>
              These contacts haven't exchanged key packages yet.
              Send a message to establish an encrypted session.
            </Text>

            {unencryptedPeers.map(contact => (
              <View key={contact.id} style={[styles.peerItem, { borderBottomColor: theme.colors.border }]}>
                <Icon name="lock-open" size={16} color={theme.colors.warning} />
                <View style={styles.peerInfo}>
                  <Text style={[styles.peerName, { color: theme.colors.text }]}>
                    {contact.name}
                  </Text>
                  <Text style={[styles.peerId, { color: theme.colors.textSecondary }]}>
                    {contact.isOnline ? 'Online - Ready for encryption' : 'Offline'}
                  </Text>
                </View>
                <Icon
                  name={contact.isOnline ? 'arrow-forward' : 'time'}
                  size={20}
                  color={theme.colors.textSecondary}
                />
              </View>
            ))}
          </View>
        )}

        {/* How It Works */}
        <View style={[styles.section, { backgroundColor: theme.colors.surface }]}>
          <Text style={[styles.sectionTitle, { color: theme.colors.text }]}>
            How MLS Encryption Works
          </Text>

          <View style={styles.infoItem}>
            <View style={[styles.infoNumber, { backgroundColor: theme.colors.primary }]}>
              <Text style={styles.infoNumberText}>1</Text>
            </View>
            <Text style={[styles.infoText, { color: theme.colors.textSecondary }]}>
              When you discover a peer, key packages are automatically exchanged
            </Text>
          </View>

          <View style={styles.infoItem}>
            <View style={[styles.infoNumber, { backgroundColor: theme.colors.primary }]}>
              <Text style={styles.infoNumberText}>2</Text>
            </View>
            <Text style={[styles.infoText, { color: theme.colors.textSecondary }]}>
              On first message, an MLS session is created with a Welcome message
            </Text>
          </View>

          <View style={styles.infoItem}>
            <View style={[styles.infoNumber, { backgroundColor: theme.colors.primary }]}>
              <Text style={styles.infoNumberText}>3</Text>
            </View>
            <Text style={[styles.infoText, { color: theme.colors.textSecondary }]}>
              All subsequent messages are encrypted with forward secrecy
            </Text>
          </View>

          <View style={styles.infoItem}>
            <View style={[styles.infoNumber, { backgroundColor: theme.colors.success }]}>
              <Icon name="lock-closed" size={14} color="white" />
            </View>
            <Text style={[styles.infoText, { color: theme.colors.textSecondary }]}>
              Look for the 🔒 icon on messages to confirm encryption
            </Text>
          </View>
        </View>

        <View style={{ height: insets.bottom + 20 }} />
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingBottom: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: 'rgba(0,0,0,0.1)',
  },
  backButton: {
    width: 40,
    height: 40,
    alignItems: 'center',
    justifyContent: 'center',
  },
  headerTitle: {
    flex: 1,
    fontSize: 17,
    fontWeight: '600',
    textAlign: 'center',
  },
  content: {
    flex: 1,
  },
  section: {
    margin: 16,
    marginBottom: 0,
    padding: 16,
    borderRadius: 12,
  },
  sectionTitle: {
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 12,
  },
  sectionDescription: {
    fontSize: 13,
    lineHeight: 18,
    marginBottom: 12,
  },
  diagnosticItem: {
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  diagnosticHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginBottom: 4,
  },
  diagnosticLabel: {
    fontSize: 14,
    fontWeight: '500',
  },
  diagnosticValue: {
    fontSize: 16,
    fontWeight: '600',
    marginLeft: 28,
  },
  diagnosticDescription: {
    fontSize: 12,
    marginLeft: 28,
    marginTop: 4,
  },
  progressContainer: {
    marginTop: 12,
  },
  progressBar: {
    height: 8,
    borderRadius: 4,
    overflow: 'hidden',
  },
  progressFill: {
    height: '100%',
    borderRadius: 4,
  },
  progressText: {
    fontSize: 12,
    marginTop: 6,
    textAlign: 'center',
  },
  peerItem: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    gap: 12,
  },
  peerInfo: {
    flex: 1,
  },
  peerName: {
    fontSize: 14,
    fontWeight: '500',
  },
  peerId: {
    fontSize: 11,
    marginTop: 2,
  },
  emptyText: {
    fontSize: 13,
    lineHeight: 18,
    textAlign: 'center',
    paddingVertical: 16,
  },
  infoItem: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    marginBottom: 12,
    gap: 12,
  },
  infoNumber: {
    width: 24,
    height: 24,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
  },
  infoNumberText: {
    color: 'white',
    fontSize: 12,
    fontWeight: '700',
  },
  infoText: {
    flex: 1,
    fontSize: 13,
    lineHeight: 18,
  },
});

