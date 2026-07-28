# Message Delivery & Reliability

## Overview

The Offline Protocol SDK is designed for environments where connectivity is intermittent or absent for extended periods. The delivery system ensures that messages persist until delivered (or expired), and are sent immediately when any transport becomes available.

This guide explains how messages move through the system, how failures are handled, and how client applications should integrate with the delivery lifecycle.

## Message Lifecycle

```
send_message()
    │
    ├─ Transport available ──► Send ──► ACK tracking ──► MessageDelivered
    │                                       │
    │                                       ├─ ACK timeout ──► Retry via queue (MessageRetrying)
    │                                       │                      │
    │                                       │                      ├─ Send succeeds ──► ACK tracking
    │                                       │                      └─ Max ACK retries ──► MessageFailed
    │                                       │                         (or re-park, if the recipient
    │                                       │                          is parked as unreachable)
    │                                       │
    │                                       └─ Relay: recipient unreachable ──► MessageUndeliverable
    │                                                              │              (message parked)
    │                                                              └─ Reachability edge ──► fresh ACK budget
    │
    └─ Transport unavailable ──► Outbox + Retry Queue ──► MessageDeferred
                                        │
                                        ├─ Backoff timer fires ──► Retry send
                                        ├─ Peer discovered ──► Immediate flush
                                        └─ Internet reconnects ──► Immediate flush
```

### Successful Send

1. `send_message()` creates the message and attempts transport delivery via DORS
2. On success, the message is registered for ACK tracking
3. When the recipient sends an ACK, a `MessageDelivered` event fires
4. The message is removed from outbox and retry queue

### Deferred Send

1. `send_message()` returns `Ok(message_id)` even when the transport send fails
2. The message is persisted to the outbox and enqueued in the retry queue
3. A `MessageDeferred` event fires with the reason
4. The retry queue will attempt redelivery on its next cycle

The caller always receives the message ID. A deferred message is not a failure — it's queued for delivery.

## Retry System

### Two Independent Retry Paths

The system has two distinct retry mechanisms that serve different purposes:

**Transport Retries** (retry queue):
- Fires when no transport can deliver the message right now
- The retry queue is a pure scheduling mechanism with no attempt limit
- Uses exponential backoff: 1s → 2s → 4s → 8s → ... → 30s (capped)
- Messages stay in the queue indefinitely as long as the process runs
- Processed in batches of 20 during each `process()` tick

