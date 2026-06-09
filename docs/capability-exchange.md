# Capability Exchange

An open capability exchange over the Offline Protocol mesh. Any participant — a human user or an autonomous agent — can publish a **listing**, discover listings across the mesh, invoke them, and settle payment for metered invocations. It works offline first and online when available.

A listing is one of two things:

- a **service**: a request/response capability another node invokes (physical, software, data, compute — anything the provider exposes), or
- an **adapter**: a model adapter artifact (LoRA or similar) that a consumer pulls, content-addressed and integrity-checked, to gain a local capability.

Two rules define the economics:

- Adapters are **free to publish and free to pull**, but publishing is an **attested publish** — signed by the publisher's OfflineID (MLS identity) key, never a raw upload.
- The **metered, paid event is the invocation**, settled with signed usage receipts against a prepaid balance, with a protocol fee applied at clearing.

Free-to-publish, metered-to-use.

## How it layers on service discovery

The exchange **wraps, never changes, the service discovery wire format**. A listing is the existing `ServiceDescriptor` plus a versioned JSON envelope carried in the descriptor's `capabilities` map under the reserved key `x-op-listing`:

```
ServiceDescriptor {
  service_id: "weather.v1",
  version: "1.0",
  capabilities: {
    "format": "json",                      // normal advertisory entries
    "x-op-listing": "{\"v\":1, ...}"       // the exchange envelope
  }
}
```

Exchange-unaware nodes see a plain service and interoperate untouched. Exchange-aware nodes parse the envelope, verify the attestation, and surface the listing.

The envelope (version `1`) carries:

| Field | Description |
|-------|-------------|
| `v` | Envelope schema version. Readers reject unknown versions. |
| `kind` | `"service"` or `"adapter"`. |
| `terms` | Pricing: `price` (minor units per unit, absent = free), `unit` (`per_call` \| `per_token` \| `per_second` \| `flat`), `currency`, `max_payload_kb`. |
| `artifact` | Adapters only: `content_hash` (SHA-256, lowercase hex), `size_bytes`, `base_model`, `base_model_version`, `chunking`. |
| `publisher` | Stable OfflineID user id. |
| `attestation` | Publisher's Ed25519 public key + signature over the canonical listing bytes + timestamp. |

## Provenance: attested publish

Publishing signs the canonical listing bytes (service id, version, kind, full terms, artifact hash, publisher, timestamp — length-prefixed, domain-separated `OP-EXCHANGE-LISTING-V1`) with the node's MLS Ed25519 identity key. **Publishing therefore requires `initialize_mls`.**

On discovery, consumers verify the signature and receive one of:

- `verified` — signature checks out against the canonical bytes
- `invalid` — signature present but wrong (tampered terms, wrong key, malformed)
- `unsigned` — no attestation

Paid invocation and adapter loading are **refused** unless the status is `verified`.

