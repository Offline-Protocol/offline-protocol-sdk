import React, { useCallback, useEffect, useMemo, useState } from 'react';
import {
  View,
  Text,
  StyleSheet,
  ScrollView,
  TouchableOpacity,
  TextInput,
  ActivityIndicator,
} from 'react-native';
import type {
  SendFileParams,
  TransportType,
  InternetTransportConfig,
  WifiDirectTransportConfig,
} from '@offlineprotocol/react-native';
import type {
  DorsRuntimeConfig,
  FileTransferState,
  RelayPriorityInput,
} from '../types/runtime';
import { labelRelayPriority, mapRelayInputToNative } from '../types/runtime';

interface ControlCenterScreenProps {
  isStarted: boolean;
  activeTransports: TransportType[];
  forcedTransport: TransportType | null;
  relayPriority: 'low' | 'medium' | 'high';
  batteryLevel: number | null;
  dorsConfig: DorsRuntimeConfig;
  fileTransfers: FileTransferState[];
  onRefresh: () => Promise<void>;
  onEnableTransport: (
    type: TransportType,
    config?: InternetTransportConfig | WifiDirectTransportConfig
  ) => Promise<boolean>;
  onDisableTransport: (type: TransportType) => Promise<boolean>;
  onForceTransport: (type: TransportType) => Promise<boolean>;
  onReleaseTransport: () => Promise<void>;
  onSetBatteryLevel: (level: number) => Promise<boolean>;
  onSetRelayPriority: (priority: RelayPriorityInput) => Promise<boolean>;
  onUpdateDors: (partial: Partial<DorsRuntimeConfig>) => Promise<boolean>;
  onSendFile: (params: SendFileParams) => Promise<string | null>;
  onCancelFile: (fileId: string) => Promise<boolean>;
}

const TRANSPORT_LABELS: Record<TransportType, string> = {
  ble: 'Bluetooth LE',
  internet: 'Internet',
  wifiDirect: 'Wi-Fi Direct',
};

const AVAILABLE_RELAYS: RelayPriorityInput[] = ['auto', 'low', 'medium', 'high'];

const DORS_STEP_CONFIG = {
  hysteresis: 1,
  cooldown: 5,
  retryThreshold: 1,
  congestionDuration: 5,
  ttlHold: 5,
  historyWindow: 1,
  queueRatio: 0.05,
};

const DEFAULT_FILE_PAYLOAD: SendFileParams = {
  filePath: '',
  recipient: '',
  fileName: '',
};

