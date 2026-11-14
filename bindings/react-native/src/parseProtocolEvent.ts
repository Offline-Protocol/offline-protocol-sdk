/**
 * Runtime helpers for opaque engine events (bridge contract C3).
 *
 * Events still cross the native boundary as JSON strings; these functions parse
 * and validate the `type` discriminant against the catalog in `types.ts`. They
 * do not replace `OfflineProtocol.on(...)` — use them when you handle raw
 * `eventJson` (custom transports, logging, tests).
 */

import type { EventType, ProtocolEvent } from './types';

/**
 * Every `ProtocolEvent` discriminant declared in `types.ts`.
 *
 * Kept as one explicit list so Rust guards can pin it against the interfaces
 * without running TypeScript. When you add an event interface, append its
 * `type` here in the same change.
 */
export const PROTOCOL_EVENT_TYPES = [
  'ack_evicted',
  'connection_accepted',
  'connection_rejected',
  'connection_request_cancelled',
  'connection_request_received',
  'connection_request_undeliverable',
  'convergence_diag',
  'data_attachment_received',
  'data_attachment_requested',
  'data_attachment_unavailable',
  'data_changed',
  'data_doc_removed',
  'data_doc_size_warning',
  'data_doc_unsyncable',
  'diagnostic',
  'dors_escalation_triggered',
  'dors_score_updated',
  'dors_transport_selected',
  'dors_transport_switched',
  'file_progress',
  'file_receive_failed',
  'file_received',
  'fragment_assembly_evicted',
  'group_created',
  'group_epoch_fork_detected',
  'group_epoch_fork_resolved',
  'group_error',
  'group_info',
  'group_member_added',
  'group_member_removed',
  'group_message_delivery_report',
  'group_message_partial_failure',
  'group_message_received',
  'group_message_sent',
  'group_relay_sync_changed',
  'group_renamed',
  'group_rich_extras_dropped',
  'group_role_changed',
  'group_unauthorized_membership_change',
  'identity_ready',
  'internet_server_message',
  'internet_session_superseded',
  'internet_status_changed',
  'media_resend_required',
  'media_send_failed',
  'media_sent',
  'mesh_stopped_by_user',
  'message_decryption_failed',
  'message_deferred',
  'message_delivered',
  'message_failed',
  'message_received',
  'message_relayed',
  'message_retrying',
  'message_sent',
  'message_undeliverable',
  'neighbor_discovered',
  'neighbor_lost',
  'network_metrics',
  'peer_rssi',
  'presence_updated',
  'read_receipt_received',
  'relay_demoted',
  'relay_demoted_battery',
  'relay_promoted',
  'secure_session_established',
  'secure_session_failed',
  'security_warning',
  'service_discovered',
  'service_request_received',
  'service_response_received',
  'service_subscribe_received',
  'service_subscription_changed',
  'service_update_received',
  'transport_switched',
  'typing_indicator_received',
  'user_blocked',
  'user_groups',
  'user_unblocked',
  'username_resolved',
  'welcome_send_attempted',
  'welcome_send_expired',
  'welcome_send_failed',
  'welcome_send_succeeded',
] as const satisfies readonly EventType[];

/** Set view of {@link PROTOCOL_EVENT_TYPES} for membership checks. */
export const PROTOCOL_EVENT_TYPE_SET: ReadonlySet<string> = new Set(PROTOCOL_EVENT_TYPES);

export type ParseProtocolEventErrorCode =
  | 'invalid_json'
  | 'not_object'
  | 'missing_type'
  | 'invalid_type'
  | 'unknown_type';

export interface ParseProtocolEventError {
  code: ParseProtocolEventErrorCode;
  message: string;
}

export type ParseProtocolEventResult =
  | { ok: true; event: ProtocolEvent }
  | { ok: false; error: ParseProtocolEventError };

export interface ParseProtocolEventOptions {
  /**
   * When `true` (default), reject events whose `type` is not in
   * {@link PROTOCOL_EVENT_TYPES}. Set `false` to accept any non-empty string
   * `type` (forward compatibility when the core ships a tag before `types.ts`
   * catches up).
   */
  requireKnownType?: boolean;
}

function fail(
  code: ParseProtocolEventErrorCode,
  message: string
): ParseProtocolEventResult {
  return { ok: false, error: { code, message } };
}

/**
 * Reads a string `type` field when `value` is a plain object.
 */
export function protocolEventTypeOf(value: unknown): string | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return null;
  }
  const type = (value as { type?: unknown }).type;
  return typeof type === 'string' && type.length > 0 ? type : null;
}

/** Type guard for catalogued event tags. */
export function isProtocolEventType(type: string): type is EventType {
  return PROTOCOL_EVENT_TYPE_SET.has(type);
}

/**
 * Parses opaque event JSON (or a value already produced by `JSON.parse`).
 */
export function parseProtocolEvent(
  input: string | unknown,
  options: ParseProtocolEventOptions = {}
): ParseProtocolEventResult {
  const requireKnownType = options.requireKnownType !== false;
  let value: unknown = input;
  if (typeof input === 'string') {
    try {
      value = JSON.parse(input);
    } catch {
      return fail('invalid_json', 'Event payload is not valid JSON');
    }
  }

  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return fail('not_object', 'Event payload must be a JSON object');
  }

  const type = protocolEventTypeOf(value);
  if (type === null) {
    return fail('missing_type', 'Event payload must include a non-empty string `type`');
  }

  if (requireKnownType && !isProtocolEventType(type)) {
    return fail('unknown_type', `Unknown protocol event type: ${type}`);
  }

  return { ok: true, event: value as ProtocolEvent };
}

/**
 * Same as {@link parseProtocolEvent} but returns `null` instead of a result object.
 */
export function tryParseProtocolEvent(
  input: string | unknown,
  options?: ParseProtocolEventOptions
): ProtocolEvent | null {
  const result = parseProtocolEvent(input, options);
  return result.ok ? result.event : null;
}

/**
 * Narrows a {@link ProtocolEvent} by its `type` discriminant.
 */
export function narrowProtocolEvent<T extends ProtocolEvent['type']>(
  event: ProtocolEvent,
  type: T
): Extract<ProtocolEvent, { type: T }> | null {
  return event.type === type ? (event as Extract<ProtocolEvent, { type: T }>) : null;
}
