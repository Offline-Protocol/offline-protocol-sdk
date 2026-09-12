/**
 * Constants used throughout the Offline Protocol React Native bindings.
 */

/**
 * Error message shown when the native module is not properly linked.
 */
export const LINKING_ERROR =
  `The package '@offline-protocol/mesh-sdk' doesn't seem to be linked. Make sure: \n\n` +
  '- You have run pod install (iOS)\n' +
  '- You rebuilt the app after installing the package\n' +
  '- You are not using Expo Go\n' +
  '- Your Podfile has no leftover manual "pod \'MeshSdk\'" line (iOS autolinking\n' +
  '  handles it as of 0.20.0; a stale line pointing into ios/ makes pod install\n' +
  '  fail outright, and one pointing elsewhere can shadow the autolinked pod)\n';

/**
 * Default delay in milliseconds before refreshing runtime state after protocol start.
 * This gives the protocol time to fully initialize before querying state.
 */
export const PROTOCOL_START_DELAY_MS = 100;

/**
 * Maximum number of events to keep in memory for the event history.
 * Older events are automatically removed to prevent unbounded memory growth.
 */
export const MAX_EVENT_HISTORY = 200;

/**
 * The *one-shot* event tags: events that report a state change nothing else
 * will ever restate.
 *
 * `mesh_stopped_by_user` is emitted after the transports, the scheduler and
 * the core are already down, so there is no later event to carry the same news
 * and nothing left to ask. `internet_session_superseded` is the same shape for
 * the relay: the transport latches stopped and refuses every reconnect path
 * until an explicit re-enable, so a lost emit means the app never learns it is
 * connected elsewhere. Every other event on this bridge is periodic,
 * re-derivable, or followed by another carrying the same state, and dropping
 * one is correct.
 *
 * Membership is a decision, not a filter: an event only belongs here when
 * redelivering it late is *better* than losing it. A held periodic event
 * (`internet_status_changed`, say) replayed after the fact would report a link
 * state that has since changed — worse than the drop it replaced.
 *
 * These same two tags are enrolled in the native Android sticky buffer, which
 * holds them across a *native→JS* gap; this set drives the JS-side hold that
 * covers the *JS→app-listener* gap (see `OfflineProtocol.on`). The tags must
 * agree across all four definitions — this one,
 * `OfflineProtocolModule.EVENT_MESH_STOPPED_BY_USER` (Kotlin), and
 * `SupersededLatchPolicy.EVENT_TYPE` (Kotlin and Swift) — or half the
 * mechanism silently stops working while everything still compiles. A Rust
 * guard (`react_native_one_shot_event_set_matches_native` in
 * `crates/offline-protocol-uniffi`) pins them together.
 */
export const ONE_SHOT_EVENT_TYPES = [
  'internet_session_superseded',
  'mesh_stopped_by_user',
] as const;

/**
 * The *inbound* event tags: events that each report one message the core has
 * already taken responsibility for.
 *
 * By the time `message_received` (or `file_received`, or the
 * `message_decryption_failed` that stands in for one) is emitted, the core
 * has sent the delivery ACK, dedup-marked the id and dropped its queued copy
 * — the sender will not resend, and nothing in the SDK will restate the
 * event. A drop between the core and the app's handler is therefore a lost
 * message, and the drop is ordinary: the app is backgrounded and the React
 * instance is down, or a push injection delivers the ciphertext before the
 * app has registered `on('message_received')`. Unlike the one-shot set these
 * do not collapse per type — every message is its own fact — so the native
 * buffers key them `type:message_id` and the JS-side hold is a FIFO, both
 * capped at 256 entries.
 *
 * The same three tags are enrolled in the native buffers on both platforms
 * (`OfflineProtocolModule.BUFFERED_INBOUND_EVENT_TYPES` in Kotlin,
 * `InboundEventBuffer.bufferedEventTypes` in Swift), which hold them across
 * the *native→JS* gap; this set drives the JS-side hold that covers the
 * *JS→app-listener* gap (see `OfflineProtocol.on`). A Rust guard
 * (`react_native_buffered_inbound_event_set_matches_native` in
 * `crates/offline-protocol-uniffi`) pins the three definitions together.
 */
export const BUFFERED_INBOUND_EVENT_TYPES = [
  'message_received',
  'file_received',
  'message_decryption_failed',
] as const;

/** Most inbound events the JS-side hold keeps; the oldest is dropped past it. */
export const MAX_PENDING_INBOUND_EVENTS = 256;

/**
 * The Headless JS task key the Android keep-alive uses to wake JavaScript after
 * a process kill (Android only; see `registerMeshWakeTask`).
 *
 * Must match `MeshWakePolicy.TASK_KEY` in the Kotlin bindings. A drift fails
 * silently and in the worst possible way — React Native logs "No task
 * registered for key" to the device log, the app sees an opt-in that does
 * nothing, and both sides still compile — so it is pinned by a Rust guard
 * (`react_native_mesh_wake_wiring_is_present` in
 * `crates/offline-protocol-uniffi`).
 */
export const MESH_WAKE_TASK_KEY = 'OfflineProtocolMeshWake';

