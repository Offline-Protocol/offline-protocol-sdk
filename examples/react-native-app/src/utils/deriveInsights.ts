import type {
  MessageDeliveredEvent,
  MessageFailedEvent,
  MessageReceivedEvent,
  MessageSentEvent,
  NetworkMetricsEvent,
  NeighborDiscoveredEvent,
  NeighborLostEvent,
  ProtocolEvent,
  TransportSwitchedEvent,
} from '@offline-protocol/mesh-sdk';

type TransportKey = string;

export interface TransportHealth {
  delivered: number;
  received: number;
  failed: number;
  lastSeenAt: number | null;
}

export interface DorsSwitch {
  at: number;
  from: string | null;
  to: string;
  reason: string;
}

export interface MessageMetrics {
  sent: number;
  received: number;
  delivered: number;
  failed: number;
  pending: number;
  successRate: number | null;
  averageLatencyMs: number | null;
  averageHopCount: number | null;
  lastSentAt: number | null;
  lastReceivedAt: number | null;
}

export interface DorsMetrics {
  currentTransport: string | null;
  lastSwitch: DorsSwitch | null;
  switches: DorsSwitch[];
  transportHealth: Record<TransportKey, TransportHealth>;
}

export interface NeighborMetrics {
  peers: string[];
  total: number;
  lastChangeAt: number | null;
}

export interface NetworkSummary {
  neighborCount: number | null;
  relayCount: number | null;
  deliveryRatio: number | null;
  avgLatencyMs: number | null;
  lastReportedAt: number | null;
}

export interface DerivedInsights {
  messageMetrics: MessageMetrics;
  dorsMetrics: DorsMetrics;
  neighborMetrics: NeighborMetrics;
  networkSummary: NetworkSummary;
}

const EMPTY_TRANSPORT_HEALTH: TransportHealth = {
  delivered: 0,
  received: 0,
  failed: 0,
  lastSeenAt: null,
};

const DEFAULT_INSIGHTS: DerivedInsights = {
  messageMetrics: {
    sent: 0,
    received: 0,
    delivered: 0,
    failed: 0,
    pending: 0,
    successRate: null,
    averageLatencyMs: null,
    averageHopCount: null,
    lastSentAt: null,
    lastReceivedAt: null,
  },
  dorsMetrics: {
    currentTransport: null,
    lastSwitch: null,
    switches: [],
    transportHealth: {},
  },
  neighborMetrics: {
    peers: [],
    total: 0,
    lastChangeAt: null,
  },
  networkSummary: {
    neighborCount: null,
    relayCount: null,
    deliveryRatio: null,
    avgLatencyMs: null,
    lastReportedAt: null,
  },
};

function ensureTransportHealth(
  store: Map<TransportKey, TransportHealth>,
  key: TransportKey,
): TransportHealth {
  if (!store.has(key)) {
    store.set(key, { ...EMPTY_TRANSPORT_HEALTH });
  }
  return store.get(key)!;
}

function annotateTransportHealth(
  store: Map<TransportKey, TransportHealth>,
  transport: string | undefined,
  at: number,
  field: keyof Omit<TransportHealth, 'lastSeenAt'>,
) {
  if (!transport) {
    return;
  }
  const health = ensureTransportHealth(store, transport);
  health[field] += 1;
  health.lastSeenAt = at;
}

