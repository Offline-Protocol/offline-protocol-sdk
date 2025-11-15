/**
 * Lightweight port of the Swift/Kotlin MeshController logic used in the SDK.
 * We mirror the scoring function, leader election, cluster capacity checks, and
 * connection intents so the visualization reflects real controller behavior.
 */

const DEFAULT_CONFIG = {
  maxConnectionsPerDevice: 4,
  maxClusterSize: 4,
  leaderReselectionInterval: 30_000,
  leaderDropScoreThreshold: 0.25,
  metadataTTL: 60_000,
  activePeerGrace: 90_000,
};

const MeshRole = {
  LEADER: "LEADER",
  MEMBER: "MEMBER",
  BRIDGE: "BRIDGE",
};

const ConnectionIntent = {
  INTRA_CLUSTER: "INTRA_CLUSTER",
  INTER_CLUSTER: "INTER_CLUSTER",
  REJECTED: "REJECTED",
};

class PeerState {
  constructor(deviceId, role = MeshRole.MEMBER, metrics = randomMetrics(), now = Date.now()) {
    this.deviceId = deviceId;
    this.role = role;
    this.metrics = metrics;
    this.lastUpdated = now;
    this.lastActivity = now;
  }

  score() {
    const { rssi, batteryPercent, signalQuality, stability, hopCount } = this.metrics;
    const rssiScore =
      typeof rssi === "number" ? ((clamp(rssi + 100, -100, -20) + 100) / 80.0) : 0.5;
    const batteryScore = (clamp(batteryPercent ?? 60, 0, 100)) / 100.0;
    const signalScore = (clamp(signalQuality ?? 50, 0, 100)) / 100.0;
    const stabilityScore = clamp(stability ?? 0.5, 0, 1);
    const hopScore = typeof hopCount === "number" ? 1.0 / (1 + hopCount) : 1.0;
    return rssiScore * 0.3 + batteryScore * 0.25 + signalScore * 0.2 + stabilityScore * 0.15 + hopScore * 0.1;
  }
}

class MeshControllerJS {
  constructor(selfId, config = {}) {
    this.selfId = selfId;
    this.config = { ...DEFAULT_CONFIG, ...config };
    this.clusterId = MeshControllerJS.makeClusterId(selfId);
    this.members = new Map();
    this.activeConnections = new Map();
    this.leaderId = selfId;
    this.clusterVersion = 1;
    this.lastElection = Date.now();
    const initial = new PeerState(selfId, MeshRole.LEADER);
    this.members.set(selfId, initial);
  }

  snapshot() {
    const availableSlots = Math.max(this.config.maxClusterSize - this.members.size, 0);
    const leaderScore = this.members.get(this.leaderId)?.score() ?? 0;
    return {
      clusterId: this.clusterId,
      leaderId: this.leaderId,
      members: Array.from(this.members.values()).map((state) => ({
        deviceId: state.deviceId,
        role: state.role,
        metrics: { ...state.metrics },
        score: state.score(),
      })),
      availableSlots,
      leaderScore,
    };
  }

  connectionBudgetLeft() {
    return Math.max(this.config.maxConnectionsPerDevice - this.activeConnections.size, 0);
  }

  shouldAcceptInboundConnection(remoteId) {
    const now = Date.now();
    if (this.activeConnections.size >= this.config.maxConnectionsPerDevice) {
      return { intent: ConnectionIntent.REJECTED, reason: "connection_budget_exhausted" };
    }
    if (this.members.size >= this.config.maxClusterSize) {
      return { intent: ConnectionIntent.INTER_CLUSTER, reason: "cluster_full" };
    }

    if (remoteId && !this.members.has(remoteId)) {
      this.members.set(remoteId, new PeerState(remoteId, MeshRole.MEMBER, randomMetrics(), now));
    }
    if (remoteId) {
      const state = this.members.get(remoteId);
      state.lastActivity = now;
    }

    return { intent: ConnectionIntent.INTRA_CLUSTER, reason: "slot_available" };
  }

