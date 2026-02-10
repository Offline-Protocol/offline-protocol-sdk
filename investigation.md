# DORS Reliability Investigation

## Expected Behavior Model

When `preferOnline = true` and `sendMessage()` is called:

1. DORS scores Internet transport with a **+25 baseline bonus** plus weighted bandwidth (35%), reliability (30%), congestion (15%), energy (10%), load (10%)
2. BLE scores with signal (30%), energy (30%), congestion (15%), proximity (15%), reliability (5%), load (5%) — **no baseline bonus**
3. Internet should win decisively and be used for sending
4. BLE should only be used when Internet transport's status is not `Available`
5. Send failures should be detected, retried, and if unrecoverable, reported to the caller

## Actual Behavior Observed (Verified from Code)

The app's complaints about unreliability are **well-founded**. The code reveals seven distinct root causes that conspire to produce unpredictable routing, silent message loss, and spurious BLE fallback.

## Root Causes Identified

### RC-1: Internet `send()` Returns False Success (Critical)

```rust
// crates/offline-protocol-transport/src/internet.rs:258-275
fn send(&self, message: &Message) -> Result<()> {
    // Check status
    if self.status() != TransportStatus::Available {
        return Err(crate::Error::TransportNotAvailable(
            "Internet transport is not available".to_string(),
        ));
    }

    // Add to send queue
    let mut queue = self.send_queue.lock().unwrap();
    queue.push_back(message.clone());

    // Update metrics
    let mut metrics = self.metrics.lock().unwrap();
    metrics.queue_depth = queue.len();
    metrics.congestion = ((metrics.queue_depth as f32) / 25.0).clamp(0.0, 1.0);

    Ok(())
}
```

Internet transport `send()` returns `Ok(())` after **enqueueing** the message, not after actually transmitting it. The message sits in `send_queue` waiting for the platform layer to call `internet_get_next_message()` to drain it. There is no feedback loop: if the platform never polls, or the WebSocket drops the message, or the queue grows unbounded, the SDK has already reported success.

This is the **single most impactful bug**. Every downstream assumption — ACK registration, success tracking, retry decisions — is based on the lie that the message was sent.

### RC-2: `send_message()` Returns `Ok` Even When Send Fails (Critical)

```rust
// crates/offline-protocol/src/protocol.rs:740-758
match send_result {
    Ok(()) => {
        self.handle_send_success(&message, current_transport)?;
    }
    Err(err) => {
        self.handle_send_failure(&message, current_transport.or(previous_transport))?;
        warn!(
            message_id = %message.id,
            error = %err,
            "Send failed, message deferred"
        );
    }
}
// ...
Ok(message_id)
```

Even when `send_result` is `Err`, the function returns `Ok(message_id)` to the caller. The app cannot distinguish a successful send from a deferred failure. Combined with RC-1, the app has **zero reliable signal** about message delivery status.

### RC-3: UniFFI Layer Forces BLE to Available (Critical)

```rust
// crates/offline-protocol-uniffi/src/lib.rs:961-978
//  Ensure BLE transport is available before attempting to send
// This is especially important when BLE is the only transport enabled (Internet/WiFi disabled)
// BLE should work independently and be available for message sending
if let Some(transport_arc) = protocol
    .transport_manager()
    .get_transport(CoreTransportType::BLE)
{
    let transport = transport_arc.lock().unwrap();
    if let Some(ble_transport) = transport.as_any().downcast_ref::<BleTransport>() {
        let current_status = ble_transport.status();
        if current_status != offline_protocol_transport::TransportStatus::Available {
            // Force status to Available if BLE transport exists
            ble_transport
                .on_status_changed(offline_protocol_transport::TransportStatus::Available);
        }
    }
}
```

On **every single `send_message()` call**, the UniFFI layer forces BLE status to `Available` regardless of actual BLE state. This means:
- DORS always sees BLE as available, even if no peers are connected
- BLE's `success_count` / `failure_count` never accurately reflect reality
- When Internet has transient issues, DORS sees BLE as a viable alternative and may select it
- The forced-available BLE competes with Internet in scoring, making routing unpredictable

### RC-4: No Transport-Level Fallback After Selection Failure (High)

```rust
// crates/offline-protocol/src/transport_manager.rs:157-188
pub fn send(&mut self, message: &Message) -> Result<()> {
    let transport_type = self
        .select_transport(message)
        .ok_or_else(|| { /* error */ })?;

    self.current_transport = Some(transport_type);

    let transport = self.transports.get(&transport_type)
        .ok_or_else(|| Error::Other("Transport not found".to_string()))?;

    let transport_lock = transport.lock().map_err(|_| { /* error */ })?;
    transport_lock.send(message)
        .map_err(|e| Error::Other(format!("Transport send failed: {}", e)))?;

    Ok(())
}
```

