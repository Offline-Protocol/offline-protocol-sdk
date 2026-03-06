import React, {useState, useEffect, useCallback, useRef} from 'react';
import {
  SafeAreaView,
  StatusBar,
  StyleSheet,
  Text,
  View,
  ScrollView,
  TouchableOpacity,
  Alert,
  Switch,
} from 'react-native';
import {
  OfflineProtocol,
  type ProtocolEvent,
  type ServiceDiscoveredEvent,
  type ServiceRequestReceivedEvent,
  type ServiceResponseReceivedEvent,
} from '@offline-protocol/mesh-sdk';

// ---------------------------------------------------------------------------
// Knowledge Pack Data
// ---------------------------------------------------------------------------

interface KnowledgeEntry {
  question: string;
  answer: string;
}

interface KnowledgePack {
  serviceId: string;
  version: string;
  name: string;
  icon: string;
  entries: Record<string, KnowledgeEntry>;
}

const KNOWLEDGE_PACKS: Record<string, KnowledgePack> = {
  'first-aid.v1': {
    serviceId: 'first-aid.v1',
    version: '1.0',
    name: 'First Aid',
    icon: '+',
    entries: {
      cpr: {
        question: 'How do I perform CPR?',
        answer:
          '1. Call for help. 2. Place heel of one hand on center of chest, other hand on top. 3. Push hard and fast — at least 2 inches deep, 100-120 compressions/min. 4. After 30 compressions, tilt head back, lift chin, give 2 rescue breaths. 5. Repeat until help arrives or the person recovers.',
      },
      burns: {
        question: 'How to treat a burn?',
        answer:
          '1. Cool the burn under cool (not cold) running water for at least 10 minutes. 2. Remove jewelry or tight clothing near the burn before swelling starts. 3. Cover with a clean, non-stick bandage. 4. Do NOT apply ice, butter, or toothpaste. 5. Seek medical help for burns larger than your palm or on the face/hands/joints.',
      },
      bleeding: {
        question: 'How to stop bleeding?',
        answer:
          '1. Apply firm, direct pressure with a clean cloth or bandage. 2. Keep pressure steady — do not lift to check. 3. If blood soaks through, add more cloth on top. 4. Elevate the injured area above the heart if possible. 5. For severe bleeding, apply a tourniquet 2-3 inches above the wound and note the time.',
      },
      choking: {
        question: 'What to do if someone is choking?',
        answer:
          '1. Ask "Are you choking?" — if they cannot speak or cough, act immediately. 2. Stand behind them, wrap your arms around their waist. 3. Make a fist with one hand, place it just above the navel. 4. Grasp your fist with the other hand and thrust inward and upward. 5. Repeat until the object is expelled or the person can breathe.',
      },
      sprains: {
        question: 'How to treat a sprain?',
        answer:
          'Remember RICE: 1. Rest — stop using the injured area. 2. Ice — apply ice wrapped in cloth for 15-20 minutes every 2-3 hours. 3. Compression — wrap with an elastic bandage (snug, not tight). 4. Elevation — raise the injured area above heart level. Seek medical help if you cannot bear weight or the swelling is severe.',
      },
    },
  },
  'cooking.v1': {
    serviceId: 'cooking.v1',
    version: '1.0',
    name: 'Cooking Basics',
    icon: '~',
    entries: {
      rice: {
        question: 'How to cook rice?',
        answer:
          '1. Rinse 1 cup rice under cold water until water runs clear. 2. Add to pot with 1.5 cups water (white) or 2 cups (brown). 3. Bring to boil, then reduce to lowest heat. 4. Cover tightly and cook 15 min (white) or 40 min (brown). 5. Remove from heat, keep covered 5 min, then fluff with fork.',
      },
      eggs: {
        question: 'How to boil eggs perfectly?',
        answer:
          'Soft-boiled: Place eggs in boiling water, cook 6-7 min, then ice bath. Medium: 9-10 min. Hard-boiled: 12-13 min, then ice bath for 5 min. Start with room-temperature eggs to prevent cracking. Older eggs peel easier than fresh ones.',
      },
      substitutions: {
        question: 'Common ingredient substitutions?',
        answer:
          'No eggs? Use 1/4 cup applesauce or mashed banana per egg. No buttermilk? Add 1 tbsp vinegar to 1 cup milk, wait 5 min. No baking powder? Mix 1/4 tsp baking soda + 1/2 tsp cream of tartar. No butter? Use equal amount of coconut oil or applesauce (for baking). No onion? Use 1 tsp onion powder per small onion.',
      },
      bread: {
        question: 'How to make simple flatbread?',
        answer:
          '1. Mix 2 cups flour, 1/2 tsp salt, 3/4 cup warm water. 2. Knead 5 minutes until smooth. 3. Divide into 8 balls, roll each thin. 4. Cook in dry hot skillet 1-2 min per side until bubbles form and brown spots appear. 5. Brush with butter or oil if desired. No yeast needed!',
      },
    },
  },
  'diy-repair.v1': {
    serviceId: 'diy-repair.v1',
    version: '1.0',
    name: 'DIY Repair',
    icon: '#',
    entries: {
      leaky_faucet: {
        question: 'How to fix a leaky faucet?',
        answer:
          '1. Turn off water supply under the sink. 2. Remove the handle (usually a screw under a decorative cap). 3. Remove the cartridge or stem. 4. Replace the O-ring or washer (bring old one to hardware store to match size). 5. Reassemble in reverse order and turn water back on slowly.',
      },
      clogged_drain: {
        question: 'How to unclog a drain?',
        answer:
          '1. Try a plunger first — seal it over drain and pump vigorously. 2. If that fails, pour 1/2 cup baking soda then 1/2 cup vinegar down the drain. Wait 30 min, then flush with boiling water. 3. For hair clogs, use a drain snake or bent wire hanger to pull debris out. 4. Avoid chemical drain cleaners — they can damage pipes.',
      },
      wall_hole: {
        question: 'How to patch a hole in drywall?',
        answer:
          'Small holes (nail-sized): Fill with spackle, let dry, sand smooth, paint. Medium holes (up to 4 inches): Cut a drywall patch slightly larger, trace on wall, cut out damaged area, secure patch with drywall tape and joint compound. Apply 2-3 thin coats, sanding between each. Prime and paint.',
      },
      squeaky_door: {
        question: 'How to fix a squeaky door?',
        answer:
          '1. Apply WD-40, silicone spray, or petroleum jelly to the hinge pins. 2. Open and close the door several times to work it in. 3. If that fails, remove hinge pins one at a time, coat with lubricant, and replace. 4. For persistent squeaks, the pin may be worn — replace the hinge.',
      },
    },
  },
  'survival.v1': {
    serviceId: 'survival.v1',
    version: '1.0',
    name: 'Outdoor Survival',
    icon: '*',
    entries: {
      water: {
        question: 'How to find and purify water?',
        answer:
          'Finding: Follow animal tracks downhill, listen for running water, look for green vegetation. Purifying: 1. Boiling (1 min at rolling boil) is most reliable. 2. UV from direct sunlight in clear plastic bottle for 6+ hours. 3. Filter through layers of sand, charcoal, and gravel. Always purify — even clear-looking water can contain parasites.',
      },
      fire: {
        question: 'How to start a fire without matches?',
        answer:
          '1. Gather tinder (dry grass, bark shavings, lint), kindling (small sticks), and fuel (larger wood). 2. Friction method: carve a notch in a dry board, spin a dry stick rapidly in the notch with your palms. 3. Flint and steel: strike steel against flint near tinder. 4. Battery + steel wool: touch both terminals to fine steel wool. Build fire gradually — tinder first, then kindling, then fuel.',
      },
      shelter: {
        question: 'How to build an emergency shelter?',
        answer:
          'Lean-to: 1. Find a long, sturdy branch (ridgepole). 2. Prop one end on a rock, stump, or low tree fork. 3. Lean shorter branches along one side at 45 degrees. 4. Layer leaves, pine needles, or ferns thickly on top for waterproofing. 5. Add insulation on the ground inside (leaves, pine boughs). Face the open side away from wind.',
      },
      navigation: {
        question: 'How to navigate without a compass?',
        answer:
          'Sun: rises in east, sets in west. At noon, shadows point roughly north (northern hemisphere). Stars: find the North Star (Polaris) — follow the two pointer stars at the end of the Big Dipper. Stick method: place a stick upright, mark shadow tip, wait 15 min, mark again — line between marks runs east-west.',
      },
      signaling: {
        question: 'How to signal for rescue?',
        answer:
          '1. Universal distress signal: 3 of anything (3 fires, 3 whistle blasts, 3 mirror flashes). 2. Ground signals: make large X or SOS with rocks, logs, or trampled snow — at least 10 feet tall. 3. Signal mirror: angle reflected sunlight toward aircraft or horizon. 4. Whistle carries farther than shouting and uses less energy. 5. At night, keep a fire burning in an open area.',
      },
    },
  },
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface DiscoveredPack {
  serviceId: string;
  version: string;
  providerPeerId: string;
  capabilities: Record<string, string>;
  hopCount: number;
  discoveredAt: number;
}

interface TopicAnswer {
  serviceId: string;
  providerPeerId: string;
  topic: string;
  question: string;
  answer: string;
  receivedAt: number;
}

interface ActivityLog {
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

function truncate(str: string, maxLen: number): string {
  return str.length > maxLen ? str.slice(0, maxLen) + '...' : str;
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

const App = () => {
  // Protocol state
  const [protocol, setProtocol] = useState<OfflineProtocol | null>(null);
  const [isStarted, setIsStarted] = useState(false);
  const [userId] = useState(generateUserId);

  // Host state — which packs are enabled
  const [enabledPacks, setEnabledPacks] = useState<Set<string>>(new Set());

  // Search state
  const [discoveredPacks, setDiscoveredPacks] = useState<DiscoveredPack[]>([]);
  const [answers, setAnswers] = useState<TopicAnswer[]>([]);
  const [pendingRequests, setPendingRequests] = useState<
    Map<string, {serviceId: string; providerPeerId: string; topic: string}>
  >(new Map());

  // Log state
  const [logs, setLogs] = useState<ActivityLog[]>([]);
  const logIdRef = useRef(0);

  // UI state
  const [activeTab, setActiveTab] = useState<'host' | 'search' | 'logs'>(
    'host',
  );

  const addLog = useCallback(
    (direction: ActivityLog['direction'], message: string) => {
      setLogs(prev =>
        [
          {
            id: String(++logIdRef.current),
            timestamp: Date.now(),
            direction,
            message,
          },
          ...prev,
        ].slice(0, 200),
      );
    },
    [],
  );

  // Handle incoming events
  const handleEvent = useCallback(
    (event: ProtocolEvent) => {
      switch (event.type) {
        case 'service_discovered': {
          const e = event as ServiceDiscoveredEvent;
          addLog(
            'in',
            `Found "${e.service_id}" from ${e.provider_peer_id.slice(0, 12)}... (${e.hop_count} hop${e.hop_count !== 1 ? 's' : ''})`,
          );
          setDiscoveredPacks(prev => {
            const key = `${e.service_id}:${e.provider_peer_id}`;
            const filtered = prev.filter(
              s => `${s.serviceId}:${s.providerPeerId}` !== key,
            );
            return [
              ...filtered,
              {
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
            `Query from ${e.sender.slice(0, 12)}...: ${e.service_id} / ${e.method}`,
          );

          // Look up the topic in our local knowledge pack
          if (protocol) {
            if (!enabledPacks.has(e.service_id)) {
              protocol
                .respondToServiceRequest(
                  e.request_id,
                  e.sender,
                  e.service_id,
                  'unavailable',
                  JSON.stringify({error: 'Pack not currently hosted'}),
                )
                .catch(() => {});
              addLog('system', `Ignored request for disabled pack "${e.service_id}"`);
              break;
            }

            const pack = KNOWLEDGE_PACKS[e.service_id];
            let status: string;
            let responseBody: string;

            if (pack && pack.entries[e.method]) {
              const entry = pack.entries[e.method];
              status = 'ok';
              responseBody = JSON.stringify({
                topic: e.method,
                question: entry.question,
                answer: entry.answer,
                source: userId,
              });
            } else if (e.method === 'list_topics' && pack) {
              status = 'ok';
              responseBody = JSON.stringify({
                topics: Object.keys(pack.entries),
                packName: pack.name,
                source: userId,
              });
            } else {
              status = 'not_found';
              responseBody = JSON.stringify({
                error: 'Topic not found',
                available: pack
                  ? Object.keys(pack.entries)
                  : [],
              });
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
                addLog(
                  'out',
                  `Answered ${e.method} [${status}] to ${e.sender.slice(0, 12)}...`,
                );
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
            `Answer [${e.status}] from ${e.provider_peer_id.slice(0, 12)}...: ${truncate(e.body, 80)}`,
          );

          // Match to a pending request and display the answer
          const info = pendingRequests.get(e.request_id);
          if (info) {
            setPendingRequests(prev => {
              const next = new Map(prev);
              next.delete(e.request_id);
              return next;
            });
            try {
              const parsed = JSON.parse(e.body);
              if (e.status === 'ok' && parsed.answer) {
                setAnswers(a => [
                  {
                    serviceId: info.serviceId,
                    providerPeerId: info.providerPeerId,
                    topic: info.topic,
                    question: parsed.question ?? info.topic,
                    answer: parsed.answer,
                    receivedAt: Date.now(),
                  },
                  ...a,
                ]);
              }
            } catch {
              // non-JSON response, ignore
            }
          }
          break;
        }

        case 'neighbor_discovered':
          addLog(
            'system',
            `Peer joined: ${(event as any).peer_id?.slice(0, 12) ?? 'unknown'}...`,
          );
          break;

        case 'neighbor_lost':
          addLog(
            'system',
            `Peer left: ${(event as any).peer_id?.slice(0, 12) ?? 'unknown'}...`,
          );
          break;
      }
    },
    [protocol, userId, enabledPacks, pendingRequests, addLog],
  );

  // Initialize protocol
  useEffect(() => {
    const proto = new OfflineProtocol({
      appId: 'mesh-wiki',
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
      addLog('system', `MeshWiki started as ${userId}`);
    } catch (err: any) {
      Alert.alert('Start Failed', err.message);
    }
  };

  const handleStop = async () => {
    if (!protocol) return;
    try {
      await protocol.stop();
      setIsStarted(false);
      addLog('system', 'MeshWiki stopped');
    } catch (err: any) {
      Alert.alert('Stop Failed', err.message);
    }
  };

  // -- Host actions --

  const handleTogglePack = async (packId: string, enable: boolean) => {
    if (!protocol || !isStarted) return;
    const pack = KNOWLEDGE_PACKS[packId];
    if (!pack) return;

    try {
      if (enable) {
        await protocol.registerService(pack.serviceId, pack.version, {
          topics: Object.keys(pack.entries).join(','),
          packName: pack.name,
        });
        setEnabledPacks(prev => new Set(prev).add(packId));
        addLog('out', `Hosting "${pack.name}" (${Object.keys(pack.entries).length} topics)`);
      } else {
        await protocol.unregisterService(pack.serviceId);
        setEnabledPacks(prev => {
          const next = new Set(prev);
          next.delete(packId);
          return next;
        });
        addLog('out', `Stopped hosting "${pack.name}"`);
      }
    } catch (err: any) {
      Alert.alert('Error', err.message);
    }
  };

  // -- Search actions --

  const handleScanMesh = async () => {
    if (!protocol || !isStarted) return;
    try {
      const queryId = await protocol.discoverServices();
      addLog(
        'out',
        `Scanning mesh for all knowledge packs (query: ${queryId.slice(0, 8)}...)`,
      );
    } catch (err: any) {
      Alert.alert('Error', err.message);
    }
  };

  const handleQueryTopic = async (
    pack: DiscoveredPack,
    topicKey: string,
  ) => {
    if (!protocol || !isStarted) return;
    try {
      const requestId = await protocol.sendServiceRequest(
        pack.providerPeerId,
        pack.serviceId,
        topicKey,
        '{}',
      );
      setPendingRequests(prev => {
        const next = new Map(prev);
        next.set(requestId, {
          serviceId: pack.serviceId,
          providerPeerId: pack.providerPeerId,
          topic: topicKey,
        });
        return next;
      });
      addLog(
        'out',
        `Asked "${topicKey}" from ${pack.serviceId} @ ${pack.providerPeerId.slice(0, 12)}...`,
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
        <View style={{flex: 1}}>
          <Text style={styles.headerTitle}>MeshWiki</Text>
          <Text style={styles.headerSubtitle}>
            {userId} {isStarted ? '(online)' : '(offline)'}
          </Text>
        </View>
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
        {(['host', 'search', 'logs'] as const).map(tab => (
          <TouchableOpacity
            key={tab}
            style={[styles.tab, activeTab === tab && styles.activeTab]}
            onPress={() => setActiveTab(tab)}>
            <Text
              style={[
                styles.tabText,
                activeTab === tab && styles.activeTabText,
              ]}>
              {tab === 'host'
                ? `Host (${enabledPacks.size})`
                : tab === 'search'
                  ? `Search (${discoveredPacks.length})`
                  : `Log (${logs.length})`}
            </Text>
          </TouchableOpacity>
        ))}
      </View>

      {/* Tab Content */}
      <ScrollView style={styles.content} contentContainerStyle={styles.contentInner}>
        {activeTab === 'host' && (
          <HostTab
            isStarted={isStarted}
            enabledPacks={enabledPacks}
            onTogglePack={handleTogglePack}
          />
        )}

        {activeTab === 'search' && (
          <SearchTab
            isStarted={isStarted}
            discoveredPacks={discoveredPacks}
            answers={answers}
            onScanMesh={handleScanMesh}
            onQueryTopic={handleQueryTopic}
            onClearAnswers={() => setAnswers([])}
          />
        )}

        {activeTab === 'logs' && (
          <LogsTab logs={logs} onClear={() => setLogs([])} />
        )}
      </ScrollView>
    </SafeAreaView>
  );
};

// ---------------------------------------------------------------------------
// Host Tab
// ---------------------------------------------------------------------------

interface HostTabProps {
  isStarted: boolean;
  enabledPacks: Set<string>;
  onTogglePack: (packId: string, enable: boolean) => void;
}

const HostTab = ({isStarted, enabledPacks, onTogglePack}: HostTabProps) => (
  <View>
    <Text style={styles.sectionTitle}>Knowledge Packs</Text>
    <Text style={styles.hint}>
      Toggle packs to host on the mesh. Other devices can discover and query
      your enabled topics.
    </Text>

    {Object.values(KNOWLEDGE_PACKS).map(pack => {
      const enabled = enabledPacks.has(pack.serviceId);
      return (
        <View key={pack.serviceId} style={styles.packCard}>
          <View style={styles.packHeader}>
            <View style={styles.packIcon}>
              <Text style={styles.packIconText}>{pack.icon}</Text>
            </View>
            <View style={{flex: 1, marginLeft: 12}}>
              <Text style={styles.packName}>{pack.name}</Text>
              <Text style={styles.packMeta}>
                {pack.serviceId} v{pack.version} | {Object.keys(pack.entries).length} topics
              </Text>
            </View>
            <Switch
              value={enabled}
              onValueChange={val => onTogglePack(pack.serviceId, val)}
              disabled={!isStarted}
              trackColor={{false: '#2a2a4a', true: '#2d6a4f'}}
              thumbColor={enabled ? '#b7e4c7' : '#666'}
            />
          </View>

          {/* Topic list */}
          <View style={styles.topicList}>
            {Object.keys(pack.entries).map(key => (
              <View key={key} style={styles.topicChip}>
                <Text style={styles.topicChipText}>{key}</Text>
              </View>
            ))}
          </View>
        </View>
      );
    })}

    {!isStarted && (
      <Text style={styles.emptyText}>
        Start MeshWiki to begin hosting knowledge packs.
      </Text>
    )}
  </View>
);

// ---------------------------------------------------------------------------
// Search Tab
// ---------------------------------------------------------------------------

interface SearchTabProps {
  isStarted: boolean;
  discoveredPacks: DiscoveredPack[];
  answers: TopicAnswer[];
  onScanMesh: () => void;
  onQueryTopic: (pack: DiscoveredPack, topicKey: string) => void;
  onClearAnswers: () => void;
}

const SearchTab = ({
  isStarted,
  discoveredPacks,
  answers,
  onScanMesh,
  onQueryTopic,
  onClearAnswers,
}: SearchTabProps) => (
  <View>
    <Text style={styles.sectionTitle}>Scan the Mesh</Text>
    <Text style={styles.hint}>
      Find knowledge packs hosted by nearby devices. Tap a topic to get the
      answer.
    </Text>

    <TouchableOpacity
      style={[styles.button, !isStarted && styles.buttonDisabled]}
      onPress={onScanMesh}
      disabled={!isStarted}>
      <Text style={styles.buttonText}>Scan Mesh</Text>
    </TouchableOpacity>

    {/* Discovered packs */}
    <Text style={[styles.sectionTitle, {marginTop: 24}]}>
      Available Packs ({discoveredPacks.length})
    </Text>
    {discoveredPacks.length === 0 ? (
      <Text style={styles.emptyText}>
        No packs found yet. Tap "Scan Mesh" to discover.
      </Text>
    ) : (
      discoveredPacks.map((pack, i) => {
        const topics = pack.capabilities.topics?.split(',') ?? [];
        const packName = pack.capabilities.packName ?? pack.serviceId;
        return (
          <View
            key={`${pack.serviceId}:${pack.providerPeerId}:${i}`}
            style={styles.discoveredCard}>
            <View style={styles.discoveredHeader}>
              <Text style={styles.packName}>{packName}</Text>
              <View style={styles.hopBadge}>
                <Text style={styles.hopBadgeText}>
                  {pack.hopCount} hop{pack.hopCount !== 1 ? 's' : ''}
                </Text>
              </View>
            </View>
            <Text style={styles.providerText}>
              {pack.serviceId} v{pack.version} | {pack.providerPeerId.slice(0, 16)}...
            </Text>

            {/* Queryable topics */}
            {topics.length > 0 && (
              <View style={styles.topicButtonRow}>
                {topics.map(topic => (
                  <TouchableOpacity
                    key={topic}
                    style={styles.topicButton}
                    onPress={() => onQueryTopic(pack, topic)}>
                    <Text style={styles.topicButtonText}>{topic}</Text>
                  </TouchableOpacity>
                ))}
              </View>
            )}
          </View>
        );
      })
    )}

    {/* Answers */}
    <View style={[styles.answerHeader, {marginTop: 24}]}>
      <Text style={styles.sectionTitle}>
        Answers ({answers.length})
      </Text>
      {answers.length > 0 && (
        <TouchableOpacity onPress={onClearAnswers}>
          <Text style={styles.clearText}>Clear</Text>
        </TouchableOpacity>
      )}
    </View>
    {answers.length === 0 ? (
      <Text style={styles.emptyText}>
        Tap a topic above to query a nearby device.
      </Text>
    ) : (
      answers.map((a, i) => (
        <View key={`${a.topic}-${a.receivedAt}-${i}`} style={styles.answerCard}>
          <Text style={styles.answerQuestion}>{a.question}</Text>
          <Text style={styles.answerBody}>{a.answer}</Text>
          <Text style={styles.answerSource}>
            via {a.serviceId} @ {a.providerPeerId.slice(0, 12)}...
          </Text>
        </View>
      ))
    )}
  </View>
);

// ---------------------------------------------------------------------------
// Logs Tab
// ---------------------------------------------------------------------------

interface LogsTabProps {
  logs: ActivityLog[];
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
            {log.direction === 'in'
              ? 'IN'
              : log.direction === 'out'
                ? 'OUT'
                : 'SYS'}
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
    fontSize: 22,
    fontWeight: '700',
    color: '#e0e0ff',
  },
  headerSubtitle: {
    fontSize: 12,
    color: '#8888aa',
    marginTop: 2,
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

  // Host tab
  packCard: {
    backgroundColor: '#1a1a2e',
    borderWidth: 1,
    borderColor: '#2a2a4a',
    borderRadius: 10,
    padding: 14,
    marginBottom: 12,
  },
  packHeader: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  packIcon: {
    width: 36,
    height: 36,
    borderRadius: 8,
    backgroundColor: '#2a2a4a',
    alignItems: 'center',
    justifyContent: 'center',
  },
  packIconText: {
    fontSize: 18,
    color: '#e0e0ff',
    fontWeight: '700',
  },
  packName: {
    color: '#e0e0ff',
    fontSize: 15,
    fontWeight: '600',
  },
  packMeta: {
    color: '#8888aa',
    fontSize: 11,
    marginTop: 2,
  },
  topicList: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 6,
    marginTop: 10,
  },
  topicChip: {
    backgroundColor: '#2a2a4a',
    paddingHorizontal: 10,
    paddingVertical: 4,
    borderRadius: 12,
  },
  topicChipText: {
    color: '#c0c0e0',
    fontSize: 12,
  },

  // Search tab - discovered packs
  discoveredCard: {
    backgroundColor: '#1a1a2e',
    borderWidth: 1,
    borderColor: '#2a3a5a',
    borderRadius: 10,
    padding: 14,
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
    fontSize: 11,
    marginTop: 4,
    fontFamily: 'monospace',
  },
  topicButtonRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
    marginTop: 10,
  },
  topicButton: {
    backgroundColor: '#1a3a5c',
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderRadius: 16,
  },
  topicButtonText: {
    color: '#8ec5fc',
    fontSize: 13,
    fontWeight: '500',
  },

  // Answer cards
  answerHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 6,
  },
  answerCard: {
    backgroundColor: '#1a2a1a',
    borderWidth: 1,
    borderColor: '#2d6a4f',
    borderRadius: 10,
    padding: 14,
    marginBottom: 10,
  },
  answerQuestion: {
    color: '#b7e4c7',
    fontSize: 14,
    fontWeight: '600',
    marginBottom: 8,
  },
  answerBody: {
    color: '#d0e0d0',
    fontSize: 13,
    lineHeight: 20,
  },
  answerSource: {
    color: '#5a8a5a',
    fontSize: 11,
    marginTop: 8,
    fontFamily: 'monospace',
  },

  // Shared
  emptyText: {
    color: '#555577',
    fontSize: 13,
    fontStyle: 'italic',
    textAlign: 'center',
    paddingVertical: 20,
  },
  clearText: {
    color: '#6c63ff',
    fontSize: 13,
  },

  // Logs tab
  logHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 8,
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
