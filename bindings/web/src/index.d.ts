/** An engine event: the engine's JSON, tagged by `type`. */
export interface EngineEvent {
  type: string;
  [field: string]: unknown;
}

export interface StartOptions {
  /** Scopes the node's traffic, its storage and the tab carrier's channel. */
  appId: string;
  /** The local account name. Not the identity, which the node mints. */
  profile?: string;
  /**
   * "durable" (the default): the identity and groups live in sealed
   * origin-private files and outlive the worker. One node per account per
   * origin; a second is refused. "memory": a new device every start.
   */
  storage?: "durable" | "memory";
  /** Link to this origin's other tabs over a BroadcastChannel. Default true. */
  tabs?: boolean;
  /** The Nostr transport over relays the application names (W12). */
  nostr?: NostrOptions;
  /** The Bluetooth LE slot, driven by a BluetoothCarrier on this page (W14). */
  ble?: boolean;
  /** Where worker.js is served, if not beside this module. */
  workerUrl?: string | URL;
  /**
   * The engine module, for a host that cannot serve it beside worker.js:
   * its bytes or a compiled module. By default the worker fetches it.
   */
  wasm?: BufferSource | WebAssembly.Module;
  /**
   * Aborts a start that has not finished: its worker ends, and with it the
   * account's lock or its place in the queue for one (W8). The start rejects
   * with the signal's reason. No effect once the start has resolved.
   */
  signal?: AbortSignal;
}

export interface NostrOptions {
  /**
   * The relays to use, as ws:// or wss:// URLs. Required: the SDK names no
   * relay of its own.
   */
  relays: string[];
  /**
   * Publish key packages so a device that knows only this node's address
   * can reach it, and fetch others' (cold first contact). Default true.
   */
  coldContact?: boolean;
  /**
   * Publish `profile` as a username in the relays' directory, and resolve
   * others'. Default false: it binds a readable name to this address in
   * public. Needs `coldContact`.
   */
  usernameDiscovery?: boolean;
}

/** Where `establishSecureSession` left a session. */
export type SessionStart = "created" | "existing" | "pending";

/** An invite another device shared, verified. */
export interface InviteInfo {
  /** The address, checked to derive from `publicKey`. */
  address: string;
  /** The Ed25519 identity key, base64. */
  publicKey: string;
  /** A suggested name; the person may change it. */
  petname: string | null;
  /** Whether a signature binds the petname to the key. */
  signed: boolean;
}

/** A group as this device holds it. */
export interface GroupInfo {
  group_id: string;
  name: string | null;
  members: string[];
  members_count: number;
  /** Advances on every membership change; a removed member never holds the next one's keys. */
  epoch: number;
  [field: string]: unknown;
}

export interface SendOptions {
  /**
   * Send once over the live link and never retry (W10). For state the next
   * send replaces; never `message_delivered` or `message_failed`.
   */
  ephemeral?: boolean;
}

/** What one durable log has cost since the node started. */
export interface LogStats {
  flushes: number;
  log_bytes: number;
  live_bytes: number;
  compactions: number;
}

/** A carrier the page owns, toward one other node. */
export interface PageCarrier {
  /** Puts bytes on the carrier, in order. */
  send(bytes: Uint8Array): void;
  /** Closes the carrier; the other end must see it close. */
  close(): void;
}

/**
 * A node running in its own worker. Every engine event is dispatched twice:
 * as a DOM event named by its `type` and as `event`, with the JSON as `detail`.
 *
 * Two more events are the binding's own. `error` carries a tick that failed
 * and the node carried on, or a Nostr relay the node cannot use (one the
 * browser will not dial, or one that closed this device's subscription). `fatal` carries why the node ended (its module
 * trapped, its worker failed, or it was terminated): every call in flight and
 * every later one rejects. Starting the node again with the same `appId` and
 * `profile` is the same device when its storage is durable.
 */
