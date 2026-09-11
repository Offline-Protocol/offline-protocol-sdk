# Telemetry

The SDK can report how the mesh behaves for your users: delivery latencies,
transport dwell, routing switches, MLS handshake health, relay activity. It
does so as a hosted, metered service: the SDK collects, batches and uploads
accepted events to the Offline Protocol ingest itself, on a background thread
inside the native library, and the developer portal turns them into
dashboards, cross-app benchmarks and a privacy-preserving public report.

Hosted telemetry is inactive until the application calls `enableTelemetry`
with a telemetry API key and application identifier. Before that call
succeeds, the hosted telemetry client does not collect, buffer, persist, or
upload telemetry. The SDK's ordinary application traffic and public event APIs
operate independently of hosted telemetry. The hosted service authenticates
the supplied credentials.

This page is the complete inventory of what leaves the device, when, and how
to control it. [docs/privacy.md](privacy.md) has the app-store disclosures.
The wire contract itself is pinned by fixtures in
`crates/offline-protocol-telemetry-wire/fixtures/`.

## Enabling it

React Native:

```typescript
await protocol.start();
await protocol.enableTelemetry({
  apiKey: 'mp_…',      // from the developer portal
  appId: 'app_…',      // the app the key was issued against
  appVersion: '1.4.2', // your build's version, stamped on every batch
});
```

The native module fills in the platform and owns the application lifecycle:
entering the background closes the telemetry session and flushes it, and
returning to the foreground opens the next one. On iOS that flush runs inside
a `UIApplication` background task, so the send is not cut short by the process
being suspended; on Android it wakes the uploader thread and relies on the
process staying scheduled. There is no lifecycle call for the app to make.

Python:

```python
pm = ProtocolManager(config)
await pm.start()
pm.enable_telemetry("mp_…", "app_…", app_version="1.4.2")
```

Rust:

```rust
use offline_protocol::{TelemetryConfig, TelemetryHost, TelemetryOs};

proto.enable_telemetry(
    TelemetryConfig::default()
        .with_api_key("mp_…")
        .with_app_id("app_…")
        .with_app_version("1.4.2"),
    TelemetryHost::new(TelemetryOs::current(), 0),
)?;
```

A refused configuration (an empty key, an app id that is not printable
ASCII, a zero batch size, a flush interval under a second) is
`TelemetryConfigInvalid` naming the field, and nothing starts. Calling `enableTelemetry` again replaces the running pipe
after its final flush.

## What leaves the device

Every upload is one batch: an envelope and a list of events. The envelope is

