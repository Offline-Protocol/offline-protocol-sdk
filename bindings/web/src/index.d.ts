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
  sendMessage(to: string, content: string): Promise<string>;
  createGroup(name: string): Promise<{ group_id: string; [field: string]: unknown }>;
  inviteToGroup(groupId: string, member: string): Promise<void>;
  sendGroupMessage(groupId: string, content: string): Promise<string[]>;
  /** The durable logs' costs, or null for a node in memory. */
  stats(): Promise<{ mls: LogStats; protocol_state: LogStats } | null>;
  /** Registers a carrier the page opened; resolves to its link id. */
  openLink(carrier: PageCarrier): Promise<number>;
  /** Bytes the carrier delivered; resolves to the reason it was closed, if it was. */
  linkReceived(link: number, bytes: Uint8Array): Promise<string | undefined>;
  /** The carrier closed. */
  closeLink(link: number): Promise<void>;
  stop(): Promise<void>;
  /** Ends the worker at once, as a crash would. */
  terminate(): void;
}

/**
 * WebRTC links between devices on one local network, set up with two codes
 * and no server: the offering side shows a code and reads the answer back.
 */
export class WebRtcCarrier {
  constructor(node: OfflineNode);
  offer(): Promise<{ code: string; accept(answerCode: string): Promise<void>; close(): void }>;
  answer(offerCode: string): Promise<{ code: string; close(): void }>;
  close(): void;
}
