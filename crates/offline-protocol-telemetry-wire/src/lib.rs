//! Wire-format types for the telemetry ingest, shared between the SDK's
//! telemetry pipe (`offline-protocol`, `telemetry::pipe`) and the ingest
//! service that receives what it sends.
//!
//! The surface is deliberately opaque at the envelope: a [`RawEvent`] is a
//! `type` discriminator, a `ts_ms` timestamp and a `data` JSON payload, and a
//! [`Batch`] is a list of them under one session. The typed layer ([`Event`]
//! and the per-type `*Data` structs, reached through [`classify`]) is what the
//! ingest binds to columns; the pipe builds every payload it sends through
//! those same structs, so a payload that serializes here is one the ingest
//! accepts.
//!
//! `batch.rs` and `event.rs` are copied from the ingest service's own wire
//! crate. Keep them that way: an edit that is not additive belongs upstream
//! first, and `tests/fixtures.rs` pins the canonical batch byte for byte.

#![deny(unsafe_code)]

pub mod batch;
pub mod event;

pub use batch::{Batch, BatchId, DeviceId, OsKind, SessionId, WireVersion, CURRENT_WIRE_VERSION};
pub use event::{classify, Event, RawEvent};