export const ControlCenterScreen: React.FC<ControlCenterScreenProps> = ({
  isStarted,
  activeTransports,
  forcedTransport,
  relayPriority,
  batteryLevel,
  dorsConfig,
  fileTransfers,
  onRefresh,
  onEnableTransport,
  onDisableTransport,
  onForceTransport,
  onReleaseTransport,
  onSetBatteryLevel,
  onSetRelayPriority,
  onUpdateDors,
  onSendFile,
  onCancelFile,
}) => {
  const [refreshing, setRefreshing] = useState(false);
  const [batteryDraft, setBatteryDraft] = useState<number>(batteryLevel ?? 72);
  const [fileDraft, setFileDraft] = useState<SendFileParams>(DEFAULT_FILE_PAYLOAD);
  const [isSendingFile, setIsSendingFile] = useState(false);

  useEffect(() => {
    if (typeof batteryLevel === 'number') {
      setBatteryDraft(batteryLevel);
    }
  }, [batteryLevel]);

  const normalizedActive = useMemo(
    () => activeTransports.map((t) => t.toLowerCase()),
    [activeTransports]
  );

  const handleRefresh = useCallback(async () => {
    setRefreshing(true);
    try {
      await onRefresh();
    } finally {
      setRefreshing(false);
    }
  }, [onRefresh]);

  const handleBatteryChange = useCallback(
    async (nextLevel: number) => {
      setBatteryDraft(nextLevel);
      await onSetBatteryLevel(nextLevel);
    },
    [onSetBatteryLevel]
  );

  const handleRelayPriority = useCallback(
    async (priority: RelayPriorityInput) => {
      await onSetRelayPriority(priority);
    },
    [onSetRelayPriority]
  );

  const handleDorsStep = useCallback(
    async (
      key: keyof DorsRuntimeConfig,
      delta: number,
      options?: { min?: number; max?: number; precision?: number }
    ) => {
      if (!isStarted) {
        return;
      }
      const current = dorsConfig[key] as number | boolean;
      if (typeof current === 'number') {
        const precision = options?.precision ?? 0;
        const factor = Math.pow(10, precision);
        let next = ((current as number) + delta);
        next = Math.round(next * factor) / factor;
        if (options?.min !== undefined) {
          next = Math.max(options.min, next);
        }
        if (options?.max !== undefined) {
          next = Math.min(options.max, next);
        }
        if (options?.min === undefined && options?.max === undefined) {
          next = Math.max(0, next);
        }
        await onUpdateDors({ [key]: next } as Partial<DorsRuntimeConfig>);
      }
    },
    [dorsConfig, isStarted, onUpdateDors]
  );

  const handleTogglePreferOnline = useCallback(async () => {
    if (!isStarted) {
      return;
    }
    await onUpdateDors({ preferOnline: !dorsConfig.preferOnline });
  }, [dorsConfig.preferOnline, isStarted, onUpdateDors]);

  const submitFileTransfer = useCallback(async () => {
    if (!fileDraft.filePath.trim() || !fileDraft.recipient.trim()) {
      return;
    }
    setIsSendingFile(true);
    try {
      await onSendFile({
        ...fileDraft,
        filePath: fileDraft.filePath.trim(),
        recipient: fileDraft.recipient.trim(),
        fileName: fileDraft.fileName?.trim() || undefined,
      });
      setFileDraft(DEFAULT_FILE_PAYLOAD);
    } finally {
      setIsSendingFile(false);
    }
  }, [fileDraft, onSendFile]);

  return (
    <ScrollView
      style={styles.container}
      contentContainerStyle={styles.contentContainer}
      showsVerticalScrollIndicator={false}
    >
      <View style={styles.headerRow}>
        <Text style={styles.title}>Control Center</Text>
        <TouchableOpacity
          style={styles.refreshButton}
          onPress={handleRefresh}
          disabled={refreshing}
        >
          {refreshing ? (
            <ActivityIndicator color="#1d4ed8" size="small" />
          ) : (
            <Text style={styles.refreshText}>Refresh</Text>
          )}
        </TouchableOpacity>
      </View>

      {!isStarted ? (
        <View style={styles.warningCard}>
          <Text style={styles.warningTitle}>Protocol stopped</Text>
          <Text style={styles.warningText}>
            Start the protocol to enable transport management, relays, and file transfer controls.
          </Text>
        </View>
      ) : null}

      <View style={styles.section}>
        <SectionHeader title="Transports" subtitle="Manage available transport layers" />
        {(['ble', 'internet', 'wifiDirect'] as TransportType[]).map((transport) => {
          const isActive = normalizedActive.includes(transport.toLowerCase());
          const isForced =
            forcedTransport !== null &&
            forcedTransport.toLowerCase() === transport.toLowerCase();
          const isBLE = transport === 'ble';

          return (
            <View key={transport} style={styles.transportCard}>
              <View style={styles.transportHeader}>
                <Text style={styles.transportTitle}>{TRANSPORT_LABELS[transport]}</Text>
                <StatusPill label={isActive ? 'Active' : 'Inactive'} tone={isActive ? 'good' : 'neutral'} />
              </View>
              <Text style={styles.transportHint}>
                {isBLE
                  ? 'Bluetooth LE is managed automatically when the protocol is running.'
                  : 'Toggle this transport or force routing decisions manually.'}
              </Text>
              <View style={styles.transportControls}>
                <ControlButton
                  label={isActive ? 'Disable' : 'Enable'}
                  onPress={() =>
                    isActive
                      ? onDisableTransport(transport)
                      : onEnableTransport(transport)
                  }
                  disabled={isBLE || !isStarted}
                  variant={isActive ? 'danger' : 'primary'}
                />
                <ControlButton
                  label={isForced ? 'Release Lock' : 'Force'}
                  onPress={() =>
                    isForced ? onReleaseTransport() : onForceTransport(transport)
                  }
                  disabled={!isStarted || (isForced && forcedTransport === transport && !isActive)}
                  variant={isForced ? 'neutral' : 'secondary'}
                />
              </View>
            </View>
          );
        })}
      </View>

      <View style={styles.section}>
        <SectionHeader title="Relay & Battery" subtitle="Fine tune relay behaviour" />
        <View style={styles.card}>
          <Text style={styles.cardLabel}>Reported battery level</Text>
          <View style={styles.sliderRow}>
            {[20, 40, 60, 80, 100].map((mark) => (
              <TouchableOpacity
                key={mark}
                style={[
                  styles.batteryChip,
                  batteryDraft === mark && styles.batteryChipActive,
                ]}
                onPress={() => handleBatteryChange(mark)}
              >
                <Text
                  style={[
                    styles.batteryChipText,
                    batteryDraft === mark && styles.batteryChipTextActive,
                  ]}
                >
                  {mark}%
                </Text>
              </TouchableOpacity>
            ))}
          </View>

          <Text style={[styles.cardLabel, { marginTop: 16 }]}>Relay priority</Text>
          <View style={styles.priorityRow}>
            {AVAILABLE_RELAYS.map((option) => (
              <TouchableOpacity
                key={option}
                style={[
                  styles.priorityChip,
                  mapRelayInputToNative(option) === relayPriority && styles.priorityChipActive,
                ]}
                onPress={() => handleRelayPriority(option)}
              >
                <Text
                  style={[
                    styles.priorityChipText,
                    mapRelayInputToNative(option) === relayPriority && styles.priorityChipTextActive,
                  ]}
                >
                  {labelRelayPriority(option)}
                </Text>
              </TouchableOpacity>
            ))}
          </View>
        </View>
      </View>

      <View style={styles.section}>
        <SectionHeader title="Dynamic Routing (DORS)" subtitle="Adjust routing heuristics in real time" />
        <View style={styles.card}>
          <View style={styles.dorsRow}>
            <Text style={styles.cardLabel}>Prefer online routes</Text>
            <TouchableOpacity
              style={[
                styles.toggle,
                dorsConfig.preferOnline ? styles.toggleActive : styles.toggleInactive,
                !isStarted && styles.toggleDisabled,
              ]}
              onPress={handleTogglePreferOnline}
              disabled={!isStarted}
            >
              <Text
                style={[
                  styles.toggleText,
                  dorsConfig.preferOnline ? styles.toggleTextActive : styles.toggleTextInactive,
                  !isStarted && styles.toggleTextDisabled,
                ]}
              >
                {dorsConfig.preferOnline ? 'Enabled' : 'Disabled'}
              </Text>
            </TouchableOpacity>
          </View>

          <StepperRow
            label="Switch hysteresis"
            value={dorsConfig.switchHysteresis}
            onChange={(delta) =>
              handleDorsStep('switchHysteresis', delta * DORS_STEP_CONFIG.hysteresis, {
                min: 0,
                max: 100,
              })
            }
            suffix=" pts"
            disabled={!isStarted}
          />
          <StepperRow
            label="Switch cooldown"
            value={dorsConfig.switchCooldownSecs}
            onChange={(delta) =>
              handleDorsStep('switchCooldownSecs', delta * DORS_STEP_CONFIG.cooldown, {
                min: 0,
                max: 120,
              })
            }
            suffix=" s"
            disabled={!isStarted}
          />
          <StepperRow
            label="BLE ➜ Wi-Fi retries"
            value={dorsConfig.bleToWifiRetryThreshold}
            onChange={(delta) =>
              handleDorsStep('bleToWifiRetryThreshold', delta * DORS_STEP_CONFIG.retryThreshold, {
                min: 0,
                max: 5,
              })
            }
            disabled={!isStarted}
          />
          <StepperRow
            label="Congestion duration"
            value={dorsConfig.congestionDurationSecs}
            onChange={(delta) =>
              handleDorsStep('congestionDurationSecs', delta * DORS_STEP_CONFIG.congestionDuration, {
                min: 0,
                max: 120,
              })
            }
            suffix=" s"
            disabled={!isStarted}
          />
          <StepperRow
            label="TTL hold window"
            value={dorsConfig.ttlEscalationHoldSecs}
            onChange={(delta) =>
              handleDorsStep('ttlEscalationHoldSecs', delta * DORS_STEP_CONFIG.ttlHold, {
                min: 1,
                max: 180,
              })
            }
            suffix=" s"
            disabled={!isStarted}
          />
          <StepperRow
            label="History window"
            value={dorsConfig.historyWindowSize}
            onChange={(delta) =>
              handleDorsStep('historyWindowSize', delta * DORS_STEP_CONFIG.historyWindow, {
                min: 1,
                max: 100,
              })
            }
            suffix=" samples"
            disabled={!isStarted}
          />
          <StepperRow
            label="Queue recovery target"
            value={Math.round(dorsConfig.queueRecoveryRatio * 100)}
            onChange={(delta) =>
              handleDorsStep(
                'queueRecoveryRatio',
                delta * DORS_STEP_CONFIG.queueRatio,
                { min: 0, max: 1, precision: 2 }
              )
            }
            suffix="%"
            disabled={!isStarted}
          />
        </View>
      </View>

      <View style={styles.section}>
        <SectionHeader title="File Transfer" subtitle="Send and monitor file deliveries" />
        <View style={styles.card}>
          <Text style={styles.fileHint}>
            Provide a valid local path or URI. Transfers queue automatically when peers are in range.
          </Text>
          <View style={styles.inputGroup}>
            <Text style={styles.cardLabel}>Recipient User ID</Text>
            <TextInput
              style={styles.input}
              placeholder="user_123"
              value={fileDraft.recipient}
              onChangeText={(text) => setFileDraft((prev) => ({ ...prev, recipient: text }))}
              autoCapitalize="none"
            />
          </View>
          <View style={styles.inputGroup}>
            <Text style={styles.cardLabel}>File path or URI</Text>
            <TextInput
              style={styles.input}
              placeholder="/path/to/file.txt"
              value={fileDraft.filePath}
              onChangeText={(text) => setFileDraft((prev) => ({ ...prev, filePath: text }))}
            />
          </View>
          <View style={styles.inputGroup}>
            <Text style={styles.cardLabel}>Optional file name</Text>
            <TextInput
              style={styles.input}
              placeholder="file.txt"
              value={fileDraft.fileName ?? ''}
              onChangeText={(text) => setFileDraft((prev) => ({ ...prev, fileName: text }))}
            />
          </View>
          <TouchableOpacity
            style={[styles.submitButton, (!fileDraft.recipient || !fileDraft.filePath) && styles.submitButtonDisabled]}
            onPress={submitFileTransfer}
            disabled={!fileDraft.recipient || !fileDraft.filePath || isSendingFile || !isStarted}
          >
            {isSendingFile ? (
              <ActivityIndicator color="#fff" />
            ) : (
              <Text style={styles.submitButtonText}>Send File</Text>
            )}
          </TouchableOpacity>
        </View>

        {fileTransfers.length > 0 ? (
          <View style={[styles.card, { marginTop: 12 }]}>
            {fileTransfers.map((transfer) => (
              <View key={transfer.fileId} style={styles.transferRow}>
                <View style={styles.transferHeader}>
                  <Text style={styles.transferName} numberOfLines={1}>
                    {transfer.fileName || transfer.fileId}
                  </Text>
                  <StatusPill
                    label={transfer.status === 'pending' ? 'In flight' : transfer.status}
                    tone={transfer.status === 'completed' ? 'good' : transfer.status === 'cancelled' ? 'neutral' : 'info'}
                  />
                </View>
                <Text style={styles.transferMeta}>
                  {transfer.direction === 'outbound' ? `To ${transfer.recipient ?? 'peer'}` : `From ${transfer.sender ?? 'peer'}`}
                </Text>
                <View style={styles.progressBar}>
                  <View
                    style={[
                      styles.progressFill,
                      { width: `${Math.min(transfer.percentage, 100)}%` },
                    ]}
                  />
                </View>
                <View style={styles.transferFooter}>
                  <Text style={styles.transferProgress}>{transfer.percentage}%</Text>
                  {transfer.status === 'pending' ? (
                    <TouchableOpacity onPress={() => onCancelFile(transfer.fileId)}>
                      <Text style={styles.transferCancel}>Cancel</Text>
                    </TouchableOpacity>
                  ) : null}
                </View>
              </View>
            ))}
          </View>
        ) : null}
      </View>
    </ScrollView>
  );
};