**ACK Retries** (ACK manager):
- Fires when a message was sent but no acknowledgment arrived
- Each re-schedule emits a non-terminal `MessageRetrying` event with the actual `next_retry_at`
- Limited to `max_retries` attempts (default: 10)
- After exhausting retries, the message permanently fails with `MessageFailed` — unless the recipient is currently parked as unreachable, in which case the message re-parks instead of settling (see [Unreachable Recipients: Parking](#unreachable-recipients-parking))

This separation is critical: a message that can't reach any transport should not be permanently failed. Only messages that were sent but never acknowledged count toward the retry limit. The terminal paths for a regular message are ACK-retry exhaustion (against a reachable peer) and outbox lifetime/capacity expiry — nothing else settles it as failed.

### Exponential Backoff

```
Retry 0:  1s     (initial_delay_ms)
Retry 1:  2s     (1000 × 2^1)
Retry 2:  4s     (1000 × 2^2)
Retry 3:  8s     (1000 × 2^3)
Retry 4:  16s    (1000 × 2^4)
Retry 5+: 30s    (capped at max_delay_ms)
```

The backoff exponent is capped at 20 to prevent arithmetic overflow in the intermediate calculation. In practice, `max_delay_ms` (30s) kicks in much earlier.

### Priority Ordering

The retry queue is a priority-based min-heap. When multiple messages are ready:

1. Earlier `retry_at` times are dequeued first
2. Among messages with the same retry time, higher priority wins
3. Within the same priority, order is arbitrary

## Transport-Triggered Flush

When a transport becomes available, pending messages are sent immediately — they don't wait for their backoff timer to expire.

### Peer Discovery Flush

When `on_neighbor_discovered()` is called (a BLE or Wi-Fi Direct peer appears):

1. The outbox is scanned for messages addressed to that peer
2. Matching messages are removed from the retry queue
3. Each message is sent immediately (batch limit: 20)
4. Failed sends are re-enqueued with their current attempt count

This means a message queued while a peer was unreachable will be delivered as soon as that peer reappears, without waiting for the next backoff cycle.

### Internet Reconnect Flush

When `internet_status_changed(true)` is called after a disconnection:

1. All entries are drained from the retry queue (ignoring timing)
2. Outbox entries not in the retry queue are also collected (stranded messages)
3. Each message is sent immediately (batch limit: 20)
4. Failed sends are re-enqueued

### Batch Limits

Both flush methods cap at 20 messages per invocation to prevent blocking. If more messages are pending, the remainder stays in the retry queue and will be picked up on the next `process()` tick or the next flush.

## Outbox

The outbox persists messages that require acknowledgment. It serves as the source of truth for "what messages are in flight."

| Parameter | Default | Description |
|-----------|---------|-------------|
| Max entries | 500 | Regular messages |
| Max media entries | 100 | File chunk messages |
| Max lifetime | 7 days | `outbox_max_lifetime_ms` |

When the outbox is full, the oldest entry is evicted with a terminal `message_failed` event (reason `"Outbox capacity exceeded"`). When a message exceeds its lifetime and has no pending ACK, it is dropped and a terminal `message_failed` event (reason `"Outbox lifetime exceeded"`) is emitted so the app can settle its UI state.

**Important**: When a message storage backend is configured, regular-message outbox entries are persisted and restored on the next `start()` with a refreshed delivery window. Media chunks are never persisted — an interrupted transfer surfaces as `media_resend_required` instead. See [Client-Side Persistence](#client-side-persistence) for the app-side layer.

## Unreachable Recipients: Parking

When the internet relay reports a recipient unreachable for an in-flight regular message (its `recipient_unreachable` delivery verdict), the message does not burn its ACK retry budget against a peer that is provably offline. Instead it is **parked**:

1. A non-terminal `MessageUndeliverable` event fires (`message_id`, `recipient`, `reason`, and `file_id` when the message is a media chunk)
2. The pending ACK and the retry-queue entry are dropped — the retry machinery goes quiet
3. The outbox entry stays put, so the message remains "in flight" and subject only to the outbox lifetime

A parked message is re-driven with a fresh ACK budget on every reachability edge:

- Transport reconnect (`internet_status_changed(true)`)
- `start()` after a restart
- The peer being discovered on a local transport
- The peer coming online per presence — `internet_presence_watchlist()` includes recipients of pending/parked outbox messages, so the SDK owns presence-watching its own outbox; apps do not need their own watch queue for offline sends

**Reachability probing**: a parked message never goes fully quiet — the SDK keeps a timed reachability probe running on **every** carrier. The probe interval escalates with each consecutive unreachable park (15s doubling up to a 600s cap) and resets on any reachability edge. The escalation counter is per **recipient** while the probes are per message: a burst of DMs to one offline peer climbs the shared ladder once per park, so later messages start at an already-escalated interval rather than each walking 15s → 600s on their own — the delivery re-drive below is the compensating edge. If a probe attempt exhausts its ACK budget while the recipient still holds a live park counter, the message re-parks at the escalated interval rather than settling.

The probe is deliberately carrier-agnostic. With a local mesh carrier (BLE / Wi-Fi Direct) up the peer may be a room away even though the relay reports it offline — and possibly already a discovered neighbor, so no future edge would fire for it. On an internet-only device the external edges above are the *only* other recovery, which leaves delivery hostage to the platform's presence-polling cadence (and to nothing at all for a consumer that never polls presence). Probing over the relay is self-limiting in every outcome: a still-offline peer returns a fresh verdict that escalates the interval, an accepted frame becomes an ordinary in-flight send on the ACK ladder, and a peer that is back means the probe *was* the delivery.

Relay traffic is bounded differently in each of those branches. When the relay answers with a verdict, the escalation is the bound — one frame per interval per parked message, settling at one per 600s. When the relay *accepts* the frame instead (its push fallback succeeded, so no verdict comes back), the probe rides the ordinary ACK ladder — up to `max_retries` sends on 1s → 300s backoff, roughly 800s cumulative on the defaults — before re-parking at the escalated interval. Plan relay capacity against the second number, not the first. Delivery of any one message to a parked peer immediately re-drives that peer's remaining parked messages rather than leaving them on their own escalated timers.

The outbox lifetime bounds the entry itself, with one caveat worth knowing: each probe refreshes the entry's last-send timestamp, so the sliding 7-day window stops binding and terminal `message_failed` moves out to the absolute cap (4× the lifetime, i.e. ~28 days).

**What parks and what doesn't**:

| Message kind | Behavior on `recipient_unreachable` |
|--------------|-------------------------------------|
| Regular DM | Parked (as above) |
| Media chunk | Not parked — normal retry exhaustion → transfer abort → `media_resend_required` |
| Connection request | Not parked — settles immediately via `connection_request_undeliverable` |

**Contract**: `MessageUndeliverable` is the "recipient is offline" signal and may fire repeatedly for the same message while the peer stays offline. Terminal settlement happens only at delivery (`MessageDelivered`) or outbox-lifetime expiry (`MessageFailed`). Apps that previously keyed "recipient offline" UX off the ~15-minute terminal `message_failed` should key it off `message_undeliverable` instead.

## ACK Cleanup

When an ACK is received for a message:

1. `MessageDelivered` event is emitted with latency and hop count
2. The message is removed from the retry queue (prevents ghost re-sends)
3. The message is removed from the outbox
4. Transport delivery metrics are updated

When a message exceeds max ACK retries:

1. `MessageFailed` event is emitted
2. The message is removed from the retry queue
3. The ACK tracking is removed
4. The outbox entry is removed

## Configuration

### Reliability Parameters

```typescript
const config = {
  reliability: {
    ack: {
      defaultTimeoutMs: 10000,  // 10s ACK timeout (default)
      maxPendingAcks: 1000,
    },
    retry: {
      maxRetries: 10,              // ACK retry limit (default)
      initialDelayMs: 1000,        // First backoff delay
      maxDelayMs: 300000,          // Backoff ceiling (5 min)
      backoffMultiplier: 2.0,      // Exponential factor
      outboxMaxLifetimeMs: 604800000, // 7 day outbox lifetime
      pendingMessageMaxLifetimeMs: 604800000, // 7 days awaiting MLS session
    },
  },
};
```

The outbound queue waiting for MLS session establishment is also hard-bounded
at 64 messages per peer and 4096 messages globally. At capacity the oldest
entry is settled with `message_failed` before the new message is admitted.
Expiry work is scheduled from the earliest queued deadline rather than scanning
the queue on every `process()` tick.

### Tuning for Different Scenarios

**High-reliability (field operations, disaster response)**:
```typescript
reliability: {
  ack: { defaultTimeoutMs: 15000 },
  retry: {
    maxRetries: 20,
    outboxMaxLifetimeMs: 2592000000,  // 30 days
  },
}
```

**Low-latency (real-time chat with good connectivity)**:
```typescript
reliability: {
  ack: { defaultTimeoutMs: 5000 },
  retry: {
    maxRetries: 5,
    maxDelayMs: 10000,
  },
}
```

**Battery-constrained (IoT sensors)**:
```typescript
reliability: {
  retry: {
    maxRetries: 3,
    initialDelayMs: 5000,
    maxDelayMs: 60000,  // Longer backoff to save power
  },
}
```

## Events

The delivery system emits events at each stage of the message lifecycle:

| Event | When | Key Fields |
|-------|------|------------|
| `MessageSent` | Message accepted and sent via transport | `message_id`, `recipient`, `priority` |
| `MessageDeferred` | Message queued for retry (transport unavailable) | `message_id`, `reason`, `retry_count`, `next_retry_at` |
| `MessageRetrying` | Retry re-scheduled after a failed attempt (transport send error or ACK timeout); non-terminal | `message_id`, `recipient`, `retry_count`, `next_retry_at` |
| `MessageUndeliverable` | Transport reported the recipient unreachable; message parked, non-terminal | `message_id`, `recipient`, `reason`, `file_id?` |
| `MessageDelivered` | ACK received from recipient | `message_id`, `latency_ms`, `hop_count`, `transport` |
| `MessageFailed` | Terminal failure (max ACK retries, outbox lifetime or capacity exceeded) | `message_id`, `reason`, `retry_count` |
| `MediaResendRequired` | Interrupted outbound media transfer detected at `start()`; app must re-supply the bytes via `send_media` with the same `file_id` | `file_id`, `recipient`, `file_name`, `file_size` |

### Event Flow Examples

**Immediate delivery**:
```
MessageSent → MessageDelivered
```

**Deferred then delivered**:
```
MessageDeferred → (transport becomes available) → MessageDelivered
```

**Permanent failure**:
```
MessageSent → (ACK timeout) → MessageRetrying → ... → MessageFailed
```

**Offline recipient (internet path)**:
```
MessageSent → MessageUndeliverable (parked) → (peer comes online) → MessageSent → MessageDelivered
                                            → (7-day outbox lifetime expires) → MessageFailed
```

## Breaking Changes

This version introduces the following breaking changes to the delivery system:

### `send_message()` now returns `Ok` on transport failure

Previously, `send_message()` returned `Err` when no transport could deliver the message immediately. Now it returns `Ok(message_id)` and emits a `MessageDeferred` event instead. The message is queued for automatic retry.

**Migration**: If your code matches on `Err` from `send_message()` to detect "no transport available," switch to listening for `MessageDeferred` events instead. An `Err` from `send_message()` now only indicates a true failure (e.g., invalid recipient, protocol not started).

### `Error::MaxRetriesExceeded` removed

The `MaxRetriesExceeded` variant was removed from the reliability crate's error type. The retry queue no longer enforces a retry limit — only ACK timeouts count toward permanent failure.

**Migration**: Remove any match arms for `Error::MaxRetriesExceeded`. If you need to detect permanent failure, listen for the `MessageFailed` event.

### Default constants changed

| Parameter | Old Default | New Default |
|-----------|-------------|-------------|
| ACK timeout | 5,000 ms | 10,000 ms |
| Max ACK retries | 3 | 10 |

Messages will take longer to permanently fail (up to ~100 seconds worst-case vs ~15 seconds before). Configure lower values if you need faster failure detection:

```typescript
reliability: {
  ack: { defaultTimeoutMs: 5000 },
  retry: { maxRetries: 3 },
}
```

## Client-Side Persistence

When a message storage backend is configured, the SDK persists regular-message outbox entries and restores them on the next `start()` with a refreshed delivery window (media chunks are never persisted — see `media_resend_required`). The refresh is bounded: an entry whose total age exceeds 4× the outbox lifetime (28 days at the default) is dropped at restore with a terminal `message_failed` instead of re-granted a window. Client applications should still maintain their own persistence layer for message history and UI state.

### Recommended Pattern

```
┌─────────────────────────────────────┐
│           Client App                │
│                                     │
│  ┌───────────┐   ┌──────────────┐  │
│  │ Local DB  │   │  Message UI  │  │
│  │ (SQLite)  │   │              │  │
│  └─────┬─────┘   └──────────────┘  │
│        │                            │
│  Store message    Read events       │
│  with status      and update        │
│        │          status            │
│        ▼                            │
│  ┌──────────────────────────────┐   │
│  │     Offline Protocol SDK     │   │
│  │  (in-memory retry + outbox)  │   │
│  └──────────────────────────────┘   │
└─────────────────────────────────────┘
```

### Implementation Steps

1. **On send**: Store the message in local DB with `status: pending`

```typescript
// Send the message
const messageId = protocol.sendMessage(recipient, content, priority);

// Persist locally
db.insert({
  id: messageId,
  recipient,
  content,
  priority,
  status: 'pending',
  createdAt: Date.now(),
});
```

2. **On delivery**: Update status when ACK arrives

```typescript
protocol.onEvent((event) => {
  if (event.type === 'message_delivered') {
    db.update(event.message_id, { status: 'delivered' });
  }
  if (event.type === 'message_failed') {
    db.update(event.message_id, { status: 'failed' });
  }
});
```

3. **On app restart**: Re-send pending messages

```typescript
const pending = db.query({ status: 'pending' });
for (const msg of pending) {
  // Optional: skip messages older than your retention policy
  if (Date.now() - msg.createdAt > 7 * 24 * 3600 * 1000) {
    db.update(msg.id, { status: 'expired' });
    continue;
  }
  protocol.sendMessage(msg.recipient, msg.content, msg.priority);
}
```

### Considerations

- **Duplicate delivery**: If the original send succeeded but the app was killed before receiving the ACK, re-sending creates a duplicate. The receiver's deduplicator catches duplicates by message ID, but re-sends generate a *new* message ID. Consider content-level dedup in the UI if this matters for your use case.
- **Message ordering**: Re-sent messages get new timestamps and Lamport clocks. If strict ordering matters, the client should track sequence numbers.
- **Retention policy**: Decide how long to keep unsent messages. A disaster-response app might keep them for days; a chat app might expire after hours.
- **Storage limits**: Cap the local pending queue to prevent unbounded growth during extended offline periods.
