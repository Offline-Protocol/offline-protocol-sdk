import React, {useState, useEffect, useCallback, useRef} from 'react';
import {
  SafeAreaView,
  StatusBar,
  StyleSheet,
  Text,
  View,
  ScrollView,
  TouchableOpacity,
  TextInput,
  Alert,
} from 'react-native';
import {
  OfflineProtocol,
  type ProtocolEvent,
  type ServiceDiscoveredEvent,
  type ServiceRequestReceivedEvent,
  type ServiceResponseReceivedEvent,
} from '@offline-protocol/mesh-sdk';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface RegisteredService {
  serviceId: string;
  version: string;
  capabilities: Record<string, string>;
}

interface DiscoveredService {
  queryId: string;
  serviceId: string;
  version: string;
  providerPeerId: string;
  capabilities: Record<string, string>;
  hopCount: number;
  discoveredAt: number;
}

interface ServiceLog {
  id: string;
  timestamp: number;
  direction: 'in' | 'out' | 'system';
  message: string;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const generateUserId = () =>
  `user-${Math.random().toString(36).substring(2, 8)}`;

const timestamp = () => new Date().toLocaleTimeString();

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

const App = () => {
  // Protocol state
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [userId] = useState(generateUserId);

  // Service state
  const [registeredServices, setRegisteredServices] = useState<RegisteredService[]>([]);
  const [discoveredServices, setDiscoveredServices] = useState<DiscoveredService[]>([]);
  const [logs, setLogs] = useState<ServiceLog[]>([]);
  const logIdRef = useRef(0);

  // UI state
  const [activeTab, setActiveTab] = useState<'provide' | 'discover' | 'logs'>('provide');
  const [newServiceId, setNewServiceId] = useState('echo.v1');
  const [newServiceVersion, setNewServiceVersion] = useState('1.0');
  const [requestMethod, setRequestMethod] = useState('ping');
  const [requestBody, setRequestBody] = useState('{"message": "hello from the mesh!"}');

  const addLog = useCallback(
    (direction: ServiceLog['direction'], message: string) => {
      setLogs(prev => [
        {
          id: String(++logIdRef.current),
          timestamp: Date.now(),
          direction,
          message,
        },
        ...prev,
      ].slice(0, 200));
    },
    [],
  );

  // Handle incoming service events
  const handleEvent = useCallback(
    (event: ProtocolEvent) => {
      switch (event.type) {
        case 'service_discovered': {
          const e = event as ServiceDiscoveredEvent;
          addLog(
            'in',
            `Discovered "${e.service_id}" v${e.version} from ${e.provider_peer_id.slice(0, 12)}... (${e.hop_count} hop${e.hop_count !== 1 ? 's' : ''})`,
          );
          setDiscoveredServices(prev => {
            // Replace if same service+provider, otherwise append
            const key = `${e.service_id}:${e.provider_peer_id}`;
            const filtered = prev.filter(
              s => `${s.serviceId}:${s.providerPeerId}` !== key,
            );
            return [
              ...filtered,
              {
                queryId: e.query_id,
                serviceId: e.service_id,
                version: e.version,
                providerPeerId: e.provider_peer_id,
                capabilities: e.capabilities,
                hopCount: e.hop_count,
                discoveredAt: Date.now(),
              },
            ];
          });
          break;
        }

        case 'service_request_received': {
          const e = event as ServiceRequestReceivedEvent;
          addLog(
            'in',
            `Request from ${e.sender.slice(0, 12)}...: ${e.service_id}.${e.method}`,
          );

          // Auto-respond based on the method
          if (protocol) {
            let responseBody: string;
            let status = 'ok';

            try {
              const parsed = JSON.parse(e.body);
              switch (e.method) {
                case 'ping':
                  responseBody = JSON.stringify({
                    pong: true,
                    echo: parsed.message ?? null,
                    respondedBy: userId,
                    respondedAt: new Date().toISOString(),
                  });
                  break;
                case 'get_info':
                  responseBody = JSON.stringify({
                    nodeId: userId,
                    platform: 'react-native',
                    uptime: Date.now(),
                    servicesOffered: registeredServices.map(s => s.serviceId),
                  });
                  break;
                default:
                  responseBody = JSON.stringify({
                    echo: parsed,
                    method: e.method,
                    processedBy: userId,
                  });
              }
            } catch {
              responseBody = JSON.stringify({echo: e.body, processedBy: userId});
            }

            protocol
              .respondToServiceRequest(
                e.request_id,
                e.sender,
                e.service_id,
                status,
                responseBody,
              )
              .then(() => {
                addLog('out', `Responded to ${e.method} with status "${status}"`);
              })
              .catch(err => {
                addLog('system', `Failed to respond: ${err.message}`);
              });
          }
          break;
        }

        case 'service_response_received': {
          const e = event as ServiceResponseReceivedEvent;
          addLog(
            'in',
            `Response [${e.status}] from ${e.provider_peer_id.slice(0, 12)}...: ${truncate(e.body, 100)}`,
          );
          break;
        }

        case 'neighbor_discovered':
          addLog('system', `Peer joined: ${(event as any).peer_id?.slice(0, 12) ?? 'unknown'}...`);
          break;

        case 'neighbor_lost':
          addLog('system', `Peer left: ${(event as any).peer_id?.slice(0, 12) ?? 'unknown'}...`);
          break;
      }
    },
    [protocol, userId, registeredServices, addLog],
  );

  // Initialize protocol
  useEffect(() => {
    const proto = new OfflineProtocol({
      appId: 'service-discovery-demo',
      userId,
      transports: {
        ble: {enabled: true},
        internet: {enabled: false},
        wifiDirect: {enabled: false},
      },
      encryption: {enabled: false},
    });

    proto.on('all', handleEvent);
    setProtocol(proto);

    return () => {
      proto.removeAllListeners();
      proto.destroy().catch(() => {});
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Update event handler when dependencies change
  useEffect(() => {
    if (!protocol) return;
    protocol.removeAllListeners();
    protocol.on('all', handleEvent);
  }, [protocol, handleEvent]);

  const handleStart = async () => {
    if (!protocol) return;
    try {
      await protocol.start();
      setIsStarted(true);
      addLog('system', `Protocol started as ${userId}`);
    } catch (err: any) {
      Alert.alert('Start Failed', err.message);
    }
  };

  const handleStop = async () => {
    if (!protocol) return;
    try {
      await protocol.stop();
      setIsStarted(false);
      addLog('system', 'Protocol stopped');
    } catch (err: any) {
      Alert.alert('Stop Failed', err.message);
    }
  };

  // -- Service Provider actions --

  const handleRegisterService = async () => {
    if (!protocol || !isStarted) return;
    if (!newServiceId.trim()) {
      Alert.alert('Error', 'Service ID is required');
      return;
    }
    try {
      await protocol.registerService(newServiceId.trim(), newServiceVersion, {
        format: 'json',
        transport: 'mesh',
      });
      setRegisteredServices(prev => [
        ...prev.filter(s => s.serviceId !== newServiceId.trim()),
        {
          serviceId: newServiceId.trim(),
          version: newServiceVersion,
          capabilities: {format: 'json', transport: 'mesh'},
        },
      ]);
      addLog('out', `Registered service "${newServiceId.trim()}" v${newServiceVersion}`);
    } catch (err: any) {
      Alert.alert('Error', err.message);
    }
  };

  const handleUnregisterService = async (serviceId: string) => {
    if (!protocol) return;
    try {
      await protocol.unregisterService(serviceId);
      setRegisteredServices(prev => prev.filter(s => s.serviceId !== serviceId));
      addLog('out', `Unregistered service "${serviceId}"`);
    } catch (err: any) {
      Alert.alert('Error', err.message);
    }
  };

  // -- Service Discovery actions --

  const handleDiscover = async () => {
    if (!protocol || !isStarted) return;
    try {
      const queryId = await protocol.discoverServices();
      addLog('out', `Discovery broadcast sent (query: ${queryId.slice(0, 8)}...)`);
    } catch (err: any) {
      Alert.alert('Error', err.message);
    }
  };

  const handleDiscoverSpecific = async (serviceId: string) => {
    if (!protocol || !isStarted) return;
    try {
      const queryId = await protocol.discoverServices(serviceId);
      addLog('out', `Searching for "${serviceId}" (query: ${queryId.slice(0, 8)}...)`);
    } catch (err: any) {
      Alert.alert('Error', err.message);
    }
  };

  const handleSendRequest = async (service: DiscoveredService) => {
    if (!protocol || !isStarted) return;
    try {
      const requestId = await protocol.sendServiceRequest(
        service.providerPeerId,
        service.serviceId,
        requestMethod,
        requestBody,
      );
      addLog(
        'out',
        `Request "${requestMethod}" sent to ${service.providerPeerId.slice(0, 12)}... (req: ${requestId.slice(0, 8)}...)`,
      );
    } catch (err: any) {
      Alert.alert('Error', err.message);
    }
  };

  return (
    <SafeAreaView style={styles.container}>
      <StatusBar barStyle="light-content" backgroundColor="#1a1a2e" />

      {/* Header */}
      <View style={styles.header}>
        <Text style={styles.headerTitle}>Mesh Services</Text>
        <Text style={styles.headerSubtitle}>
          {userId} {isStarted ? '(online)' : '(offline)'}
        </Text>
        <TouchableOpacity
          style={[styles.startButton, isStarted && styles.stopButton]}
          onPress={isStarted ? handleStop : handleStart}>
          <Text style={styles.startButtonText}>
            {isStarted ? 'Stop' : 'Start'}
          </Text>
        </TouchableOpacity>
      </View>

      {/* Tabs */}
      <View style={styles.tabs}>
        {(['provide', 'discover', 'logs'] as const).map(tab => (
          <TouchableOpacity
            key={tab}
            style={[styles.tab, activeTab === tab && styles.activeTab]}
            onPress={() => setActiveTab(tab)}>
            <Text
              style={[
                styles.tabText,
                activeTab === tab && styles.activeTabText,
              ]}>
              {tab === 'provide'
                ? `Provide (${registeredServices.length})`
                : tab === 'discover'
                  ? `Discover (${discoveredServices.length})`
                  : `Logs (${logs.length})`}
            </Text>
          </TouchableOpacity>
        ))}
      </View>

      {/* Tab Content */}
      <ScrollView style={styles.content} contentContainerStyle={styles.contentInner}>
        {activeTab === 'provide' && (
          <ProvideTab
            isStarted={isStarted}
            registeredServices={registeredServices}
            newServiceId={newServiceId}
            setNewServiceId={setNewServiceId}
            newServiceVersion={newServiceVersion}
            setNewServiceVersion={setNewServiceVersion}
            onRegister={handleRegisterService}
            onUnregister={handleUnregisterService}
          />
        )}

        {activeTab === 'discover' && (
          <DiscoverTab
            isStarted={isStarted}
            discoveredServices={discoveredServices}
            requestMethod={requestMethod}
            setRequestMethod={setRequestMethod}
            requestBody={requestBody}
            setRequestBody={setRequestBody}
            onDiscover={handleDiscover}
            onDiscoverSpecific={handleDiscoverSpecific}
            onSendRequest={handleSendRequest}
          />
        )}

        {activeTab === 'logs' && <LogsTab logs={logs} onClear={() => setLogs([])} />}
      </ScrollView>
    </SafeAreaView>
  );
};

// ---------------------------------------------------------------------------
// Provide Tab
// ---------------------------------------------------------------------------

interface ProvideTabProps {
  isStarted: boolean;
  registeredServices: RegisteredService[];
  newServiceId: string;
  setNewServiceId: (v: string) => void;
  newServiceVersion: string;
  setNewServiceVersion: (v: string) => void;
  onRegister: () => void;
  onUnregister: (id: string) => void;
}

const ProvideTab = ({
  isStarted,
  registeredServices,
  newServiceId,
  setNewServiceId,
  newServiceVersion,
  setNewServiceVersion,
  onRegister,
  onUnregister,
}: ProvideTabProps) => (
  <View>
    <Text style={styles.sectionTitle}>Register a Service</Text>
    <Text style={styles.hint}>
      Offer a service on the mesh for nearby peers to discover and use.
    </Text>

    <View style={styles.inputRow}>
      <TextInput
        style={[styles.input, {flex: 2}]}
        value={newServiceId}
        onChangeText={setNewServiceId}
        placeholder="Service ID (e.g. echo.v1)"
        placeholderTextColor="#666"
      />
      <TextInput
        style={[styles.input, {flex: 1, marginLeft: 8}]}
        value={newServiceVersion}
        onChangeText={setNewServiceVersion}
        placeholder="Version"
        placeholderTextColor="#666"
      />
    </View>

    <TouchableOpacity
      style={[styles.button, !isStarted && styles.buttonDisabled]}
      onPress={onRegister}
      disabled={!isStarted}>
      <Text style={styles.buttonText}>Register Service</Text>
    </TouchableOpacity>

    {/* Quick-register presets */}
    <Text style={[styles.sectionTitle, {marginTop: 20}]}>Quick Register</Text>
    <View style={styles.presetRow}>
      {[
        {id: 'echo.v1', label: 'Echo'},
        {id: 'notes.v1', label: 'Notes'},
        {id: 'weather.v1', label: 'Weather'},
        {id: 'translate.v1', label: 'Translate'},
      ].map(preset => (
        <TouchableOpacity
          key={preset.id}
          style={styles.presetChip}
          onPress={() => {
            setNewServiceId(preset.id);
            setNewServiceVersion('1.0');
          }}>
          <Text style={styles.presetChipText}>{preset.label}</Text>
        </TouchableOpacity>
      ))}
    </View>

    {/* Registered services list */}
    <Text style={[styles.sectionTitle, {marginTop: 24}]}>
      My Services ({registeredServices.length})
    </Text>
    {registeredServices.length === 0 ? (
      <Text style={styles.emptyText}>No services registered yet.</Text>
    ) : (
      registeredServices.map(svc => (
        <View key={svc.serviceId} style={styles.serviceCard}>
          <View style={{flex: 1}}>
            <Text style={styles.serviceId}>{svc.serviceId}</Text>
            <Text style={styles.serviceVersion}>v{svc.version}</Text>
          </View>
          <TouchableOpacity
            style={styles.removeButton}
            onPress={() => onUnregister(svc.serviceId)}>
            <Text style={styles.removeButtonText}>Remove</Text>
          </TouchableOpacity>
        </View>
      ))
    )}
  </View>
);

// ---------------------------------------------------------------------------
// Discover Tab
// ---------------------------------------------------------------------------

interface DiscoverTabProps {
  isStarted: boolean;
  discoveredServices: DiscoveredService[];
  requestMethod: string;
  setRequestMethod: (v: string) => void;
  requestBody: string;
  setRequestBody: (v: string) => void;
  onDiscover: () => void;
  onDiscoverSpecific: (id: string) => void;
  onSendRequest: (svc: DiscoveredService) => void;
}

const DiscoverTab = ({
  isStarted,
  discoveredServices,
  requestMethod,
  setRequestMethod,
  requestBody,
  setRequestBody,
  onDiscover,
  onDiscoverSpecific,
  onSendRequest,
}: DiscoverTabProps) => (
  <View>
    <Text style={styles.sectionTitle}>Find Services</Text>
    <Text style={styles.hint}>
      Scan the mesh for services offered by nearby peers.
    </Text>

    <TouchableOpacity
      style={[styles.button, !isStarted && styles.buttonDisabled]}
      onPress={onDiscover}
      disabled={!isStarted}>
      <Text style={styles.buttonText}>Discover All Services</Text>
    </TouchableOpacity>

    {/* Quick-discover presets */}
    <View style={[styles.presetRow, {marginTop: 8}]}>
      {['echo.v1', 'notes.v1', 'weather.v1'].map(id => (
        <TouchableOpacity
          key={id}
          style={[styles.presetChip, styles.discoverChip]}
          onPress={() => onDiscoverSpecific(id)}
          disabled={!isStarted}>
          <Text style={styles.presetChipText}>Find {id.split('.')[0]}</Text>
        </TouchableOpacity>
      ))}
    </View>

    {/* Request builder */}
    <Text style={[styles.sectionTitle, {marginTop: 24}]}>Request Builder</Text>
    <TextInput
      style={styles.input}
      value={requestMethod}
      onChangeText={setRequestMethod}
      placeholder="Method (e.g. ping, get_info)"
      placeholderTextColor="#666"
    />
    <TextInput
      style={[styles.input, styles.bodyInput]}
      value={requestBody}
      onChangeText={setRequestBody}
      placeholder="Request body (JSON)"
      placeholderTextColor="#666"
      multiline
    />

    {/* Discovered services */}
    <Text style={[styles.sectionTitle, {marginTop: 24}]}>
      Discovered ({discoveredServices.length})
    </Text>
    {discoveredServices.length === 0 ? (
      <Text style={styles.emptyText}>
        No services found yet. Tap "Discover All Services" to scan.
      </Text>
    ) : (
      discoveredServices.map((svc, i) => (
        <View key={`${svc.serviceId}:${svc.providerPeerId}:${i}`} style={styles.discoveredCard}>
          <View style={styles.discoveredHeader}>
            <Text style={styles.serviceId}>{svc.serviceId}</Text>
            <View style={styles.hopBadge}>
              <Text style={styles.hopBadgeText}>
                {svc.hopCount} hop{svc.hopCount !== 1 ? 's' : ''}
              </Text>
            </View>
          </View>
          <Text style={styles.providerText}>
            Provider: {svc.providerPeerId.slice(0, 16)}...
          </Text>
          <Text style={styles.serviceVersion}>
            v{svc.version}
            {Object.keys(svc.capabilities).length > 0 &&
              ` | ${Object.entries(svc.capabilities)
                .map(([k, v]) => `${k}=${v}`)
                .join(', ')}`}
          </Text>
          <TouchableOpacity
            style={[styles.button, styles.requestButton]}
            onPress={() => onSendRequest(svc)}>
            <Text style={styles.buttonText}>
              Send "{requestMethod}" Request
            </Text>
          </TouchableOpacity>
        </View>
      ))
    )}
  </View>
);

// ---------------------------------------------------------------------------
// Logs Tab
// ---------------------------------------------------------------------------

interface LogsTabProps {
  logs: ServiceLog[];
  onClear: () => void;
}

const LogsTab = ({logs, onClear}: LogsTabProps) => (
  <View>
    <View style={styles.logHeader}>
      <Text style={styles.sectionTitle}>Activity Log</Text>
      <TouchableOpacity onPress={onClear}>
        <Text style={styles.clearText}>Clear</Text>
      </TouchableOpacity>
    </View>
    {logs.length === 0 ? (
      <Text style={styles.emptyText}>No activity yet.</Text>
    ) : (
      logs.map(log => (
        <View key={log.id} style={styles.logEntry}>
          <Text style={styles.logTime}>
            {new Date(log.timestamp).toLocaleTimeString()}
          </Text>
          <Text
            style={[
              styles.logDirection,
              log.direction === 'in' && styles.logIn,
              log.direction === 'out' && styles.logOut,
              log.direction === 'system' && styles.logSystem,
            ]}>
            {log.direction === 'in' ? 'IN' : log.direction === 'out' ? 'OUT' : 'SYS'}
          </Text>
          <Text style={styles.logMessage} numberOfLines={3}>
            {log.message}
          </Text>
        </View>
      ))
    )}
  </View>
);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function truncate(str: string, maxLen: number): string {
  return str.length > maxLen ? str.slice(0, maxLen) + '...' : str;
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#0f0f23',
  },
  header: {
    paddingHorizontal: 16,
    paddingVertical: 12,
    backgroundColor: '#1a1a2e',
    borderBottomWidth: 1,
    borderBottomColor: '#2a2a4a',
    flexDirection: 'row',
    alignItems: 'center',
  },
  headerTitle: {
    fontSize: 20,
    fontWeight: '700',
    color: '#e0e0ff',
  },
  headerSubtitle: {
    fontSize: 12,
    color: '#8888aa',
    marginLeft: 12,
    flex: 1,
  },
  startButton: {
    backgroundColor: '#2d6a4f',
    paddingHorizontal: 16,
    paddingVertical: 8,
    borderRadius: 6,
  },
  stopButton: {
    backgroundColor: '#9b2226',
  },
  startButtonText: {
    color: '#fff',
    fontWeight: '600',
    fontSize: 14,
  },
  tabs: {
    flexDirection: 'row',
    backgroundColor: '#1a1a2e',
    borderBottomWidth: 1,
    borderBottomColor: '#2a2a4a',
  },
  tab: {
    flex: 1,
    paddingVertical: 10,
    alignItems: 'center',
  },
  activeTab: {
    borderBottomWidth: 2,
    borderBottomColor: '#6c63ff',
  },
  tabText: {
    color: '#8888aa',
    fontSize: 13,
    fontWeight: '500',
  },
  activeTabText: {
    color: '#e0e0ff',
  },
  content: {
    flex: 1,
  },
  contentInner: {
    padding: 16,
    paddingBottom: 40,
  },
  sectionTitle: {
    fontSize: 16,
    fontWeight: '600',
    color: '#e0e0ff',
    marginBottom: 6,
  },
  hint: {
    fontSize: 13,
    color: '#8888aa',
    marginBottom: 12,
  },
  inputRow: {
    flexDirection: 'row',
    marginBottom: 10,
  },
  input: {
    backgroundColor: '#1a1a2e',
    borderWidth: 1,
    borderColor: '#2a2a4a',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    color: '#e0e0ff',
    fontSize: 14,
    marginBottom: 8,
  },
  bodyInput: {
    minHeight: 60,
    textAlignVertical: 'top',
  },
  button: {
    backgroundColor: '#6c63ff',
    paddingVertical: 12,
    borderRadius: 8,
    alignItems: 'center',
  },
  buttonDisabled: {
    opacity: 0.4,
  },
  buttonText: {
    color: '#fff',
    fontWeight: '600',
    fontSize: 14,
  },
  requestButton: {
    marginTop: 10,
    backgroundColor: '#2d6a4f',
  },
  presetRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
  },
  presetChip: {
    backgroundColor: '#2a2a4a',
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderRadius: 16,
  },
  discoverChip: {
    backgroundColor: '#1a3a5c',
  },
  presetChipText: {
    color: '#c0c0e0',
    fontSize: 13,
  },
  serviceCard: {
    backgroundColor: '#1a1a2e',
    borderWidth: 1,
    borderColor: '#2a2a4a',
    borderRadius: 8,
    padding: 12,
    marginBottom: 8,
    flexDirection: 'row',
    alignItems: 'center',
  },
  serviceId: {
    color: '#e0e0ff',
    fontSize: 15,
    fontWeight: '600',
  },
  serviceVersion: {
    color: '#8888aa',
    fontSize: 12,
    marginTop: 2,
  },
  removeButton: {
    backgroundColor: '#9b2226',
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 6,
  },
  removeButtonText: {
    color: '#fff',
    fontSize: 12,
    fontWeight: '600',
  },
  discoveredCard: {
    backgroundColor: '#1a1a2e',
    borderWidth: 1,
    borderColor: '#2a3a5a',
    borderRadius: 8,
    padding: 12,
    marginBottom: 10,
  },
  discoveredHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  hopBadge: {
    backgroundColor: '#2d6a4f',
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 10,
  },
  hopBadgeText: {
    color: '#b7e4c7',
    fontSize: 11,
    fontWeight: '600',
  },
  providerText: {
    color: '#6c8cbf',
    fontSize: 12,
    marginTop: 4,
    fontFamily: 'monospace',
  },
  emptyText: {
    color: '#555577',
    fontSize: 13,
    fontStyle: 'italic',
    textAlign: 'center',
    paddingVertical: 20,
  },
  logHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 8,
  },
  clearText: {
    color: '#6c63ff',
    fontSize: 13,
  },
  logEntry: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    paddingVertical: 6,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: '#1a1a2e',
  },
  logTime: {
    color: '#555577',
    fontSize: 11,
    width: 70,
    fontFamily: 'monospace',
  },
  logDirection: {
    fontSize: 10,
    fontWeight: '700',
    width: 30,
    textAlign: 'center',
    marginRight: 8,
    marginTop: 1,
  },
  logIn: {
    color: '#52b788',
  },
  logOut: {
    color: '#6c63ff',
  },
  logSystem: {
    color: '#8888aa',
  },
  logMessage: {
    color: '#c0c0e0',
    fontSize: 12,
    flex: 1,
  },
});

export default App;