| Field | Value |
|---|---|
| `batch_id` | A fresh UUID per batch, reused on every resend so the ingest can deduplicate |
| `session_id` | The telemetry session the events were observed under (see below) |
| `device_id` | Present only when `includeDeviceId` is set: the opaque per-install id, see [privacy](privacy.md#if-you-set-includedeviceid) |
| `wire_version` | `1` |
| `app_version` | What you passed |
| `os`, `os_major` | Filled by the binding: `ios` 18, `android` 34 (the API level), `macos` 14, `linux`, `windows` |
| `sdk_version` | The SDK version |

Each event is `{type, ts_ms, data}`. The pipe emits exactly these types, with
exactly these `data` fields, and nothing else:

| Type | Fields | Where it comes from |
|---|---|---|
| `protocol.message.delivered` | `latency_ms`, `hop_count`, `transport` | Every delivery acknowledgement |
| `protocol.message.failed` | `reason`, `retry_count` | A send that gave up |
| `protocol.message.deferred` | `reason`, `retry_count` | A send parked for later |
| `protocol.message.relayed` | `hop_count`, `remaining_ttl` | A frame this device forwarded for someone else |
| `protocol.relay.promoted` | `connection_count`, `battery_level` | This device started carrying traffic |
| `protocol.relay.demoted` | `reason` | It stopped |
| `protocol.relay.demoted_battery` | `battery_level`, `min_required` | It stopped on the battery floor |
| `protocol.routing.decision` | `phase`, `from`, `to`, `winning_score`, `reason_code` | DORS selected, switched, escalated, or re-scored |
| `protocol.transport.state_changed` | `transport`, `previous`, `current` | A transport changed status |
| `protocol.device.capability_changed` | `battery_level`, `is_charging`, `relay_role`, `changed_fields` | Battery, charging or relay role changed |
| `mls.decryption_failed` | `failure_kind`, `context` | An inbound frame would not decrypt |
| `mls.session_missing` | `context` | A send found no session, or a frame arrived for one this device does not hold |
| `mesh_metrics_rollup` | One row per minute: `bucket_start_ms`, `bucket_duration_s`, `neighbor_count_{avg,max,p50}`, `ack_pending_{avg,max}`, `retry_queue_total_{avg,max}`, `retry_queue_critical_count_max`, `transport_time_ms` (`ble`, `wifi_direct`, `internet`, `none`), `is_local_relay_duration_s`, `current_transport_at_bucket_end`, `sends_{attempted,succeeded,failed}_sum`, `bytes_{sent,received}_sum` (always 0) | The periodic metrics frames, folded on the device |
| `mesh_session_summary` | `session_duration_s`, `routing_switches`, `routing_escalations`, `escalation_reasons` (six buckets), `mls_session_ready_latency_p50_ms` (absent when no handshake paired), `mls_encryption_used_count` | Computed on the device at each session boundary |

`mls_session_ready_latency_p50_ms` is the median time from the engine wanting
a secure session with a peer and not having one, to that session being
usable. The window opens where a handshake begins: normally when a session is
created from the peer's key package and a Welcome goes out, or earlier when a
send found no key package either and had to wait for one. Where both happen
for a peer the earlier one is kept, so the span covers the whole wait. It is
paired per peer, so a handshake this device did not start contributes nothing
(a Welcome that simply arrived, or an encrypted frame that outran one), and a
want whose handshake never completes is discarded after ten minutes rather
than reported.

Three things about that table are worth stating plainly.

**No identifier has a field to land in.** Message ids, peer addresses, group
ids, usernames, message content, file names: none of them appear in any row
above. The pipe does read one identifier, the peer id on MLS lifecycle records
(hashed unless `scrubIds` is false), and only to pair a handshake's start with
its end for `mls_session_ready_latency_p50_ms`. It never reaches a batch, and
the golden fixture pins the bytes that do. See
[Identifier scrubbing](#identifier-scrubbing) for what `scrubIds` changes.

**`reason` is the engine's own fixed token.** `protocol.message.failed`,
`.deferred` and `protocol.relay.demoted` carry the reason verbatim, and every
reason the engine can produce is a locally chosen token or literal
(`recipient_unreachable`, `Max retries exceeded`, `no traffic carried for
other devices recently`), never text chosen by a remote party. That is the
[threat model's producer rule](security/threat-model.md#the-telemetry-producer-rule),
and it is why the classification of reasons into families happens on the
ingest rather than on the device.

**Some events are counted and never sent.** `protocol.message.sent` carries
the message content, so only its count survives, folded into the rollup's
`sends_attempted_sum`. `mls.session_establishing`, `mls.session_ready` and
`mls.encryption_used` are consumed on the device to compute the session
summary, and `mls.initialized` is dropped outright: it fires once per process
for no peer, so it starts nothing and counts nothing. `protocol.neighbor.discovered` and `.lost` are read as neighbour
churn, for which neither the summary nor the rollup has a field today, so
they currently contribute nothing. Every other event the engine emits,
sixty-odd of them, is dropped by name before its payload is read.

The `routingDiagnostic` option adds a `scores` array to routing decisions
(the seven scoring factors per transport). The ingest has no column for it;
it exists for local debugging with the tap below.

## When it leaves the device

The uploader runs on one background thread and wakes for these reasons:

- every `flushIntervalMs` (default 30 s), when there is something to send;
- when the buffered estimate passes `maxBatchBytes` (default 256 KiB);
- when the app enters the background, after the session summary;
- when the internet or Nostr transport becomes available after being down;
- on `flushTelemetry`, `endTelemetrySession` and `disableTelemetry`;
- when the protocol-state store attaches, to persist what is pending;
- when a backoff from a failed send expires.

Per wake it drains the ring buffer into batches, cuts them on session and then
on size, persists them, and sends at most eight before sleeping again. An
empty buffer opens no socket. Below 15% battery and not charging, every wake
is deferred except the background, explicit and disable flushes, which never
are. A deferred wake still cuts and persists batches; only the send waits.

A batch the ingest does not acknowledge is kept and retried: 1 s doubling to
15 min, plus up to a second of jitter. A 429 is the one answer whose
`Retry-After` is honoured, in seconds or as an HTTP date, capped at the same
15 min; a 408 or 5xx uses the doubling schedule whether or not it carries the
header. A batch older than six days is dropped unsent, because the ingest
deduplicates on `batch_id` for seven and a later replay would count twice.
A 401 or 403 (a bad key, or a key for another app) keeps the batch, records
the error in the stats, and stops sending until the next `enableTelemetry`:
the key is a configuration fault you fix and re-enable through, so the queue
waits rather than losing what it holds. Any other 4xx drops the batch and
continues, and so does any 3xx, because the uploader follows no redirect. A
followed 301, 302 or 303 would turn the `POST` into a `GET` at another
location, where a 2xx would delete the batch unsent and count it as accepted.

The queue is bounded at 64 batches and 4 MiB; beyond either the oldest batch
is dropped and counted. The ring buffer in front of it holds
`maxBufferedRecords` events (default 4096), oldest dropped first.

## Sessions

A session id is minted at `enableTelemetry`. Entering the background emits
the session summary and requests a flush; the first foreground after it
rotates the id, reporting first anything observed in between. Every buffered
event is stamped with the session current when it was buffered, and batches
are cut on that stamp, so a boundary event always carries the session it
reports. `endTelemetrySession` is the explicit form, for an app with its own
notion of a session; calling both is safe, because each summary reports only
what the previous one did not, and the ingest sums the counters.

Every field of a summary is a delta, `session_duration_s` included: a summary
reports the span since the previous one, not the span since the session
opened, so summing a session's rows gives its length exactly once. The first
boundary of a session always produces a row, because the duration is what the
row is for. A later boundary with nothing observed since the last one
produces none, so backgrounding and then calling `endTelemetrySession` is not
billed twice.

For the ten counters that is what the ingest already does with them.
`session_duration_s` is a contract change rather than a restored one: the
ingest stores that column and reads it in neither plane today, and rows
written by the client this release replaces carry the span since the session
opened, repeated in every summary. A reader that starts summing it therefore
has to know which client wrote the row. The counters beside it are unaffected.

## Where it goes and how

Every batch is `POST`ed to one endpoint, fixed at build time, over TLS 1.2 or
1.3 through `rustls` with the Mozilla root set compiled into the SDK. The
device's own trust store is not consulted, and the uploader follows no
redirect, so that endpoint is the only destination a batch is sent to.

The telemetry uploader can use an HTTP CONNECT proxy selected through the HTTP
client's supported environment configuration, read when telemetry is enabled.
TLS remains between the SDK and the destination server and is validated
against the bundled Mozilla root certificates. Installing an interception
certificate only in the device trust store does not make that certificate
trusted by the SDK. Such an interception attempt fails certificate validation;
queued telemetry remains subject to retry, capacity, and expiry limits.

Only HTTP CONNECT proxies are used. The client reads `ALL_PROXY`,
`HTTPS_PROXY` or `HTTP_PROXY` from the process environment, in that order and
in either case, and skips any host `NO_PROXY` names. A SOCKS proxy named there
is not used, and a system proxy setting that never reaches the process
environment is not read. The debug tap below is the way to inspect what is
sent.

The bundled root set also ages with the SDK release. The ingest stays on a
mainstream certificate authority; a change of CA is an SDK release event, and
an SDK too old to trust the ingest's chain reports it as a network error in
`telemetryStats().lastError` and backs off, sending nothing.

Requests carry `Authorization: Bearer <apiKey>`, `X-Mesh-Analytics-App-Id`,
`Content-Type: application/json`, `Idempotency-Key: <batch_id>` and
`User-Agent: offline-protocol-sdk/<version> (<os>/<major>)`. A connection is
kept for a minute, so the 30 s cadence reuses its TLS session.

## Durability

Batches are persisted on the protocol-state storage seam, each as its own
sealed record under the key type `telemetry_batch`, with an index record
under `telemetry_batch_index` and an owner record under
`telemetry_batch_owner`. Sealing uses the same per-install key as the
outbox, so no plaintext batch ever sits in the app container. A batch is
written before the index names it, so a crash between the two leaves an
orphan the next launch sweeps rather than a phantom entry.

On iOS and Android the storage seam reaches the SDK inside `initializeMls`.
A pipe enabled before that holds its batches in memory and persists whatever
is pending the moment storage attaches; a pipe enabled after it is durable
from the first batch. Python and Rust hosts that construct a store before
enabling telemetry are durable from the first batch. Batches persisted by
this release are ignored by an earlier SDK, so a downgrade costs at most
4 MiB of stale records.

One pipe writes a queue at a time. Two writers would lose batches rather
than duplicate them: each rewrites the index from its own memory, and the
next launch sweeps any batch the index does not name. A pipe that
`enableTelemetry` replaced, or that is still finishing after
`disableTelemetry` or a teardown, keeps the queue until its uploader thread
exits, which is after the drain it was in and its final flush. A pipe started
over the same storage in the meantime holds its batches in memory, then
adopts the queue behind what the previous pipe left once that pipe lets go.
If it is itself stopped first, it sends what it can in its own final flush
and nothing it held is persisted.

A queued batch is deleted rather than sent in three cases, each reported as a
warning on the pipe's log target rather than in `dropped`, because a record
nothing could open is a record whose event count is exactly what was lost:

- it will not open or parse on load, which after a record-key loss means the
  whole queue;
- it is an orphan, written before a crash that stopped the index naming it;
- it names a different `appId` than the one telemetry is now enabled with.
  The app id travels in a request header, not in the batch body, so uploading
  such a batch would count another application's events against your key.

An index entry whose record is already gone is not one of these and is not
reported. The index is written once per drain, so a crash mid-drain can
leave it naming batches that were accepted, expired or trimmed before the
crash, each counted when it went.

The queue outlives `disableTelemetry()`, so a re-enable resends what an
offline stretch collected. It is cleared by uninstalling the app, by
`wipePersistedState()`, and by the app-id rule above.

## The stats call

`telemetryStats()` returns `null` until telemetry is enabled, then:

| Field | Meaning |
|---|---|
| `buffered` | Events in the ring buffer, not yet cut into a batch |
| `sentEvents` | Events in batches the ingest answered 2xx |
| `acceptedEvents` | The `accepted` count the ingest reported, summed; a deduplicated replay adds nothing |
| `dropped` | Events lost: ring overflow, queue caps, the six-day expiry, or a permanent rejection. Counted in events, so it excludes records that would not open, which are counted in records and logged |
| `sessionId` | The current session id |
| `lastError` | The most recent send failure, or the configuration problem that halted sending |
| `lastFlushAtMs` | When a batch was last accepted |

`acceptedEvents` is the number the service meters. It is on the device so an
invoice can be checked against it.

## The debug tap

`debug: true` logs every wire event, exactly as serialized, at debug level
under the `offline_protocol::telemetry::pipe` target: logcat on Android
(tag `OfflineProtocol`), the unified log on iOS (subsystem
`com.offlineprotocol.sdk`), stderr on a desktop host. Nothing is delivered to
application code, and the tap costs nothing while it is off. The debug tap
exposes serialized telemetry locally for inspection. A CONNECT proxy tunnels
the encrypted connection and does not expose its payload; an interception
certificate trusted only by the device is not trusted by the SDK.

**That target is all the Rust core ever writes to a device log.** Enabling
telemetry installs the logger the tap needs, and it passes through the pipe's
own records and nothing else: the engine's internal logging, which names
groups, senders and peers, is dropped there whatever its level. A device log
is readable by anything on the device that can run `logcat`, so the filter is
a privacy boundary and not a volume setting. An embedder that does want the
engine's stream installs its own `tracing` or `log` subscriber, which the SDK
leaves in place.

The React Native bridges are not covered by that filter and never have been.
`OfflineProtocolModule` and the transport managers log through
`android.util.Log` and `print` directly, and some of those lines name a
profile or a peer address. That logging predates the pipe and this release
does not change it: an application that treats its device log as sensitive
should strip the bridge sources' logging, or ship a release build with
logging disabled at the platform level. Only the core's stream is filtered
here.

## Controlling cost

The service meters accepted events. The controls, from coarsest to finest:

- `setTelemetryEnabled(false)` stops new telemetry collection, including
  session-summary emission, without shutting down the uploader. Previously
  queued records continue to drain. This is a collection control, not an
  immediate network-stop or deletion operation.
- `disableTelemetry()` stops new collection and initiates a final flush. The
  call waits up to three seconds for shutdown; an in-flight HTTP request may
  complete after the call returns. Remaining persisted batches survive
  disabling and may upload when telemetry is enabled again under the same
  application identifier, subject to queue limits and expiry.
- `metricsCadenceMs` (default 5000) sets how often a metrics frame is
  produced. Frames never leave the device individually, but the minute
  rollup discards a window holding a single frame and no sends, so a cadence
  coarser than one frame a minute loses idle windows outright and makes the
  dwell attribution coarser for the rest.
- `mlsSamplingBypass` (default false) controls whether high-volume MLS
  failure events are rate-limited on the device before they count. Leave
  it off unless you need exact failure counts.
- `maxBatchBytes` and `flushIntervalMs` trade upload frequency for batch
  size; they do not change how many events are accepted.
- `acceptedEvents` and `dropped` in the stats are the reconciliation.

## The free event API is per-event and unaggregated by design

`onEvent` (and the Rust `on_event`) still hands every protocol event to your
code as it happens, under either license, and always will. What it does not
carry, and never will, is the rollup or the session summary: those two
aggregates exist only inside the pipe, and a test pins that no aggregate type
is ever an event. If you want the minute-grain and session-grain views, they
come from the service; if you want the raw stream, it is yours.

## Leaf nodes

A [leaf node](spec/leaf-provisioning.md) has no telemetry of its own in this
release, and cannot run the pipe: it has no thread, no TLS stack and no
socket, and it must not carry an API key, because firmware is the easiest
place to extract one from. Everything a leaf does that the phone observes
(pairing, key packages, sealed frames) is attributed to the phone's app and
session. Phone-observed leaf events are a follow-up, not a gap this release
closes.

## Keys, revocation and rate limits

The key is the only credential, and it lives in the app binary, where it can
be extracted. Anyone who extracts it can post accepted events against it and
the app is billed for them. The ingest limits requests per key and a key can
be revoked from the portal, which is the response to a leaked one; rotation
is a portal feature, not an SDK one. The threat model records this as
[residual risk R13](security/threat-model.md#r13-a-telemetry-key-can-be-extracted-and-abused).

## Errors

| Situation | What you see |
|---|---|
| `apiKey` or `appId` empty or not printable ASCII, empty `appVersion`, zero or over-1 MiB `maxBatchBytes`, zero `maxBufferedRecords`, `flushIntervalMs` under 1000 | `enableTelemetry` rejects with `TelemetryConfigInvalid` naming the field |
| Wrong key, or a key for another app | `lastError: "ingest responded 401"` (or 403); sending halts until the next `enableTelemetry` |
| No network | `lastError` names the transport failure; batches wait, bounded by the caps and the six-day expiry |
| Calling `flushTelemetry` or `telemetryStats` before `enableTelemetry` | A no-op, and `null` |

## Identifier scrubbing

The `scrubIds` option (default true) decides whether the engine hashes the
identifiers it stamps on MLS lifecycle records before they reach the pipe. It
does not apply to `onEvent`, which delivers every event with its identifiers
as they are. The pipe reads one of those identifiers, the peer id, and only to
pair a handshake's start with its end for `mls_session_ready_latency_p50_ms`.
No identifier has a wire field to land in, and the golden fixture pins the
uploaded bytes, so `scrubIds` changes what the pairer holds in memory (a
hashed peer id, or with `scrubIds: false` the raw one) and never what is
uploaded.