export class OfflineNode extends EventTarget {
  /** This device's `off1…` address. */
  readonly address: string;
  static start(options: StartOptions): Promise<OfflineNode>;
  /**
   * `ephemeral` sends once over the live link, never acknowledged or
   * retried, for state the next send replaces. Rejects at once with no link
   * or no session.
   */
  sendMessage(to: string, content: string, options?: SendOptions): Promise<string>;
  /**
   * Starts a session with a device. "pending" means its key package has not
   * arrived; with relays, the engine has asked them for it, and
   * `secure_session_established` follows.
   */
  establishSecureSession(address: string): Promise<SessionStart>;
  /**
   * Starts a directory lookup; the answer is one `username_resolved` event
   * with every claim. Needs `nostr.usernameDiscovery`.
   */
  resolveUsername(username: string): Promise<boolean>;
  /** An invite naming this device, base64url. */
  createInvite(options?: { petname?: string; sign?: boolean }): Promise<string>;
  /** Opens and verifies an invite; rejects one that does not verify. */
  parseInvite(blob: string): Promise<InviteInfo>;
  createGroup(name: string): Promise<{ group_id: string; [field: string]: unknown }>;
  inviteToGroup(groupId: string, member: string): Promise<void>;
  /**
   * Removes a member from a group this device administers. The others move
   * to a new epoch; the removed member gets `group_member_removed`.
   */
  removeFromGroup(groupId: string, member: string): Promise<void>;
  /** The group's members and epoch, or null for a group this device does not hold. */
  groupInfo(groupId: string): Promise<GroupInfo | null>;
  /** `ephemeral` as for sendMessage, to every member holding a live link. */
  sendGroupMessage(groupId: string, content: string, options?: SendOptions): Promise<string[]>;
  /** Registers a service this device offers. */
  registerService(
    serviceId: string,
    options?: { version?: string; capabilities?: Record<string, string> },
  ): Promise<void>;
  /** Unregisters a service; every subscription on it ends. */
  unregisterService(serviceId: string): Promise<boolean>;
  /**
   * Subscribes to a provider's topic (ADR 0035); resolves to the subscription
   * id. Every subscription frame is sealed in the 1:1 session.
   *
   * `topic` may be a pattern, `"prefix*"` or `"*"`: every matching topic in
   * the mode, up to 64, each update naming its own. `keys` limits a `latest`
   * subscription to those keys, each exact or a prefix ending in `*`; the
   * provider applies it.
   */
  subscribeService(
    provider: string,
    serviceId: string,
    topic: string,
    options?: { mode?: SubscriptionMode; params?: string; keys?: string[] },
  ): Promise<string>;
  unsubscribeService(subId: string): Promise<boolean>;
  /** Asks the provider for the whole state again, now. */
  resyncServiceSubscription(subId: string): Promise<void>;
  serviceSubscriptions(): Promise<ServiceSubscriptionInfo[]>;
  /** Who may subscribe to a service; `restricted` asks this page. */
  setServiceSubscriptionPolicy(serviceId: string, policy: SubscriptionPolicy): Promise<void>;
  acceptServiceSubscription(subId: string): Promise<void>;
  rejectServiceSubscription(subId: string, reason?: string): Promise<void>;
  endServiceSubscription(subId: string, reason?: string): Promise<void>;
  /** With a `key` the topic is `latest`; without one it is `log`. */
  publishServiceUpdate(
    serviceId: string,
    topic: string,
    body: string,
    options?: { key?: string | null },
  ): Promise<ServicePublishReport>;
  setServiceTopicCheckpoint(serviceId: string, topic: string, body: string): Promise<void>;
  clearServiceTopic(serviceId: string, topic: string): Promise<void>;
  serviceSubscribers(serviceId?: string | null): Promise<ServiceSubscriberInfo[]>;
  /** The durable logs' costs, or null for a node in memory. */
  stats(): Promise<{ mls: LogStats; protocol_state: LogStats } | null>;
  /** A fresh NostrSignaller room code: eighty bits a person can read out. */
  newRoom(): Promise<string>;
  /** NostrSignaller's half in the worker: the relay REQ text for a room. */
  signalSubscription(room: string, subscriptionId: string): Promise<string>;
  /** A sealed, signed relay EVENT text carrying one connection code. */
  signalEvent(room: string, role: "offer" | "answer", session: string, code: string): Promise<string>;
  /** The signal a relay message carries for the room, or null. */
  signalOpen(
    room: string,
    message: string,
  ): Promise<{ role: "offer" | "answer"; session: string; code: string; freshUntilMs: number } | null>;
  /** NearbyRoom's half in the worker (W13). */
  nearbyOpen(network: string, label: string): Promise<{ id: number; key: string }>;
  nearbySubscription(id: number, subscriptionId: string): Promise<string>;
  nearbyPresence(id: number): Promise<string>;
  nearbyOffer(id: number, to: string, session: string, code: string): Promise<string>;
  nearbyAnswer(id: number, to: string, session: string, code: string): Promise<string>;
  nearbyOpenMessage(
    id: number,
    message: string,
  ): Promise<{
    kind: "presence" | "offer" | "answer";
    peer: string;
    label: string;
    session: string | null;
    code: string | null;
    createdAtMs: number;
    freshUntilMs: number;
  } | null>;
  nearbyClose(id: number): Promise<void>;
  /** BluetoothCarrier's half in the worker (W14). */
  bleHello(): Promise<Uint8Array>;
  bleAppTag(): Promise<Uint8Array>;
  bleVerify(deviceId: Uint8Array, identity: Uint8Array): Promise<string>;
  bleLinkUp(peer: string, maxPayload: number): Promise<void>;
  bleFragmentReceived(peer: string, bytes: Uint8Array): Promise<void>;
  bleLinkDown(peer: string): Promise<void>;
  /**
   * Where the engine's outbound Bluetooth fragments go; BluetoothCarrier sets
   * it. `close` runs once when the node stops or fails. Throws when another
   * carrier holds it; `null` gives it back.
   */
  setBluetoothSink(sink: { write(peer: string, bytes: Uint8Array): void; close(): void } | null): void;
  /** Registers a carrier the page opened; resolves to its link id. */
  openLink(carrier: PageCarrier): Promise<number>;
  /** Bytes the carrier delivered; resolves to the reason it was closed, if it was. */
  linkReceived(link: number, bytes: Uint8Array): Promise<string | undefined>;
  /** The carrier closed. */
  closeLink(link: number): Promise<void>;
  /** Stops the node; every later call rejects. */
  stop(): Promise<void>;
  /** Ends the worker at once, as a crash would. */
  terminate(): void;
}

