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
  /** Where worker.js is served, if not beside this module. */
  workerUrl?: string | URL;
  /**
   * The engine module, for a host that cannot serve it beside worker.js:
   * its bytes or a compiled module. By default the worker fetches it.
   */
  wasm?: BufferSource | WebAssembly.Module;
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
 * and the node carried on. `fatal` carries why the node ended (its module
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
  constructor(node: OfflineNode, options?: { iceServers?: RTCIceServer[] });
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
 * recover; the connection closes, and the engine reports the peer lost, only
 * when the browser gives up ("failed": in Chrome 10 s after "disconnected",
 * 15 s after the last answered check). "closed" comes once, at the end.
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
  constructor(node: OfflineNode, options: { relays: string[] });
  newRoom(): Promise<string>;
  /** Answers every device that joins `room`; resolves once listening. */
  host(
    room: string,
    carrier: WebRtcCarrier,
    options?: { onConnection?(connection: SignalledConnection): void },
  ): Promise<{ room: string; close(): void }>;
  /** Joins the device hosting `room`; rejects when no answer comes in time. */
  join(room: string, carrier: WebRtcCarrier, options?: { timeoutMs?: number }): Promise<SignalledConnection>;
}