A **local reputation read** is surfaced alongside every discovery: `unknown` (never seen), `new` (verified listings, no settled history), `established` (at least one settled receipt), `flagged` (invalid attestation observed, or the publisher's attestation key changed — keys are pinned first-use per publisher). Reputation is local-only in v1: it reflects this node's own observations, not a global network.

## The plaintext rule

Service discovery runs as plaintext control messages so it can bootstrap before encryption exists. The exchange splits its traffic accordingly:

- **Discovery is plaintext-tolerant.** Listings are public adverts; their integrity comes from the attestation, not the channel.
- **Priced invocations and settlement never ride plaintext.** `invoke_listing` on a priced listing refuses to start without a **confirmed MLS session** with the provider, and usage declarations, receipts, and receipt acks are dropped rather than sent over an unconfirmed channel. (Exchange control messages are also Ed25519-signed and TOFU-checked like all control-plane traffic, and MLS-encrypted transparently once a session exists.)

## Metered invocation lifecycle

```
consumer                                         provider
  invoke_listing()
    ├─ require attestation verified
    ├─ require confirmed MLS session
    └─ hold = unit_price × max_units             handle request (app)
  __SVC_REQ__  ───────────────────────────────►  declare_invocation_usage(units)   [metered only]
                                                 respond_to_service_request()
  ◄───────────────────────────────  __SVC_RESP__
  ◄──────────────────────────────  __XCHG_USAGE__ [metered only]
  issue UsageReceipt (signed), commit hold,
  return remainder to balance
  __XCHG_RCPT__ ──────────────────────────────►  verify against published terms,
                                                 verify consumer signature,
                                                 counter-sign, store
  ◄──────────────────────────  __XCHG_RCPT_ACK__
  attach counter-signature (dual-signed)
```

- **Prepaid balance, two-phase**: the worst-case charge (`unit_price × max_units`) is held before the request leaves the node; the actual charge is debited when the receipt is issued and the remainder returns to the available balance. An error response or timeout releases the hold in full. Non-payment risk is bounded by the prepaid amount.
- **Metering**: `per_call`/`flat` bill exactly one unit. For `per_token`/`per_second` the provider declares the consumed units (before responding); the consumer bills the declared count **clamped to its agreed `max_units`** — a provider can never charge beyond the hold. A missing declaration falls back to one unit after a grace period.
- **Receipts are the durable claim.** Both parties store the receipt (`pending_settlement`); either signature alone proves one side's view, the dual-signed form is the strongest claim.

## Settlement

Settlement is **eventual**: receipts clear when connectivity allows, through a `SettlementBackend`. The protocol fee (take-rate, basis points, configurable per backend) is applied **at settlement**, never on the mesh.

- `MockClearing` ships in-crate for CI and tests: verifies receipts, applies the fee, accumulates per-identity balances, idempotent on receipt ids.
- Production clearing lives behind the same trait — mobile apps export `pending_exchange_receipts()`, submit them to the clearing service over HTTPS, and confirm with `mark_exchange_receipts_settled()`.

## Adapter distribution

Adapters are large, so distribution splits: the listing metadata and `ArtifactRef` gossip with discovery over any transport (BLE included), while the weights move over the media-transfer path, which DORS routes to a high-bandwidth transport (WiFi Direct / Internet).

- `pull_adapter` validates the listing (kind = adapter, attestation `verified`) and sends a reserved `exchange.adapter.pull` request.
- The provider auto-serves the artifact file registered at publish time — the app never handles pull requests.
- On arrival the bytes are verified against the attested `content_hash` (size first, then SHA-256). A match emits `AdapterPullCompleted` with the verified bytes; a mismatch emits `AdapterPullRejected` and **the bytes are discarded** — they never surface as a regular file.
- Loading is gated twice: `load_adapter` re-verifies the file against the attested hash immediately before handing it to the installed `AdapterRuntime`. An unverified adapter is unloadable.

## Rust API

```rust
use offline_protocol::{ListingKind, Terms, Price, BillingUnit, ListingFilter, MockClearing};

// Provider: publish a priced listing (requires initialize_mls)
let listing = protocol.publish_listing(
    descriptor,                       // plain ServiceDescriptor
    ListingKind::Service,
    Terms {
        price: Some(Price { amount_minor: 5 }),
        unit: BillingUnit::PerCall,
        currency: "USD".into(),
        max_payload_kb: 64,
    },
    None,
)?;

// Provider: publish an adapter from a local artifact file
let listing = protocol.publish_adapter_listing(
    descriptor, Terms::free(), "gemma-3-1b", "1.0", "/path/to/adapter.bin",
)?;

// Consumer: discover (results arrive as ListingDiscovered events)
let query_id = protocol.discover_listings(None)?;
let paid = protocol.discovered_listings(&ListingFilter { free: Some(false), ..Default::default() });

// Consumer: fund and invoke
protocol.credit_exchange_balance("USD", 1_000)?;     // after clearing confirms funding
let request_id = protocol.invoke_listing("bob", "weather.v1", "get_forecast", body, 1)?;

// Provider serving a metered listing: declare units before responding
protocol.declare_invocation_usage(&request_id, 37)?;
protocol.respond_to_service_request(&request_id, &sender, "llm.v1", "ok", &result)?;

// Consumer: pull and load an adapter
let pull_id = protocol.pull_adapter("bob", "adapter.medical")?;
// ... AdapterPullCompleted arrives with verified bytes; persist them, then:
protocol.set_adapter_runtime(runtime);
protocol.load_adapter("bob", "adapter.medical", "/path/to/saved/adapter.bin")?;

// Settlement on reconnect
let report = protocol.reconcile_exchange(&MockClearing::new(250))?;  // 2.5% fee
```

## Events

| Event | Side | Meaning |
|-------|------|---------|
| `ListingDiscovered` | consumer | Listing found, with `attestation_status` and `reputation`. Emitted alongside the plain `ServiceDiscovered`. |
| `ExchangeReceiptIssued` | consumer | Receipt signed, balance debited. |
| `ExchangeReceiptReceived` | provider | Receipt verified and counter-signed. |
| `ExchangeReceiptAcknowledged` | consumer | Local receipt is now dual-signed. |
| `ExchangeBalanceChanged` | consumer | Funding, hold, debit, or release. |
| `ExchangeInvocationFailed` | consumer | Error status or timeout; hold released. |
| `AdapterPullCompleted` | consumer | Artifact verified; base64 bytes attached. |
| `AdapterPullRejected` | consumer | Hash/size mismatch; bytes discarded. |

## Mobile / React Native API

`MeshExchange` is exposed via UniFFI to Swift, Kotlin, and React Native, mirroring the `MeshServices` pattern. Complex values cross the FFI as JSON.

```typescript
import { MeshExchange } from '@offline-protocol/mesh-sdk';

const exchange = new MeshExchange();

await exchange.publishListing('weather.v1', '1.0', { format: 'json' }, 'service', {
  price: { amount_minor: 5 },
  unit: 'per_call',
  currency: 'USD',
  max_payload_kb: 64,
});

await exchange.discoverListings();
protocol.on('listing_discovered', (event) => {
  if (event.attestation_status === 'verified') { /* show price + reputation */ }
});

await exchange.creditBalance('USD', 1000);
const requestId = await exchange.invokeListing('bob', 'weather.v1', 'get_forecast', body, 1);

protocol.on('exchange_receipt_issued', (event) => { /* receipt.total_minor debited */ });
protocol.on('adapter_pull_completed', (event) => { /* event.data = verified base64 bytes */ });

// Settlement: export → clearing service → confirm
const pending = await exchange.pendingReceipts();
// ... POST to clearing backend ...
await exchange.markReceiptsSettled(settledIds);
```

## Persistence

The durable exchange state — prepaid ledger, receipts, reputation — persists through the same `MlsStorage` backend as the rest of the protocol (key type `exchange_state`) and is restored by `initialize_mls` / `enable_message_persistence`. Published and discovered listings are runtime state: hosts re-publish on startup, and discovery refreshes the cache.

## Wire protocol details

| Prefix | Message | Channel rule |
|--------|---------|--------------|
| (envelope in `__SVC_DISC_R__`) | Listing discovery | Plaintext-tolerant; integrity via attestation |
| `__SVC_REQ__` / `__SVC_RESP__` | Invocation | Priced: confirmed MLS session required before send |
| `__XCHG_USAGE__` | Provider usage declaration | Confirmed MLS session required |
| `__XCHG_RCPT__` | Signed usage receipt | Confirmed MLS session required |
| `__XCHG_RCPT_ACK__` | Provider counter-signature | Confirmed MLS session required |

All `__XCHG_*` messages are signed control-plane messages (Ed25519 + TOFU), sent at High priority.

## Out of scope in v1

- A token or incentive layer (a clean seam is left: settlement is behind the `SettlementBackend` trait and `Terms.currency` is an open identifier).
- Escrow and post-paid settlement models.
- Behavioral sandboxing of adapters beyond signature + content-hash gating.
- Global/networked reputation.