/**
 * WebRTC links between devices, set up with two codes: the offering side
 * shows a code and reads the answer back, by hand or through a
 * NostrSignaller. With no ICE servers (the default) links stay on the local
 * network; `iceServers` names STUN and TURN servers for networks where
 * devices cannot reach each other directly.
 */
export class WebRtcCarrier extends EventTarget {
  /**
   * `disconnectedMs` (default 15000) is how long a connection may stay
   * "disconnected" before the carrier closes it and the engine reports the
   * peer lost. `Infinity` leaves it to the browser, which may never decide.
   */
  constructor(node: OfflineNode, options?: { iceServers?: RTCIceServer[]; disconnectedMs?: number });
  offer(): Promise<OfferedConnection>;
  answer(offerCode: string): Promise<AnsweredConnection>;
  close(): void;
}

/** What `offer()` resolves to: show `code`, then pass the answer to `accept`. */
export interface OfferedConnection {
  code: string;
  accept(answerCode: string): Promise<void>;
  close(): void;
}

/** What `answer()` resolves to: show `code` to the device that offered. */
export interface AnsweredConnection {
  code: string;
  close(): void;
}

/**
 * The detail of a WebRtcCarrier `connection` event, one per change of state.
 * "disconnected" means the last connectivity checks went unanswered and may
 * recover; the connection closes, and the engine reports the peer lost, when
 * the browser gives up ("failed": in Chrome 10 s after "disconnected", 15 s
 * after the last answered check) or when it has stayed "disconnected" for
 * the carrier's `disconnectedMs`, whichever comes first. "closed" comes
 * once, at the end.
 * Events start once `offer()` or `answer()` has resolved; a call that
 * rejects, including one whose connection closed while it gathered, reports
 * nothing.
 */
export interface ConnectionStateDetail {
  state: "connected" | "disconnected" | "closed";
  connection: OfferedConnection | AnsweredConnection;
}

/** A connection one side of a NostrSignaller room made. */
export interface SignalledConnection {
  close(): void;
}

/**
 * WebRTC signalling through Nostr relays by room code (W11). The room code is
 * a shared secret, not an identity: who is on a link is what the engine's
 * session proves.
 */
export class NostrSignaller {
  /**
   * `keepAliveMs` (default 60000) is how often a hosted room asks each relay
   * to confirm its subscription, dropping and redialling one that does not.
   * A relay has the lesser of ten seconds and `keepAliveMs` to confirm.
   */
  constructor(node: OfflineNode, options: { relays: string[]; keepAliveMs?: number });
  newRoom(): Promise<string>;
  /**
   * Answers every device that joins `room`; resolves once listening. Until
   * it is closed the room redials a relay that drops, with backoff, and
   * probes the live ones, at once when the page comes back online or into
   * view, so it outlasts a device that slept or changed networks. A room
   * whose node stops closes itself.
   */
  host(
    room: string,
    carrier: WebRtcCarrier,
    options?: { onConnection?(connection: SignalledConnection): void },
  ): Promise<{ room: string; close(): void }>;
  /** Joins the device hosting `room`; rejects when no answer comes in time. */
  join(room: string, carrier: WebRtcCarrier, options?: { timeoutMs?: number }): Promise<SignalledConnection>;
}

/** A device announced in a nearby room. */
export interface NearbyPeer {
  /** Its key in this room; what `connect` takes. Not an identity. */
  peer: string;
  /** The label its application chose. A claim, not a name anyone checked. */
  label: string;
  /** When this device last heard it, in milliseconds since the epoch. */
  seenAt: number;
}

