/** An engine event: the engine's JSON, tagged by `type`. */
export interface EngineEvent {
  type: string;
  [field: string]: unknown;
}

export interface StartOptions {
  /** Scopes the node's traffic and the tab carrier's channel. */
  appId: string;
  /** The local account name. Not the identity, which the node mints. */
  profile?: string;
  /** Where worker.js is served, if not beside this module. */
  workerUrl?: string | URL;
}

/**
 * A node running in its own worker. Every engine event is dispatched twice:
 * as a DOM event named by its `type` and as `event`, with the JSON as `detail`.
 */
export class OfflineNode extends EventTarget {
  /** This device's `off1…` address. */
  readonly address: string;
  static start(options: StartOptions): Promise<OfflineNode>;
  sendMessage(to: string, content: string): Promise<string>;
  createGroup(name: string): Promise<{ group_id: string; [field: string]: unknown }>;
  inviteToGroup(groupId: string, member: string): Promise<void>;
  sendGroupMessage(groupId: string, content: string): Promise<string[]>;
  stop(): Promise<void>;
}