  registerConnection(peerId, role) {
    const now = Date.now();
    const state = this.members.get(peerId) ?? new PeerState(peerId, role, randomMetrics(), now);
    state.role = role;
    state.lastUpdated = now;
    state.lastActivity = now;
    this.members.set(peerId, state);
    this.activeConnections.set(peerId, role);
    this.maybeElectLeader("connection_registered");
  }

  updatePeerMetrics(peerId, metrics) {
    const now = Date.now();
    const state = this.members.get(peerId) ?? new PeerState(peerId, MeshRole.MEMBER, metrics, now);
    state.metrics = metrics;
    state.lastUpdated = now;
    state.lastActivity = now;
    this.members.set(peerId, state);
    this.maybeElectLeader("peer_metrics");
  }

  updateSelfMetrics(metrics) {
    this.updatePeerMetrics(this.selfId, metrics);
  }

  registerDisconnection(peerId) {
    this.activeConnections.delete(peerId);
    this.members.delete(peerId);
    if (this.leaderId === peerId) {
      this.leaderId = this.selfId;
    }
    this.maybeElectLeader("disconnect");
  }

  maybeElectLeader(trigger) {
    const now = Date.now();
    if (
      now - this.lastElection < this.config.leaderReselectionInterval &&
      trigger !== "disconnect" &&
      trigger !== "peer_metrics"
    ) {
      return;
    }

    let bestPeer = null;
    let bestScore = -Infinity;
    for (const state of this.members.values()) {
      if (now - state.lastUpdated > this.config.metadataTTL) {
        this.members.delete(state.deviceId);
        this.activeConnections.delete(state.deviceId);
        continue;
      }
      const score = state.score();
      if (score > bestScore || (score === bestScore && state.deviceId < (bestPeer?.deviceId ?? ""))) {
        bestPeer = state;
        bestScore = score;
      }
    }

    if (!bestPeer) {
      this.leaderId = this.selfId;
      this.members.set(this.selfId, new PeerState(this.selfId, MeshRole.LEADER));
      this.clusterVersion += 1;
      this.lastElection = now;
      return;
    }

    const previousLeader = this.leaderId;
    this.leaderId = bestPeer.deviceId;
    for (const state of this.members.values()) {
      state.role = state.deviceId === this.leaderId ? MeshRole.LEADER : MeshRole.MEMBER;
    }

    if (previousLeader !== this.leaderId || bestPeer.score() < this.config.leaderDropScoreThreshold) {
      this.clusterVersion += 1;
      this.lastElection = now;
    }
  }

  static makeClusterId(selfId) {
    return `cluster-${MeshControllerJS.hash32(selfId).toString(16).padStart(8, "0")}`;
  }

  static hash32(input) {
    let hash = 0x811c9dc5;
    for (let i = 0; i < input.length; i += 1) {
      hash ^= input.charCodeAt(i);
      hash = (hash * 0x01000193) >>> 0;
    }
    return hash;
  }
}

class MeshSimulation {
  constructor(config = {}) {
    this.controller = new MeshControllerJS("node-0", config);
    this.nodes = new Map();
    this.remoteClusters = [];
    this.sequence = 0;
    const selfMetrics = randomMetrics();
    this.nodes.set("node-0", {
      id: "node-0",
      status: "leader",
      role: MeshRole.LEADER,
      metrics: selfMetrics,
      reason: "self",
      order: this.sequence,
      clusterType: "local",
    });
    this.controller.updateSelfMetrics(selfMetrics);
    this.nextId = 1;
    this.lastSnapshot = this.controller.snapshot();
  }

  setNodeCount(targetCount) {
    const total = () => this.nodes.size;
    while (total() < targetCount) {
      this.spawnNode();
    }
    while (total() > targetCount && total() > 1) {
      this.removeNode();
    }
    this.syncSnapshot();
  }

