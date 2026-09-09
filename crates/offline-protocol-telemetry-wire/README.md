# offline-protocol-telemetry-wire

The shape of one telemetry upload, shared between the SDK's telemetry pipe and the ingest service that receives it: the `Batch` envelope posted to `POST /v1/events`, the opaque `RawEvent` inside it, and the typed per-event payloads the ingest binds to columns.

Provides:

- `Batch`, `BatchId`, `SessionId`, `DeviceId`, `OsKind` and `CURRENT_WIRE_VERSION`, the envelope every upload carries
- `RawEvent`, the `{type, ts_ms, data}` record a batch is a list of
- One `*Data` struct per typed event, and `classify`, which turns a `(type, data)` pair into a typed `Event` or the forward-compatible `Event::Extension`
- The `TYPE_*` constants that name every event type on the wire

The SDK builds every payload it sends through these types, so a batch that serializes here is a batch the ingest accepts. The fixtures under `fixtures/` are the contract: `wire-batch-v1.json` is byte-identical to the copy the ingest service tests against, and `wire-batch-v1-reason.json` pins the additive `reason` field this crate adds.

## Provenance

`batch.rs` and `event.rs` are copied from the `mesh-analytics-wire` crate in the Offline Protocol telemetry service repository, which publishes them under the MIT OR Apache-2.0 licenses, Copyright Offline Protocol. The copy here is distributed as part of this workspace under the workspace license below, which those permissive licenses allow. Two things differ from the upstream copy: `ReasonRetryData` and `RelayDemotedData` carry a raw `reason` field alongside `reason_class`, and `OsKind` names the desktop hosts the SDK's Python binding runs on. Both are additive.

This crate is an internal layer of the SDK. Most Rust consumers want the main [`offline-protocol`](https://crates.io/crates/offline-protocol) crate instead, and app developers want the [React Native](https://www.npmjs.com/package/@offline-protocol/mesh-sdk) or [Python](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/bindings/python/README.md) bindings.

## License

Copyright © 2025-2026 Offline Protocol, Inc.

Dual-licensed: [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE) for open-source use, or a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md) for proprietary use.