const SectionHeader: React.FC<{ title: string; subtitle?: string }> = ({ title, subtitle }) => (
  <View style={styles.sectionHeader}>
    <Text style={styles.sectionTitle}>{title}</Text>
    {subtitle ? <Text style={styles.sectionSubtitle}>{subtitle}</Text> : null}
  </View>
);

const ControlButton: React.FC<{
  label: string;
  onPress: () => void;
  disabled?: boolean;
  variant?: 'primary' | 'secondary' | 'danger' | 'neutral';
}> = ({ label, onPress, disabled, variant = 'primary' }) => (
  <TouchableOpacity
    onPress={onPress}
    disabled={disabled}
    style={[
      styles.controlButton,
      variant === 'primary' && styles.controlButtonPrimary,
      variant === 'secondary' && styles.controlButtonSecondary,
      variant === 'danger' && styles.controlButtonDanger,
      variant === 'neutral' && styles.controlButtonNeutral,
      disabled && styles.controlButtonDisabled,
    ]}
  >
    <Text
      style={[
        styles.controlButtonText,
        (variant === 'secondary' || variant === 'neutral') && styles.controlButtonTextSecondary,
        disabled && styles.controlButtonTextDisabled,
      ]}
    >
      {label}
    </Text>
  </TouchableOpacity>
);