When DORS selects a transport and that transport's `send()` fails, there is **no attempt to try the next-best transport**. The failure propagates to `handle_send_failure()` which enqueues for retry with exponential backoff (1s, 2s, 4s... up to 30s). A message that could have been sent immediately via another transport instead waits seconds before retry.

### RC-5: `current_transport` Set Before Send Confirmation (Medium)

```rust
// crates/offline-protocol/src/transport_manager.rs:171-172
// Update current transport
self.current_transport = Some(transport_type);
```

`current_transport` is updated **before** the send call succeeds. If the send fails, subsequent code paths that use `current_transport` (e.g., retry failure recording, event emission) will attribute the failure to the wrong transport. In `process_retry_queue()`, failure recording uses `previous_transport` (line 1852) while success uses `current_transport` (line 1826), creating an asymmetry that corrupts DORS's success/failure tracking.

### RC-6: Internet Transport Metrics Never Reflect Real Success/Failure (High)

The Internet transport's `success_count` and `failure_count` in `TransportMetrics` stay at their defaults (0/0) because:
1. `send()` always returns `Ok(())` (it's just a queue push — RC-1)
2. There is no callback from the platform to report actual WebSocket send success/failure
3. The `ObservedStats` overlay in `TransportManager` only records when `record_delivery_success()` or `record_delivery_failure()` are explicitly called — but these are only called on **ACK receipt/timeout**, not on transport-level send

This means DORS computes Internet's reliability score from **no real data**. It defaults to 0.85 (the hardcoded fallback in `calculate_reliability_score()`). Internet's reliability score is a fiction.

### RC-7: `internet_return_message()` is a No-Op (Medium)

```rust
// crates/offline-protocol-uniffi/src/lib.rs:1433-1436
/// Internet: Return message (marks last message as sent)
pub fn internet_return_message(&self) {
    // No-op for now - message sending confirmation is handled by WebSocket
}
```

The method that should confirm Internet message transmission does nothing. The platform has no way to tell the SDK that a message was actually sent over the wire.

## Weaknesses in Current Design

### W-1: Queue-Based Internet Transport Without Delivery Feedback

The Internet transport is a pair of in-memory queues (`send_queue`, `receive_queue`) with no delivery acknowledgment mechanism. The platform drains `send_queue` via polling (`internet_get_next_message`), but there's no `internet_confirm_sent(message_id)` that feeds back into the transport's metrics or DORS's scoring. This makes Internet transport a black box to the routing layer.

### W-2: DORS preferOnline Bonus is Too Weak Under Certain Conditions

With `prefer_online = true`, Internet gets a +25 baseline. Let's compute a realistic scenario:

**Internet (connected, default metrics):**
- bandwidth_score = 100.0 (default for Internet)
- reliability_score = 85.0 (default 0.85 * 100)
- congestion_score ≈ 100.0 (assuming 0 congestion)
- energy_score ≈ 60.0 (Internet baseline)
- load_score ≈ 100.0 (assuming empty queue)
- **Total = 25.0 + (100×0.35) + (85×0.3) + (100×0.15) + (60×0.1) + (100×0.1) = 25+35+25.5+15+6+10 = 116.5** → clamped to **100.0**

**BLE (forced Available, default metrics with RSSI -60):**
- signal_score ≈ 85.0
- energy_score = 90.0
- congestion_score ≈ 97.0 (low congestion)
- proximity_score = 100.0 (hop_count=0)
- reliability_score = 85.0 (default 0.85)
- load_score ≈ 97.0
- **Total = (85×0.3) + (90×0.3) + (97×0.15) + (100×0.15) + (85×0.05) + (97×0.05) = 25.5+27+14.55+15+4.25+4.85 = 91.15**

In the default case, Internet wins (100 vs 91.15). But the margin is only **8.85 points**, which is less than the **15-point hysteresis**. This means: once DORS is on BLE, it will **never switch to Internet** under normal conditions because the improvement doesn't exceed hysteresis. The initial selection matters enormously, and because RC-3 forces BLE to Available before every send, the first-call race determines long-term routing.

### W-3: Hysteresis + Cooldown + Stability Triple-Gate is Too Conservative

To switch transports, DORS requires ALL of:
1. Score improvement ≥ 15 points (hysteresis)
2. At least 20 seconds since last switch (cooldown)
3. The candidate must have been consistently better over an 8-second window with ≥ 7.5 point average improvement (stability)

This triple-gate is designed to prevent flapping, but it makes DORS **extremely reluctant to switch**, even when the current transport is clearly degraded but hasn't triggered emergency conditions (retry failures < 2, success rate not yet below 30%). A transport can be performing poorly for 20+ seconds before DORS reacts.

### W-4: Emergency Switch Only Triggers on BLE-Specific Conditions

`is_emergency_switch_needed()` checks:
- `retry_counts >= ble_to_wifi_retry_threshold` (default: 2)
- Historical success rate < 30%
- RSSI < -90 dBm for > 20 seconds

For Internet transport, only the first two conditions apply. But because Internet `send()` always returns `Ok()` (RC-1), retry failures are **never recorded for Internet**. Internet can never trigger an emergency switch. DORS will keep selecting Internet even when the WebSocket is silently failing.

### W-5: No Observability Into DORS Decisions

There is no logging or event emission for:
- Which transport was selected and why
- What scores were computed
- Whether hysteresis/cooldown/stability blocked a switch
- What the transport health states were at decision time

The only transport-related event is `TransportSwitched`, which fires after the fact and only reports from/to transport names without scores or reasons.

## Observability Gaps

| Question | Can Answer Today? | Why Not |
|----------|------------------|---------|
| Why was this transport chosen? | No | No logging in `select_transport()` |
| What were the scores? | No | Scores computed but not emitted |
| What was the health state? | No | Transport metrics not logged at decision time |
| Why was Internet rejected? | No | No logging when transport excluded from available set |
| Was hysteresis blocking a switch? | No | `should_switch()` returns bool without explanation |
| Did the message actually leave the device? | No | Internet `send()` returns Ok on enqueue |
| Was BLE forced to Available? | No | No logging when status is force-overridden |
| What is the retry queue depth? | No | No metric or event for retry queue state |

## Proposed Improvements

### Fix 1: Internet Transport Send Confirmation Loop (RC-1, RC-7)

Replace the fire-and-forget queue model with a confirmed-delivery model:

**In `internet.rs`:**
- Add `pending_confirmation: Arc<Mutex<HashMap<MessageId, Instant>>>` to `InternetTransport`
- `send()` remains queue-based but moves messages to pending-confirmation state
- Add `confirm_sent(message_id: &str) -> Result<()>` method
- Add `report_send_failure(message_id: &str) -> Result<()>` method
- Messages in pending-confirmation for too long (e.g., 10s) should be reported as failed

**In UniFFI layer:**
- Replace `internet_return_message()` no-op with `internet_confirm_sent(message_id)` and `internet_send_failed(message_id, reason)`
- Platform (JS/Swift/Kotlin) calls these after WebSocket send succeeds or fails

### Fix 2: Remove Forced BLE Available Hack (RC-3)

Remove lines 961-978 in `crates/offline-protocol-uniffi/src/lib.rs`. If BLE needs to be available, the platform layer should call `ble_status_changed(true)` when BLE is ready. The current hack defeats DORS's purpose by giving it false information.

If backward compatibility is needed, make this behavior opt-in via config rather than unconditional.

### Fix 3: Return Send Status Honestly (RC-2)

Change `send_message()` to return a richer result type:

```rust
pub enum SendStatus {
    Sent { message_id: MessageId, transport: TransportType },
    Queued { message_id: MessageId, reason: String },
    Failed { message_id: MessageId, error: String },
}
```

Or at minimum, return `Err` when the transport send fails, so the app can implement its own retry logic.

### Fix 4: Transport-Level Fallback in `TransportManager::send()` (RC-4)

When the primary transport fails, attempt the next-best transport before returning an error:

```rust
pub fn send(&mut self, message: &Message) -> Result<()> {
    let available = self.get_available_transports();
    let mut scored = self.selector.score_all(message, &available);
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));

    for (transport_type, _score) in &scored {
        self.current_transport = Some(*transport_type);
        match self.try_send(message, *transport_type) {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(transport = ?transport_type, error = %e, "Transport send failed, trying next");
                self.selector.record_retry_failure(*transport_type);
                continue;
            }
        }
    }
    Err(Error::Other("All transports failed".to_string()))
}
```

### Fix 5: Fix `current_transport` Update Timing (RC-5)

Move `self.current_transport = Some(transport_type)` to after the send succeeds, or track both "attempted" and "confirmed" transport:

```rust
let result = transport_lock.send(message);
if result.is_ok() {
    self.current_transport = Some(transport_type);
}
result
```

### Fix 6: Increase `prefer_online` Baseline or Reduce Hysteresis for Internet (W-2, W-3)

Two complementary changes:

**Option A:** Increase Internet baseline to 40.0 when `prefer_online = true`, ensuring the gap exceeds hysteresis:

```rust
let baseline = if self.config.prefer_online { 40.0 } else { 10.0 };
```

**Option B:** When `prefer_online = true` and Internet is available, reduce switch hysteresis for switching **to** Internet:

```rust
let effective_hysteresis = if self.config.prefer_online
    && candidate_transport == TransportType::Internet {
    self.config.switch_hysteresis / 2.0
} else {
    self.config.switch_hysteresis
};
```

### Fix 7: Add DORS Decision Logging (W-5)

Add structured logging in `select_transport()`:

```rust
debug!(
    transport = ?best_transport,
    score = best_score,
    all_scores = ?scored_transports.iter().map(|(t,s)| (t, s.total)).collect::<Vec<_>>(),
    prefer_online = self.config.prefer_online,
    previous = ?self.current_transport,
    switched = (self.current_transport != Some(best_transport)),
    "DORS transport selected"
);
```

And emit a new `DorsDecision` event with scores, health states, and switching reasons.

## Implementation Plan

### Phase 1: Critical Fixes (Reliability)

| # | File | Change | Priority |
|---|------|--------|----------|
| 1 | `crates/offline-protocol-transport/src/internet.rs` | Add `confirm_sent()` / `report_send_failure()` methods with pending-confirmation tracking | P0 |
| 2 | `crates/offline-protocol-uniffi/src/lib.rs` | Replace `internet_return_message()` no-op with `internet_confirm_sent()` and `internet_send_failed()` | P0 |
| 3 | `crates/offline-protocol-uniffi/src/lib.rs` | Remove forced BLE Available hack (lines 961-978) | P0 |
| 4 | `crates/offline-protocol/src/protocol.rs` | Return `Err` from `send_message()` when transport send fails | P0 |
| 5 | `crates/offline-protocol/src/transport_manager.rs` | Add fallback to next-best transport on send failure | P1 |
| 6 | `crates/offline-protocol/src/transport_manager.rs` | Fix `current_transport` update timing (set after success) | P1 |

### Phase 2: Routing Accuracy

| # | File | Change | Priority |
|---|------|--------|----------|
| 7 | `crates/offline-protocol-router/src/dors.rs` | Increase `prefer_online` baseline to 40.0 | P1 |
| 8 | `crates/offline-protocol-router/src/dors.rs` | Reduce hysteresis for switching **to** Internet when `prefer_online = true` | P1 |
| 9 | `crates/offline-protocol-router/src/dors.rs` | Ensure emergency switch logic works for Internet (not just BLE) | P2 |

### Phase 3: Observability

| # | File | Change | Priority |
|---|------|--------|----------|
| 10 | `crates/offline-protocol-router/src/dors.rs` | Add structured `debug!` logging in `select_transport()` and `should_switch()` | P1 |
| 11 | `crates/offline-protocol/src/events.rs` | Add `DorsDecision` event variant with scores and reason | P2 |
| 12 | `crates/offline-protocol/src/transport_manager.rs` | Log when transport excluded from available set and why | P2 |

### Phase 4: Tests

| # | File | Test | Priority |
|---|------|------|----------|
| 13 | `crates/offline-protocol-router/src/dors.rs` | Test: `prefer_online` Internet score exceeds BLE by more than hysteresis | P1 |
| 14 | `crates/offline-protocol-router/src/dors.rs` | Test: Internet failure triggers emergency switch when success rate drops | P1 |
| 15 | `crates/offline-protocol/src/transport_manager.rs` | Test: fallback to second transport on primary failure | P1 |
| 16 | `crates/offline-protocol/src/protocol.rs` | Test: `send_message()` returns error when all transports fail | P2 |
| 17 | `crates/offline-protocol-transport/src/internet.rs` | Test: messages in pending-confirmation timeout correctly | P2 |
| 18 | Integration test: end-to-end with simulated Internet disconnect mid-send | P2 |

## Risks and Tradeoffs

**Risk: Removing forced BLE breaks apps that depend on it.**
The forced-BLE hack was added because BLE status reporting from the platform was unreliable. Removing it may break apps where platform code doesn't properly call `ble_status_changed()`. **Mitigation:** Add a `force_ble_available` config flag defaulting to `false`, document migration, and log a warning if BLE is the only transport and its status is not Available.

**Risk: Changing `send_message()` return type is a breaking API change.**
Apps that don't handle errors will need updating. **Mitigation:** Phase this by first adding the `DorsDecision` event so apps can observe failures before the return type changes. Or return a new `SendResult` enum with backward-compatible serialization.

**Risk: Transport fallback increases latency for the first message.**
If Internet is selected, fails, then falls back to BLE, the first message takes longer. **Mitigation:** This is strictly better than the current behavior where the message is silently queued for retry after 1+ seconds of backoff.

**Risk: Increased logging volume in production.**
DORS decision logging fires on every `send_message()`. **Mitigation:** Use `debug!` level (not `info!`) so it's off by default. The `DorsDecision` event should be opt-in.

**Simplicity check:** None of these changes introduce new abstractions, new crates, or architectural changes. They fix specific, identified bugs in existing code paths. The largest change (transport fallback) is ~20 lines. The most impactful fix (Internet send confirmation) adds a small feedback mechanism to an existing queue-based design.