  spawnNode() {
    const id = `node-${this.nextId++}`;
    const metrics = randomMetrics();
    this.sequence += 1;
    const order = this.sequence;
    const decision = this.controller.shouldAcceptInboundConnection(id);
    if (decision.intent === ConnectionIntent.INTRA_CLUSTER) {
      const role = MeshRole.MEMBER;
      this.controller.registerConnection(id, role);
      this.controller.updatePeerMetrics(id, metrics);
      this.nodes.set(id, {
        id,
        status: "member",
        role,
        metrics,
        reason: decision.reason,
        order,
        clusterType: "local",
      });
      return;
    }

    this.assignToRemoteCluster(id, metrics, decision.reason);
  }

  removeNode() {
    let candidate = null;
    for (const node of this.nodes.values()) {
      if (node.id === "node-0" && node.clusterType === "local") {
        continue;
      }
      if (!candidate || node.order > candidate.order) {
        candidate = node;
      }
    }
    if (!candidate) return;

    this.nodes.delete(candidate.id);
    if (candidate.clusterType === "remote") {
      this.removeRemoteMember(candidate);
      return;
    }
    if (candidate.status !== "rejected") {
      this.controller.registerDisconnection(candidate.id);
    }
  }

  assignToRemoteCluster(id, metrics, reason) {
    const maxClusterSize = this.controller.config.maxClusterSize;
    let cluster = this.remoteClusters.find((c) => c.nodes.length < maxClusterSize);
    if (!cluster) {
      cluster = {
        id: `remote-${this.remoteClusters.length + 1}`,
        nodes: [],
      };
      this.remoteClusters.push(cluster);
    }
    const isLeader = cluster.nodes.length === 0;
    const node = {
      id,
      status: isLeader ? "leader" : "member",
      role: isLeader ? MeshRole.LEADER : MeshRole.MEMBER,
      metrics,
      reason,
      order: this.sequence,
      clusterType: "remote",
      remoteClusterId: cluster.id,
    };
    cluster.nodes.push(node);
    this.nodes.set(id, node);
  }

  removeRemoteMember(node) {
    const cluster = this.remoteClusters.find((c) => c.id === node.remoteClusterId);
    if (!cluster) {
      return;
    }
    cluster.nodes = cluster.nodes.filter((entry) => entry.id !== node.id);
    if (!cluster.nodes.length) {
      this.remoteClusters = this.remoteClusters.filter((c) => c !== cluster);
      return;
    }
    if (node.status === "leader") {
      const nextLeader = cluster.nodes[0];
      nextLeader.status = "leader";
      nextLeader.role = MeshRole.LEADER;
    }
  }

  tick() {
    for (const node of this.nodes.values()) {
      if (node.status === "rejected") {
        continue;
      }
      node.metrics = jitterMetrics(node.metrics);
      if (node.clusterType === "remote") {
        continue;
      }
      if (node.id === "node-0") {
        this.controller.updateSelfMetrics(node.metrics);
      } else {
        this.controller.updatePeerMetrics(node.id, node.metrics);
      }
    }
    this.syncSnapshot();
  }

  syncSnapshot() {
    const snapshot = this.controller.snapshot();
    const memberMap = new Map(snapshot.members.map((m) => [m.deviceId, m]));
    for (const node of this.nodes.values()) {
      if (node.status === "rejected" || node.clusterType === "remote") {
        continue;
      }
      const state = memberMap.get(node.id);
      if (!state) {
        node.status = "rejected";
        node.role = null;
        node.reason = "evicted";
        continue;
      }
      node.role = state.role;
      if (state.role === MeshRole.LEADER) {
        node.status = "leader";
      } else if (state.role === MeshRole.BRIDGE) {
        node.status = "bridge";
      } else {
        node.status = "member";
      }
    }
    this.lastSnapshot = snapshot;
  }