export interface NearbyRoomOptions {
  /** The relays to use, as ws:// or wss:// URLs. Required: the SDK names none. */
  relays: string[];
  /**
   * Servers to learn the network's public address from, with at least one
   * STUN server. Required unless `network` is given. The SDK names none.
   */
  iceServers?: RTCIceServer[];
  /** The network's public address, when the application already knows it. */
  network?: string;
  /** What this device is shown as to the others. At most 64 characters. */
  label?: string;
  /**
   * Decides every offer this device receives. Anyone who can guess the
   * network's address can offer, so ask the person; only `true` answers.
   * `signal` aborts when the offer stops being fresh or the room closes:
   * take the question down then, as nobody is waiting for the answer. A
   * session is asked about once; its repeats are not asked again.
   */
  admit(offer: { peer: string; label: string; signal: AbortSignal }): boolean | Promise<boolean>;
}

/**
 * The devices behind this network's public address for one application,
 * found through Nostr relays (W13). Each one announces a room key and a
 * label, sees the others in `peers`, and links over WebRTC to the one its
 * person picks. The room is weak by construction: anyone who can guess the
 * address can read who is present and offer, and none can read a
 * connection code, which is sealed to one device's key. A `peers` event
 * carries the list whenever it changes. A `full` event carries `{ peer,
 * label }` for an offer dropped because sixteen were already waiting on
 * `admit`.
 */
export class NearbyRoom extends EventTarget {
  constructor(node: OfflineNode, carrier: WebRtcCarrier, options: NearbyRoomOptions);
  /** The devices present now, newest first. */
  readonly peers: NearbyPeer[];
  /** This device's key in the room, once open. */
  readonly key: string | null;
  /** Learns the address, subscribes, and starts announcing. A room opens once. */
  open(): Promise<NearbyRoom>;
  /** Links to `peer` once it admits this device; rejects when it does not answer. */
  connect(peer: string, options?: { timeoutMs?: number }): Promise<SignalledConnection>;
  /** Stops announcing and answering; links already made stay up. */
  close(): void;
}

/**
 * The network's public address, from a server-reflexive candidate: IPv4 when
 * one arrives, else IPv6 after waiting `settleMs` for an IPv4 one.
 */
export function publicAddress(
  iceServers: RTCIceServer[],
  options?: { timeoutMs?: number; settleMs?: number },
): Promise<string>;

/**
 * A Bluetooth LE carrier over Web Bluetooth (W14): this page as a GATT
 * central to phones and boards running the SDK as a peripheral. Chromium
 * only. The page is never discoverable: every link starts from a person
 * picking a device in the browser's chooser, inside a user gesture, or from a
 * device this origin was granted before. Start the node with `ble: true`.
 * A `connection` event carries `{ peer, state: "connected" | "closed" }`.
 */
export class BluetoothCarrier extends EventTarget {
  /**
   * `bluetooth` defaults to `navigator.bluetooth`. One carrier per node:
   * throws while another carrier on `node` has not been released.
   */
  constructor(node: OfflineNode, options?: { bluetooth?: unknown });
  /** Whether this browser exposes Web Bluetooth at all. */
  static readonly supported: boolean;
  /** The addresses linked now. */
  readonly peers: string[];
  /** Opens the chooser and links the device picked; call inside a user gesture. */
  request(): Promise<string>;
  /** Devices this origin was granted before, where the browser remembers them. */
  known(): Promise<unknown[]>;
  /**
   * Links a device; resolves to the address its identity proves. Rejects for
   * a device that does not verify, serves no Hello, refuses the hello, or
   * disconnects before the link is up.
   */
  connect(device: unknown): Promise<string>;
  /** Closes the link to `peer` and reports it lost at once. */
  close(peer: string): void;
  closeAll(): void;
  /** Closes every link and gives the node's Bluetooth back to another carrier. */
  release(): void;
}

/** How a topic treats its updates: state per key, or every event in order. */
export type SubscriptionMode = "latest" | "log";

/** Who may subscribe to a service. */
export type SubscriptionPolicy =
  | { kind: "restricted" }
  | { kind: "accept_all" }
  | { kind: "rules"; allowPeers?: string[]; allowGroups?: string[]; fallback?: "ask" | "reject" };

/** What one publish did, per subscriber. */
export interface ServicePublishReport {
  subscribers: number;
  sentLive: number;
  queued: number;
  coalesced: number;
  failed: number;
}

/** A subscription this device holds on another. */
export interface ServiceSubscriptionInfo {
  subId: string;
  provider: string;
  serviceId: string;
  topic: string;
  mode: SubscriptionMode;
  state: "pending" | "active" | "paused" | "ended";
  /** The `latest` keys asked for; empty for every key. */
  keys: string[];
  /** The topics heard of: the topic itself, or each one a pattern matched. */
  topics: string[];
}

/** A subscription another device holds on this one. */
export interface ServiceSubscriberInfo {
  subId: string;
  subscriber: string;
  serviceId: string;
  topic: string;
  mode: SubscriptionMode;
  state: "pending" | "active";
  params: string;
  /** The `latest` keys the subscriber asked for; empty for every key. */
  keys: string[];
}