export function deriveInsights(events: ProtocolEvent[]): DerivedInsights {
  if (!events.length) {
    return DEFAULT_INSIGHTS;
  }

  const pendingAckIds = new Set<string>();
  const deliveredLatencies: number[] = [];
  const allHopCounts: number[] = [];
  const transportHealth = new Map<TransportKey, TransportHealth>();
  const neighborIds = new Set<string>();
  const switches: DorsSwitch[] = [];

  const metrics: MessageMetrics = {
    sent: 0,
    received: 0,
    delivered: 0,
    failed: 0,
    pending: 0,
    successRate: null,
    averageLatencyMs: null,
    averageHopCount: null,
    lastSentAt: null,
    lastReceivedAt: null,
  };

  const networkSummary: NetworkSummary = {
    neighborCount: null,
    relayCount: null,
    deliveryRatio: null,
    avgLatencyMs: null,
    lastReportedAt: null,
  };

  let currentTransport: string | null = null;
  let lastSwitch: DorsSwitch | null = null;
  let lastNeighborChangeAt: number | null = null;

  // Iterate from earliest to latest to build consistent state
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const event = events[i];
    const seenAt = event.seenAt ?? Date.now();

    switch (event.type) {
      case 'message_sent': {
        const e = event as MessageSentEvent;
        metrics.sent += 1;
        metrics.lastSentAt = seenAt;
        if (e.requires_ack) {
          pendingAckIds.add(e.message_id);
        }
        break;
      }
      case 'message_received': {
        const e = event as MessageReceivedEvent;
        metrics.received += 1;
        metrics.lastReceivedAt = seenAt;
        allHopCounts.push(e.hop_count);
        annotateTransportHealth(transportHealth, e.transport, seenAt, 'received');
        break;
      }
      case 'message_delivered': {
        const e = event as MessageDeliveredEvent;
        metrics.delivered += 1;
        pendingAckIds.delete(e.message_id);
        deliveredLatencies.push(e.latency_ms);
        allHopCounts.push(e.hop_count);
        annotateTransportHealth(transportHealth, e.transport, seenAt, 'delivered');
        break;
      }
      case 'message_failed': {
        const e = event as MessageFailedEvent;
        metrics.failed += 1;
        pendingAckIds.delete(e.message_id);
        // Message failed events do not report transport, so we only count the failure globally.
        break;
      }
      case 'transport_switched': {
        const e = event as TransportSwitchedEvent;
        const switchRecord: DorsSwitch = {
          at: seenAt,
          from: e.from ?? null,
          to: e.to,
          reason: e.reason,
        };
        switches.push(switchRecord);
        currentTransport = e.to;
        lastSwitch = switchRecord;
        break;
      }
      case 'neighbor_discovered': {
        const e = event as NeighborDiscoveredEvent;
        neighborIds.add(e.peer_id);
        lastNeighborChangeAt = seenAt;
        if (!currentTransport && e.transport) {
          currentTransport = e.transport;
        }
        break;
      }
      case 'neighbor_lost': {
        const e = event as NeighborLostEvent;
        neighborIds.delete(e.peer_id);
        lastNeighborChangeAt = seenAt;
        break;
      }
      case 'network_metrics': {
        const e = event as NetworkMetricsEvent;
        networkSummary.neighborCount = e.neighbor_count;
        networkSummary.relayCount = e.relay_count;
        networkSummary.deliveryRatio = e.delivery_ratio;
        networkSummary.avgLatencyMs = e.avg_latency_ms;
        networkSummary.lastReportedAt = seenAt;
        break;
      }
      default:
        break;
    }
  }

  metrics.pending = pendingAckIds.size;
  if (metrics.delivered + metrics.failed > 0) {
    metrics.successRate = metrics.delivered / (metrics.delivered + metrics.failed);
  }
  if (deliveredLatencies.length) {
    const totalLatency = deliveredLatencies.reduce((acc, value) => acc + value, 0);
    metrics.averageLatencyMs = totalLatency / deliveredLatencies.length;
  }
  if (allHopCounts.length) {
    const hopSum = allHopCounts.reduce((acc, value) => acc + value, 0);
    metrics.averageHopCount = hopSum / allHopCounts.length;
  }

  const dorsMetrics: DorsMetrics = {
    currentTransport,
    lastSwitch,
    switches: switches.sort((a, b) => a.at - b.at).slice(-20),
    transportHealth: Object.fromEntries(transportHealth.entries()),
  };

  const neighborMetrics: NeighborMetrics = {
    peers: Array.from(neighborIds).sort(),
    total: neighborIds.size,
    lastChangeAt: lastNeighborChangeAt,
  };

  return {
    messageMetrics: metrics,
    dorsMetrics,
    neighborMetrics,
    networkSummary,
  };
}