  getRenderableState() {
    const snapshot = this.lastSnapshot ?? this.controller.snapshot();
    const allNodes = Array.from(this.nodes.values()).map((node) => ({ ...node }));
    const localNodes = allNodes.filter((node) => node.clusterType !== "remote");
    const remoteNodes = allNodes.filter((node) => node.clusterType === "remote");
    const remoteClusters = this.remoteClusters.map((cluster) => ({
      id: cluster.id,
      nodes: cluster.nodes.map((entry) => ({ ...entry })),
    }));
    const localResult = this.buildCompleteMeshEdges(
      localNodes.filter((n) => n.status !== "rejected"),
      "local"
    );
    const localEdges = localResult.edges;
    const localBudgets = localResult.capacities;
    const remoteResults = remoteClusters.map((cluster) => {
      const result = this.buildCompleteMeshEdges(cluster.nodes, "remote");
      const leaderNode = cluster.nodes.find((entry) => entry.status === "leader") ?? cluster.nodes[0];
      return {
        ...cluster,
        edges: result.edges,
        capacities: result.capacities,
        leaderNode,
        connected: false,
      };
    });
    const remoteEdges = remoteResults.flatMap((cluster) => cluster.edges);
    const bridgeEdges = this.buildBridgeEdges(remoteResults, snapshot.leaderId, localBudgets);
    const edges = [...localEdges, ...remoteEdges, ...bridgeEdges];
    return {
      nodes: [...localNodes, ...remoteNodes],
      edges,
      remoteClusters,
      stats: {
        totalNodes: allNodes.length,
        clusterSize: snapshot.members.length,
        leaderId: snapshot.leaderId,
        slots: snapshot.availableSlots,
        activeConnections: this.controller.activeConnections.size,
        rejected: allNodes.filter((n) => n.status === "rejected").length,
        clusterCount: 1 + remoteClusters.length,
      },
    };
  }

  buildCompleteMeshEdges(nodes, type) {
    if (!nodes.length) {
      return { edges: [], capacities: new Map() };
    }
    const maxConnections = this.controller.config.maxConnectionsPerDevice;
    const capacities = new Map(nodes.map((node) => [node.id, maxConnections]));
    const edges = [];
    for (let i = 0; i < nodes.length; i += 1) {
      for (let j = i + 1; j < nodes.length; j += 1) {
        const a = nodes[i];
        const b = nodes[j];
        if ((capacities.get(a.id) ?? 0) <= 0 || (capacities.get(b.id) ?? 0) <= 0) {
          continue;
        }
        edges.push({ from: a.id, to: b.id, type });
        capacities.set(a.id, (capacities.get(a.id) ?? 0) - 1);
        capacities.set(b.id, (capacities.get(b.id) ?? 0) - 1);
      }
    }
    return { edges, capacities };
  }

  buildBridgeEdges(remoteClusters, leaderId, localBudgets) {
    if (!remoteClusters.length) {
      return [];
    }
    const allowLocal = Boolean(leaderId && localBudgets);
    const edges = [];
    for (const cluster of remoteClusters) {
      const leader = cluster.leaderNode;
      if (!leader || !this.hasBudget(cluster.capacities, leader.id)) {
        continue;
      }

      let connected = false;

      if (allowLocal) {
        const target = this.pickBridgeTarget(localBudgets, leaderId);
        if (target && this.hasBudget(localBudgets, target)) {
          edges.push({ from: leader.id, to: target, type: "bridge", remoteClusterId: cluster.id });
          cluster.capacities.set(leader.id, (cluster.capacities.get(leader.id) ?? 0) - 1);
          localBudgets.set(target, (localBudgets.get(target) ?? 0) - 1);
          cluster.connected = true;
          connected = true;
        }
      }

      if (connected) {
        continue;
      }

      const partner = this.pickRemotePartner(remoteClusters, cluster.id);
      if (
        partner &&
        partner.leaderNode &&
        this.hasBudget(partner.capacities, partner.leaderNode.id) &&
        partner.connected
      ) {
        edges.push({
          from: leader.id,
          to: partner.leaderNode.id,
          type: "bridge",
          remoteClusterId: cluster.id,
        });
        cluster.capacities.set(leader.id, (cluster.capacities.get(leader.id) ?? 0) - 1);
        partner.capacities.set(
          partner.leaderNode.id,
          (partner.capacities.get(partner.leaderNode.id) ?? 0) - 1
        );
        cluster.connected = true;
      }
    }
    return edges;
  }