const StatusPill: React.FC<{ label: string; tone?: 'good' | 'info' | 'neutral' }> = ({ label, tone = 'info' }) => (
  <View
    style={[
      styles.statusPill,
      tone === 'good' && styles.statusPillGood,
      tone === 'neutral' && styles.statusPillNeutral,
    ]}
  >
    <Text style={styles.statusPillText}>{label}</Text>
  </View>
);

const StepperRow: React.FC<{
  label: string;
  value: number;
  suffix?: string;
  onChange: (delta: number) => void;
  disabled?: boolean;
}> = ({ label, value, suffix, onChange, disabled }) => (
  <View style={styles.stepperRow}>
    <View>
      <Text style={styles.stepperLabel}>{label}</Text>
      <Text style={styles.stepperValue}>
        {value}
        {suffix ?? ''}
      </Text>
    </View>
    <View style={styles.stepperControls}>
      <TouchableOpacity
        style={[styles.stepperButton, disabled && styles.stepperButtonDisabled]}
        onPress={() => !disabled && onChange(-1)}
        disabled={disabled}
      >
        <Text
          style={[
            styles.stepperButtonText,
            disabled && styles.stepperButtonTextDisabled,
          ]}
        >
          −
        </Text>
      </TouchableOpacity>
      <TouchableOpacity
        style={[styles.stepperButton, disabled && styles.stepperButtonDisabled]}
        onPress={() => !disabled && onChange(1)}
        disabled={disabled}
      >
        <Text
          style={[
            styles.stepperButtonText,
            disabled && styles.stepperButtonTextDisabled,
          ]}
        >
          +
        </Text>
      </TouchableOpacity>
    </View>
  </View>
);

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f7faff',
  },
  contentContainer: {
    padding: 20,
    paddingBottom: 32,
  },
  headerRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 16,
  },
  title: {
    fontSize: 24,
    fontWeight: '700',
    color: '#102a43',
  },
  refreshButton: {
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderRadius: 12,
    backgroundColor: '#e2e8f0',
  },
  refreshText: {
    color: '#1d4ed8',
    fontWeight: '600',
    fontSize: 13,
  },
  warningCard: {
    backgroundColor: '#fff1f2',
    borderRadius: 16,
    padding: 16,
    borderWidth: 1,
    borderColor: '#fecdd3',
    marginBottom: 16,
  },
  warningTitle: {
    fontSize: 16,
    fontWeight: '700',
    color: '#be123c',
    marginBottom: 6,
  },
  warningText: {
    fontSize: 13,
    color: '#9f1239',
  },
  section: {
    marginBottom: 20,
  },
  sectionHeader: {
    marginBottom: 12,
  },
  sectionTitle: {
    fontSize: 18,
    fontWeight: '700',
    color: '#1f2937',
  },
  sectionSubtitle: {
    fontSize: 13,
    color: '#6b7280',
    marginTop: 2,
  },
  transportCard: {
    backgroundColor: '#ffffff',
    borderRadius: 16,
    padding: 16,
    marginBottom: 12,
    shadowColor: '#0f172a',
    shadowOpacity: 0.04,
    shadowRadius: 12,
    shadowOffset: { width: 0, height: 4 },
    elevation: 2,
  },
  transportHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  transportTitle: {
    fontSize: 16,
    fontWeight: '700',
    color: '#0f172a',
  },
  transportHint: {
    marginTop: 8,
    fontSize: 13,
    color: '#64748b',
  },
  transportControls: {
    marginTop: 16,
    flexDirection: 'row',
    gap: 12,
  },
  card: {
    backgroundColor: '#ffffff',
    borderRadius: 16,
    padding: 16,
    shadowColor: '#0f172a',
    shadowOpacity: 0.04,
    shadowRadius: 12,
    shadowOffset: { width: 0, height: 4 },
    elevation: 1,
  },
  cardLabel: {
    fontSize: 13,
    fontWeight: '600',
    color: '#475569',
    marginBottom: 6,
  },
  sliderRow: {
    flexDirection: 'row',
    alignItems: 'center',
    flexWrap: 'wrap',
    gap: 8,
  },
  batteryChip: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 12,
    backgroundColor: '#e2e8f0',
  },
  batteryChipActive: {
    backgroundColor: '#1d4ed8',
  },
  batteryChipText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#475569',
  },
  batteryChipTextActive: {
    color: '#fff',
  },
  priorityRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
    marginTop: 8,
  },
  priorityChip: {
    paddingHorizontal: 14,
    paddingVertical: 6,
    borderRadius: 16,
    backgroundColor: '#e2e8f0',
  },
  priorityChipActive: {
    backgroundColor: '#2563eb',
  },
  priorityChipText: {
    fontSize: 12,
    fontWeight: '600',
    color: '#1f2937',
  },
  priorityChipTextActive: {
    color: '#fff',
  },
  toggle: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 12,
  },
  toggleActive: {
    backgroundColor: 'rgba(34,197,94,0.2)',
  },
  toggleInactive: {
    backgroundColor: '#e2e8f0',
  },
  toggleDisabled: {
    opacity: 0.6,
  },
  toggleText: {
    fontSize: 12,
    fontWeight: '600',
  },
  toggleTextActive: {
    color: '#047857',
  },
  toggleTextInactive: {
    color: '#475569',
  },
  toggleTextDisabled: {
    color: '#94a3b8',
  },
  dorsRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 12,
  },
  stepperRow: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    paddingVertical: 10,
    borderTopWidth: 1,
    borderTopColor: '#f1f5f9',
  },
  stepperLabel: {
    fontSize: 13,
    fontWeight: '600',
    color: '#475569',
  },
  stepperValue: {
    marginTop: 2,
    fontSize: 12,
    color: '#1f2937',
  },
  stepperControls: {
    flexDirection: 'row',
    gap: 8,
  },
  stepperButton: {
    width: 32,
    height: 32,
    borderRadius: 16,
    backgroundColor: '#e2e8f0',
    alignItems: 'center',
    justifyContent: 'center',
  },
  stepperButtonDisabled: {
    opacity: 0.6,
  },
  stepperButtonText: {
    fontSize: 18,
    color: '#1d4ed8',
    fontWeight: '600',
  },
  stepperButtonTextDisabled: {
    color: '#94a3b8',
  },
  inputGroup: {
    marginBottom: 12,
  },
  input: {
    borderWidth: 1,
    borderColor: '#d1d5db',
    borderRadius: 12,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 13,
    color: '#1f2937',
    backgroundColor: '#fff',
  },
  fileHint: {
    fontSize: 12,
    color: '#64748b',
    marginBottom: 12,
  },
  submitButton: {
    marginTop: 8,
    backgroundColor: '#2563eb',
    borderRadius: 14,
    alignItems: 'center',
    paddingVertical: 12,
  },
  submitButtonDisabled: {
    backgroundColor: '#93c5fd',
  },
  submitButtonText: {
    color: '#fff',
    fontWeight: '700',
  },
  transferRow: {
    paddingVertical: 12,
    borderBottomWidth: 1,
    borderBottomColor: '#f1f5f9',
  },
  transferHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  transferName: {
    flex: 1,
    fontSize: 14,
    fontWeight: '600',
    color: '#0f172a',
    marginRight: 12,
  },
  transferMeta: {
    fontSize: 12,
    color: '#64748b',
    marginTop: 4,
  },
  progressBar: {
    height: 8,
    borderRadius: 4,
    backgroundColor: '#e2e8f0',
    overflow: 'hidden',
    marginTop: 10,
  },
  progressFill: {
    height: 8,
    backgroundColor: '#2563eb',
  },
  transferFooter: {
    marginTop: 8,
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  transferProgress: {
    fontSize: 12,
    color: '#475569',
  },
  transferCancel: {
    fontSize: 12,
    color: '#ef4444',
    fontWeight: '600',
  },
  controlButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
  },
  controlButtonPrimary: {
    backgroundColor: '#2563eb',
  },
  controlButtonSecondary: {
    backgroundColor: '#1d4ed8',
  },
  controlButtonDanger: {
    backgroundColor: '#ef4444',
  },
  controlButtonNeutral: {
    backgroundColor: '#e2e8f0',
  },
  controlButtonDisabled: {
    backgroundColor: '#cbd5f5',
  },
  controlButtonText: {
    color: '#fff',
    fontWeight: '700',
    fontSize: 13,
  },
  controlButtonTextSecondary: {
    color: '#fff',
  },
  controlButtonTextDisabled: {
    color: '#f8fafc',
  },
  statusPill: {
    borderRadius: 999,
    paddingHorizontal: 10,
    paddingVertical: 4,
  },
  statusPillGood: {
    backgroundColor: 'rgba(34,197,94,0.16)',
  },
  statusPillNeutral: {
    backgroundColor: '#e2e8f0',
  },
  statusPillText: {
    fontSize: 11,
    fontWeight: '600',
    color: '#1f2937',
  },
});