  pickBridgeTarget(localBudgets, preferredId) {
    if ((localBudgets.get(preferredId) ?? 0) > 0) {
      return preferredId;
    }
    for (const [nodeId, budget] of localBudgets.entries()) {
      if (budget > 0) {
        return nodeId;
      }
    }
    return null;
  }

  pickRemotePartner(remoteClusters, excludeClusterId) {
    return remoteClusters.find(
      (cluster) =>
        cluster.id !== excludeClusterId &&
        cluster.connected &&
        cluster.leaderNode &&
        this.hasBudget(cluster.capacities, cluster.leaderNode.id)
    );
  }

  hasBudget(capacities, nodeId) {
    return (capacities?.get(nodeId) ?? 0) > 0;
  }
}

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max);
}

function randomInt(min, max) {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

function randomMetrics() {
  return {
    rssi: randomInt(-95, -35),
    batteryPercent: randomInt(25, 100),
    signalQuality: randomInt(30, 100),
    hopCount: randomInt(0, 3),
    stability: Number(Math.random().toFixed(2)),
  };
}

function jitterMetrics(metrics) {
  return {
    rssi: clamp(metrics.rssi + randomInt(-5, 5), -100, -20),
    batteryPercent: clamp(metrics.batteryPercent + randomInt(-1, 1), 0, 100),
    signalQuality: clamp(metrics.signalQuality + randomInt(-5, 5), 0, 100),
    hopCount: clamp((metrics.hopCount ?? 0) + randomInt(-1, 1), 0, 4),
    stability: clamp((metrics.stability ?? 0.5) + (Math.random() - 0.5) * 0.05, 0, 1),
  };
}

function colorForStatus(status) {
  switch (status) {
    case "leader":
      return "#ffb347";
    case "bridge":
      return "#a569ff";
    case "member":
      return "#4ecdc4";
    default:
      return "#ff6b6b";
  }
}

function runApp() {
  const canvas = document.getElementById("meshCanvas");
  const ctx = canvas.getContext("2d");
  const statsList = document.getElementById("statsList");
  const form = document.getElementById("bootstrapForm");
  const input = document.getElementById("initialNodes");

  const simulation = new MeshSimulation();
  simulation.setNodeCount(Number(input.value));

  function resizeCanvas() {
    canvas.width = canvas.clientWidth;
    canvas.height = canvas.clientHeight;
  }

  function render() {
    const state = simulation.getRenderableState();
    updateStats(statsList, state.stats);
    drawGraph(ctx, state);
  }

  resizeCanvas();
  window.addEventListener("resize", () => {
    resizeCanvas();
    render();
  });

  form.addEventListener("submit", (evt) => {
    evt.preventDefault();
    const desired = clamp(Number(input.value) || 1, 1, 200);
    input.value = desired;
    simulation.setNodeCount(desired);
    render();
  });

  window.addEventListener("keydown", (evt) => {
    if (evt.key === "ArrowUp") {
      simulation.setNodeCount(simulation.nodes.size + 1);
      input.value = simulation.nodes.size;
      render();
    } else if (evt.key === "ArrowDown") {
      const next = Math.max(1, simulation.nodes.size - 1);
      simulation.setNodeCount(next);
      input.value = simulation.nodes.size;
      render();
    }
  });

  setInterval(() => {
    simulation.tick();
    render();
  }, 1500);

  render();
}

function drawGraph(ctx, state) {
  const { width, height } = ctx.canvas;
  ctx.clearRect(0, 0, width, height);
  const centerX = width / 2;
  const centerY = height / 2;
  const positions = new Map();

  const localNodes = state.nodes.filter(
    (n) => n.clusterType !== "remote" && n.status !== "rejected"
  );
  const localRadius = Math.max(Math.min(width, height) / 3, 120);
  const localCount = Math.max(localNodes.length, 1);

  localNodes.forEach((node, index) => {
    const angle = (index / localCount) * Math.PI * 2 - Math.PI / 2;
    const x = centerX + Math.cos(angle) * localRadius;
    const y = centerY + Math.sin(angle) * localRadius;
    positions.set(node.id, { x, y });
  });

  const remoteClusters = state.remoteClusters ?? [];
  const remoteRadius = Math.max(Math.min(width, height) / 2 - 90, localRadius + 80);
  remoteClusters.forEach((cluster, clusterIdx) => {
    const angle = remoteClusters.length
      ? (clusterIdx / remoteClusters.length) * Math.PI * 2 - Math.PI / 2
      : 0;
    const cx = centerX + Math.cos(angle) * remoteRadius;
    const cy = centerY + Math.sin(angle) * remoteRadius;
    const nodes = cluster.nodes;
    const innerRadius = Math.max(50, 30 + nodes.length * 3);
    const nodeCount = Math.max(nodes.length, 1);
    nodes.forEach((node, index) => {
      const nodeAngle = (index / nodeCount) * Math.PI * 2 - Math.PI / 2;
      const x = cx + Math.cos(nodeAngle) * innerRadius;
      const y = cy + Math.sin(nodeAngle) * innerRadius;
      positions.set(node.id, { x, y });
    });
  });

  const rejected = state.nodes.filter((n) => n.status === "rejected");
  rejected.forEach((node, index) => {
    const spacing = 30;
    const startX = centerX - ((rejected.length - 1) * spacing) / 2;
    positions.set(node.id, { x: startX + index * spacing, y: height - 40 });
  });

  state.edges.forEach((edge) => {
    const from = positions.get(edge.from);
    const to = positions.get(edge.to);
    if (!from || !to) return;
    ctx.beginPath();
    if (edge.type === "bridge") {
      ctx.strokeStyle = "#ff6b6b";
      ctx.lineWidth = 1.6;
      ctx.setLineDash([4, 3]);
    } else if (edge.type === "remote") {
      ctx.strokeStyle = "rgba(165,105,255,0.5)";
      ctx.lineWidth = 1;
      ctx.setLineDash([]);
    } else {
      ctx.strokeStyle = "rgba(255,255,255,0.35)";
      ctx.lineWidth = 1.2;
      ctx.setLineDash([]);
    }
    ctx.moveTo(from.x, from.y);
    ctx.lineTo(to.x, to.y);
    ctx.stroke();
    ctx.setLineDash([]);
  });

  for (const node of state.nodes) {
    const pos = positions.get(node.id);
    if (!pos) continue;
    const color = colorForStatus(node.status);
    ctx.beginPath();
    ctx.fillStyle = color;
    ctx.strokeStyle = "rgba(0,0,0,0.4)";
    ctx.lineWidth = node.status === "leader" ? 3 : 1;
    ctx.arc(pos.x, pos.y, node.status === "leader" ? 14 : 10, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();

    ctx.fillStyle = "#0a0c12";
    ctx.font = "10px Inter, sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(node.id.replace("node-", ""), pos.x, pos.y);
  }
}

function updateStats(container, stats) {
  container.querySelector('[data-stat="total-nodes"]').textContent = stats.totalNodes;
  container.querySelector('[data-stat="cluster-size"]').textContent = stats.clusterSize;
  container.querySelector('[data-stat="leader-id"]').textContent = stats.leaderId;
  container.querySelector('[data-stat="slots"]').textContent = stats.slots;
  container.querySelector('[data-stat="connections"]').textContent = stats.activeConnections;
  container.querySelector('[data-stat="rejected"]').textContent = stats.rejected;
  container.querySelector('[data-stat="cluster-count"]').textContent = stats.clusterCount;
}

window.addEventListener("DOMContentLoaded", runApp);



